//! Where the folder suggestions come from, and how a query narrows them.
//!
//! Two modes, chosen by what the query looks like:
//!
//! * **path mode** -- the query names a path (it starts with `/`, `~` or `.`,
//!   or contains a separator). Suggestions are the real subdirectories of the
//!   parent it names, so completion is always against the filesystem as it
//!   actually is rather than against a remembered list.
//! * **catalog mode** -- the query is a bare word, or empty. Suggestions come
//!   from a catalog assembled when the picker opens: folders that already have
//!   a space, folders picked recently, the children of the configured roots,
//!   and zoxide's frecency list when it is installed.
//!
//! Both produce the same row type, so the list does not change shape as you
//! type your way from one into the other.

use crate::fuzzy::{match_prepared, split_query, Match, Prepared, Term};
use crate::pyjson::Value;
use crate::pyos;
use crate::pystr;
use crate::session::{clear_resolved, normalize, space_folders, SpaceMap};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

// Rank breaks ties between sources when two folders score the same. Lower wins.
pub const SOURCE_SPACE: &str = "space";
pub const SOURCE_RECENT: &str = "recent";
pub const SOURCE_ROOT: &str = "root";
pub const SOURCE_ZOXIDE: &str = "zoxide";
pub const SOURCE_PATH: &str = "path";

pub fn rank(source: &str) -> u8 {
    match source {
        SOURCE_PATH => 0,
        SOURCE_RECENT => 1,
        SOURCE_SPACE => 2,
        SOURCE_ZOXIDE => 3,
        _ => 4,
    }
}

/// A folder's own name is what you are almost always aiming at, so a hit
/// there outweighs the same hit buried in the middle of its path.
pub const NAME_BONUS: i64 = 24;

#[derive(Debug)]
pub struct Suggestion {
    pub path: String, // absolute, normalized
    pub name: String, // the folder's own name, which is what the row leads with
    pub source: &'static str,
    pub detail: String, // the space's label, for folders that already have one
    pub is_git: bool,
    // Worked out once per row rather than once per keystroke: the path as it
    // is shown, and both fields ready for the matcher.
    shown: OnceLock<String>,
    name_field: OnceLock<Prepared>,
    shown_field: OnceLock<Prepared>,
}

impl Clone for Suggestion {
    fn clone(&self) -> Self {
        Suggestion::with(self.path.clone(), self.name.clone(), self.source, self.detail.clone(), self.is_git)
    }
}

impl PartialEq for Suggestion {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.name == other.name
            && self.source == other.source
            && self.detail == other.detail
            && self.is_git == other.is_git
    }
}

impl Suggestion {
    pub fn with(path: String, name: String, source: &'static str, detail: String, is_git: bool) -> Suggestion {
        Suggestion {
            path,
            name,
            source,
            detail,
            is_git,
            shown: OnceLock::new(),
            name_field: OnceLock::new(),
            shown_field: OnceLock::new(),
        }
    }

    pub fn new(path: &str, name: &str, source: &'static str) -> Suggestion {
        Suggestion::with(path.to_string(), name.to_string(), source, String::new(), false)
    }

    /// `display(self.path)`, worked out once.
    pub fn shown(&self) -> &str {
        self.shown.get_or_init(|| display(&self.path))
    }

    fn name_field(&self) -> &Prepared {
        self.name_field.get_or_init(|| Prepared::new(&self.name))
    }

    fn shown_field(&self) -> &Prepared {
        self.shown_field.get_or_init(|| Prepared::new(self.shown()))
    }

    /// Fill the matcher's caches now, so scoring never has to.
    pub fn prepare(&self) {
        self.name_field();
        self.shown_field();
    }
}

pub type Row = Arc<Suggestion>;

#[derive(Clone, Debug, PartialEq)]
pub struct Scored {
    pub suggestion: Row,
    pub score: i64,
    pub name_positions: Vec<usize>,
    pub path_positions: Vec<usize>,
}

impl Scored {
    pub fn plain(suggestion: Row) -> Scored {
        Scored { suggestion, score: 0, name_positions: Vec::new(), path_positions: Vec::new() }
    }
}

// Every row's path is written for display against home, so a keystroke asks
// for it once per folder in the catalog. A process cannot move house while it
// runs.
thread_local! {
    static HOME: RefCell<String> = const { RefCell::new(String::new()) };
}

pub fn home() -> String {
    HOME.with(|home| {
        let mut home = home.borrow_mut();
        if home.is_empty() {
            *home = pyos::expanduser("~");
        }
        home.clone()
    })
}

/// Fold a different home in, as the tests and the bench do.
#[allow(dead_code)]
pub fn set_home(path: &str) {
    HOME.with(|home| *home.borrow_mut() = path.to_string());
}

/// The path as a person would write it: `~` for home.
pub fn display(path: &str) -> String {
    let root = home();
    if path == root {
        return "~".to_string();
    }
    if let Some(rest) = path.strip_prefix(root.as_str()) {
        if rest.starts_with('/') {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// What a row leads with. Home is written as `~`, not as your username.
pub fn folder_name(path: &str) -> String {
    if path == home() {
        return "~".to_string();
    }
    let base = pyos::basename(path);
    if base.is_empty() {
        path.to_string()
    } else {
        base.to_string()
    }
}

/// The part of a row's path that is not its name -- shown after it, dimmed.
pub fn parent_display(path: &str) -> String {
    display(&pyos::dirname(path))
}

// -- filesystem ------------------------------------------------------------

pub type Listing = Rc<Vec<(String, bool)>>;

/// What is inside a folder, for the preview: names only, sorted, dot-files kept.
#[derive(Clone, Debug, PartialEq)]
pub struct Contents {
    pub dirs: Vec<String>,
    pub files: Vec<String>,
    pub readable: bool,
}

// Directory listings are re-read on every keystroke as the leaf changes but
// the parent does not, so each directory is read once per picker session.
//
// The preview reads the folder under the cursor, and that one read also
// answers whether the folder exists -- the question every frame asks of the
// folder Enter would use. So a folder the preview has listed is never stat-ed
// again. `None` means it is not a directory at all.
thread_local! {
    static LISTINGS: RefCell<HashMap<(String, bool), Listing>> = RefCell::new(HashMap::new());
    static CONTENTS: RefCell<HashMap<String, Option<Rc<Contents>>>> = RefCell::new(HashMap::new());
    static BRANCHES: RefCell<HashMap<String, Option<String>>> = RefCell::new(HashMap::new());
}

fn entry_is_dir(entry: &std::fs::DirEntry) -> bool {
    match entry.file_type() {
        Ok(kind) if kind.is_dir() => true,
        Ok(kind) if kind.is_symlink() => std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false),
        Ok(_) => false,
        Err(_) => std::fs::metadata(entry.path()).map(|m| m.is_dir()).unwrap_or(false),
    }
}

fn name_of(entry: &std::fs::DirEntry) -> String {
    entry.file_name().to_string_lossy().into_owned()
}

/// `os.path.isdir(path)`.
pub fn isdir(path: &str) -> bool {
    !path.is_empty() && std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

fn scan_subdirectories(directory: &str, hidden: bool) -> Vec<(String, bool)> {
    let Ok(scan) = std::fs::read_dir(if directory.is_empty() { "\0" } else { directory }) else {
        return Vec::new();
    };
    let mut entries: Vec<(String, bool)> = Vec::new();
    for entry in scan {
        let Ok(entry) = entry else { return Vec::new() };
        let name = name_of(&entry);
        if !hidden && name.starts_with('.') {
            continue;
        }
        if !entry_is_dir(&entry) {
            continue;
        }
        let git = isdir(&pyos::join(&pyos::join(directory, &name), ".git"));
        entries.push((name, git));
    }
    entries.sort_by_cached_key(|(name, _)| pystr::lower(name));
    entries
}

/// The directory's subdirectories as (name, is_git) pairs, sorted by name.
pub fn subdirectories(directory: &str, hidden: bool) -> Listing {
    let key = (directory.to_string(), hidden);
    if let Some(found) = LISTINGS.with(|cache| cache.borrow().get(&key).cloned()) {
        return found;
    }
    let result = Rc::new(scan_subdirectories(directory, hidden));
    LISTINGS.with(|cache| cache.borrow_mut().insert(key, result.clone()));
    result
}

fn scan_contents(folder: &str) -> Option<Contents> {
    let scan = match std::fs::read_dir(if folder.is_empty() { "\0" } else { folder }) {
        Ok(scan) => scan,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            // There, but not ours to read.
            return Some(Contents { dirs: Vec::new(), files: Vec::new(), readable: false });
        }
        Err(_) => return None,
    };
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    for entry in scan {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                return Some(Contents { dirs: Vec::new(), files: Vec::new(), readable: false });
            }
            Err(_) => return None,
        };
        if entry_is_dir(&entry) {
            dirs.push(name_of(&entry));
        } else {
            files.push(name_of(&entry));
        }
    }
    dirs.sort_by_cached_key(|name| pystr::lower(name));
    files.sort_by_cached_key(|name| pystr::lower(name));
    Some(Contents { dirs, files, readable: true })
}

/// Everything inside a folder, directories first, or None when it is not one.
pub fn contents(folder: &str) -> Option<Rc<Contents>> {
    if let Some(found) = CONTENTS.with(|cache| cache.borrow().get(folder).cloned()) {
        return found;
    }
    let found = scan_contents(folder).map(Rc::new);
    CONTENTS.with(|cache| cache.borrow_mut().insert(folder.to_string(), found.clone()));
    found
}

/// `open(path, encoding="utf-8", errors="replace").readline(512).strip()`.
fn read_line(path: &str) -> String {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else { return String::new() };
    let mut data = Vec::with_capacity(512);
    // 512 characters are at most 2048 bytes, whatever they decode to.
    if file.take(4096).read_to_end(&mut data).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&data);
    let mut line = String::new();
    for (count, c) in text.chars().enumerate() {
        if count == 512 {
            break;
        }
        // Text mode reads `\r` and `\r\n` as line ends too.
        if c == '\n' || c == '\r' {
            line.push('\n');
            break;
        }
        line.push(c);
    }
    pystr::strip(&line).to_string()
}

/// The branch checked out in a folder, a short hash when detached, or None.
///
/// The folder's own listing says whether `.git` is a directory (a checkout)
/// or a file (a worktree, pointing at its git dir), so the branch costs one
/// small read, or two for a worktree, and nothing for a folder that is
/// neither.
pub fn git_branch(folder: &str, listed: Option<&Contents>) -> Option<String> {
    if let Some(found) = BRANCHES.with(|cache| cache.borrow().get(folder).cloned()) {
        return found;
    }
    let owned;
    let listed = match listed {
        Some(listed) => Some(listed),
        None => {
            owned = contents(folder);
            owned.as_deref()
        }
    };
    let mut branch: Option<String> = None;
    if let Some(listed) = listed {
        let mut head = String::new();
        if listed.dirs.iter().any(|name| name == ".git") {
            head = read_line(&pyos::join(&pyos::join(folder, ".git"), "HEAD"));
        } else if listed.files.iter().any(|name| name == ".git") {
            let pointer = read_line(&pyos::join(folder, ".git"));
            if let Some(rest) = pointer.strip_prefix("gitdir:") {
                let mut gitdir = pystr::strip(rest).to_string();
                if !pyos::isabs(&gitdir) {
                    gitdir = pyos::join(folder, &gitdir);
                }
                head = read_line(&pyos::join(&gitdir, "HEAD"));
            }
        }
        if let Some(rest) = head.strip_prefix("ref:") {
            let reference = pystr::strip(rest);
            branch = Some(reference.strip_prefix("refs/heads/").unwrap_or(reference).to_string());
        } else if !head.is_empty() {
            branch = Some(head.chars().take(7).collect());
        }
    }
    BRANCHES.with(|cache| cache.borrow_mut().insert(folder.to_string(), branch.clone()));
    branch
}

/// Forget everything read off the filesystem. What the rescan key does.
pub fn clear_cache() {
    LISTINGS.with(|cache| cache.borrow_mut().clear());
    CONTENTS.with(|cache| cache.borrow_mut().clear());
    BRANCHES.with(|cache| cache.borrow_mut().clear());
    clear_resolved();
}

/// Whether a path is a folder; answered from the preview's read when it has one.
///
/// `fresh` asks the filesystem whatever has been read before, for the one
/// moment the answer has to be current: just before something is created
/// there.
pub fn is_directory(path: &str, fresh: bool) -> bool {
    if !fresh {
        if let Some(found) = CONTENTS.with(|cache| cache.borrow().get(path).map(|c| c.is_some())) {
            return found;
        }
    }
    isdir(path)
}

// -- query shape -----------------------------------------------------------

pub fn looks_like_path(query: &str) -> bool {
    let text = pystr::strip(query);
    match text.chars().next() {
        None => false,
        Some(first) => matches!(first, '/' | '~' | '.') || text.contains('/') || text.contains('$'),
    }
}

/// Split a path query into (directory to list, leaf to match, list itself).
///
/// The third value says whether the directory should also be offered as a
/// row of its own: true when the query names it outright, which is what makes
/// `~/workspace/tri/` mean "this folder, and here is what is inside it".
pub fn split_path_query(query: &str, base: Option<&str>) -> (String, String, bool) {
    let text = pystr::strip(query);
    let expanded = pyos::expanduser(&pyos::expandvars(text));
    let root = match base {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => pyos::getcwd(),
    };

    if text == "~" || text == "~/" || expanded.ends_with('/') || expanded.is_empty() || expanded == "." {
        let trimmed = expanded.trim_end_matches('/');
        let mut directory = if !trimmed.is_empty() {
            trimmed.to_string()
        } else if expanded.starts_with('/') {
            "/".to_string()
        } else {
            root.clone()
        };
        if expanded.is_empty() || expanded == "." {
            directory = root.clone();
        }
        return (absolute(&directory, &root), String::new(), true);
    }

    let mut directory = pyos::dirname(&expanded);
    let leaf = pyos::basename(&expanded).to_string();
    if directory.is_empty() {
        directory = root.clone();
    }
    (absolute(&directory, &root), leaf, false)
}

fn absolute(path: &str, base: &str) -> String {
    if pyos::isabs(path) {
        return normalize(path);
    }
    normalize(&pyos::join(base, path))
}

// -- catalog ---------------------------------------------------------------

/// Pull `(score, path)` out of `zoxide query --list --score` output.
pub fn parse_zoxide(output: &[u8], limit: usize) -> Vec<String> {
    let text = String::from_utf8_lossy(output);
    let mut scored: Vec<(f64, String)> = Vec::new();
    for line in pystr::splitlines(&text) {
        let stripped = pystr::strip(line);
        // `split(None, 1)`: the score, then the rest as written.
        let Some(cut) = stripped.find(pystr::is_space) else { continue };
        let (first, rest) = stripped.split_at(cut);
        let rest = rest.trim_start_matches(pystr::is_space);
        if first.is_empty() || rest.is_empty() {
            continue;
        }
        let Some(score) = pystr::parse_float(first) else { continue };
        scored.push((score, rest.to_string()));
    }
    // Stable, best first; equal scores keep zoxide's own order.
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().take(limit).map(|(_, path)| path).collect()
}

/// Frecent directories from zoxide, best first. Empty when it is not installed.
pub fn zoxide_folders(limit: usize) -> Vec<String> {
    let argv: Vec<String> = ["zoxide", "query", "--list", "--score"].iter().map(|s| s.to_string()).collect();
    match crate::proc::run(&argv, 1.5) {
        Ok(output) if output.success => parse_zoxide(&output.stdout, limit),
        _ => Vec::new(),
    }
}

pub fn zoxide_installed() -> bool {
    pyos::which("zoxide").is_some()
}

/// Every immediate child of every configured root that exists.
pub fn root_folders(roots: &[String], hidden: bool) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in roots {
        let directory = normalize(raw);
        if directory.is_empty() || seen.contains(&directory) || !is_directory(&directory, false) {
            continue;
        }
        for (name, is_git) in subdirectories(&directory, hidden).iter() {
            out.push((pyos::join(&directory, name), *is_git));
        }
        seen.push(directory);
    }
    out
}

struct CatalogBuilder<'a> {
    owned: &'a SpaceMap,
    out: Vec<Suggestion>,
    seen: HashMap<String, usize>,
}

impl CatalogBuilder<'_> {
    fn add(&mut self, path: &str, source: &'static str, detail: &str, is_git: bool) {
        let folder = normalize(path);
        if folder.is_empty() {
            return;
        }
        let (mut source, mut detail) = (source, detail.to_string());
        if let Some(space) = self.owned.get(&folder) {
            // A folder that already has a space is always shown as one,
            // whichever list it also happens to appear in.
            source = crate::paths::SOURCE_SPACE;
            detail = space.label.clone();
        }
        if let Some(&index) = self.seen.get(&folder) {
            let existing = &mut self.out[index];
            if rank(source) < rank(existing.source) {
                existing.source = source;
                if !detail.is_empty() {
                    existing.detail = detail;
                }
            } else if is_git && !existing.is_git {
                existing.is_git = true;
            }
            return;
        }
        let is_git = is_git || is_directory(&pyos::join(&folder, ".git"), false);
        self.seen.insert(folder.clone(), self.out.len());
        let name = folder_name(&folder);
        self.out.push(Suggestion::with(folder, name, source, detail, is_git));
    }
}

/// Assemble the resting list, best source first, one row per folder.
///
/// Order here is the order shown before anything is typed: what you picked
/// last, then what is already open, then the frecent and scanned folders.
/// Once a query exists the score leads and this is only a tie-break.
pub fn build_catalog(
    snapshot: &Value,
    roots: &[String],
    recents: &[String],
    hidden: bool,
    use_zoxide: bool,
) -> Vec<Row> {
    let owned = space_folders(snapshot);
    build_catalog_with(&owned, roots, recents, hidden, if use_zoxide { Some(zoxide_folders(400)) } else { None })
}

/// `build_catalog`, with the space map and zoxide's answer already in hand.
pub fn build_catalog_with(
    owned: &SpaceMap,
    roots: &[String],
    recents: &[String],
    hidden: bool,
    zoxide: Option<Vec<String>>,
) -> Vec<Row> {
    let mut builder = CatalogBuilder { owned, out: Vec::new(), seen: HashMap::new() };
    for path in recents {
        if is_directory(&normalize(path), false) {
            builder.add(path, SOURCE_RECENT, "", false);
        }
    }
    for (folder, space) in owned.iter() {
        builder.add(folder, SOURCE_SPACE, &space.label, false);
    }
    if let Some(zoxide) = zoxide {
        for path in zoxide {
            if is_directory(&normalize(&path), false) {
                builder.add(&path, SOURCE_ZOXIDE, "", false);
            }
        }
    }
    for (path, is_git) in root_folders(roots, hidden) {
        builder.add(&path, SOURCE_ROOT, "", is_git);
    }
    builder.out.into_iter().map(Arc::new).collect()
}

// -- matching --------------------------------------------------------------

fn sorted_unique(mut hits: Vec<usize>) -> Vec<usize> {
    hits.sort_unstable();
    hits.dedup();
    hits
}

/// Score one folder against every AND-ed term, over its name and its path.
///
/// A term may land on either field; the row is only a match when every term
/// lands somewhere. Highlights are kept per field so the list can show
/// exactly which characters were matched where.
pub fn score(suggestion: &Row, terms: &[Term]) -> Option<Scored> {
    if terms.is_empty() {
        return Some(Scored::plain(suggestion.clone()));
    }
    let name_field = suggestion.name_field();
    let shown_field = suggestion.shown_field();
    let mut total = 0i64;
    let mut name_hits: Vec<usize> = Vec::new();
    let mut path_hits: Vec<usize> = Vec::new();
    for term in terms {
        let on_name: Option<Match> = match_prepared(term, name_field);
        let on_path: Option<Match> = match_prepared(term, shown_field);
        let name_score = on_name.as_ref().map(|m| m.score + NAME_BONUS);
        let path_score = on_path.as_ref().map(|m| m.score);
        match (name_score, path_score) {
            (None, None) => return None,
            (Some(name), path) if path.is_none() || name >= path.unwrap() => {
                total += name;
                name_hits.extend(on_name.unwrap().positions);
            }
            (_, Some(path)) => {
                total += path;
                path_hits.extend(on_path.unwrap().positions);
            }
            _ => unreachable!(),
        }
    }
    Some(Scored {
        suggestion: suggestion.clone(),
        score: total,
        name_positions: sorted_unique(name_hits),
        path_positions: sorted_unique(path_hits),
    })
}

pub fn terms_of(query: &str) -> Vec<Term> {
    split_query(query).iter().map(|term| Term::new(term)).collect()
}

pub fn filter_catalog(catalog: &[Row], query: &str, limit: usize) -> Vec<Scored> {
    let terms = terms_of(query);
    if terms.is_empty() {
        return catalog.iter().take(limit).map(|item| Scored::plain(item.clone())).collect();
    }
    let mut scored: Vec<Scored> = catalog.iter().filter_map(|item| score(item, &terms)).collect();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| rank(a.suggestion.source).cmp(&rank(b.suggestion.source)))
            .then_with(|| a.suggestion.path.cmp(&b.suggestion.path))
    });
    scored.truncate(limit);
    scored
}

/// Real subdirectories matching a path query, shell-order first.
///
/// Prefix matches lead, because that is what typing a path means; a fuzzy hit
/// further in still shows up behind them rather than not at all.
pub fn complete_path(query: &str, base: &str, hidden: bool, limit: usize) -> Vec<Scored> {
    let (directory, leaf, list_itself) = split_path_query(query, Some(base));
    let mut rows: Vec<Scored> = Vec::new();

    if list_itself && is_directory(&directory, false) {
        let is_git = is_directory(&pyos::join(&directory, ".git"), false);
        let name = folder_name(&directory);
        rows.push(Scored::plain(Arc::new(Suggestion::with(
            directory.clone(),
            name,
            SOURCE_PATH,
            String::new(),
            is_git,
        ))));
    }

    let folded = pystr::lower(&leaf);
    let leaf_len = leaf.chars().count();
    let term = Term::new(&leaf);
    let mut prefixed: Vec<Scored> = Vec::new();
    let mut fuzzy: Vec<(Scored, String)> = Vec::new();
    for (name, is_git) in subdirectories(&directory, hidden).iter() {
        let suggestion = || {
            Arc::new(Suggestion::with(pyos::join(&directory, name), name.clone(), SOURCE_PATH, String::new(), *is_git))
        };
        if leaf.is_empty() {
            prefixed.push(Scored::plain(suggestion()));
            continue;
        }
        let lowered = pystr::lower(name);
        if lowered.starts_with(&folded) {
            prefixed.push(Scored {
                suggestion: suggestion(),
                score: leaf_len as i64,
                name_positions: (0..leaf_len).collect(),
                path_positions: Vec::new(),
            });
            continue;
        }
        if let Some(found) = match_prepared(&term, &Prepared::new(name)) {
            fuzzy.push((
                Scored {
                    suggestion: suggestion(),
                    score: found.score,
                    name_positions: found.positions,
                    path_positions: Vec::new(),
                },
                lowered,
            ));
        }
    }
    fuzzy.sort_by(|(a, a_key), (b, b_key)| b.score.cmp(&a.score).then_with(|| a_key.cmp(b_key)));

    rows.extend(prefixed);
    rows.extend(fuzzy.into_iter().map(|(row, _)| row));
    rows.truncate(limit);
    rows
}

pub fn suggest(query: &str, catalog: &[Row], base: &str, hidden: bool, limit: usize) -> Vec<Scored> {
    if looks_like_path(query) {
        return complete_path(query, base, hidden, limit);
    }
    filter_catalog(catalog, query, limit)
}

// -- inline completion -----------------------------------------------------

/// The tail to show after the cursor, or "" when there is nothing to add.
///
/// Only ever a literal extension of what is already typed, in the same form
/// it was typed in -- a tilde query completes to a tilde path. Anything that
/// is not a straight extension is left to the accept key, which replaces the
/// whole input instead of pretending to grow it.
pub fn completion_for(query: &str, suggestion: Option<&Suggestion>) -> String {
    let Some(suggestion) = suggestion else { return String::new() };
    if query.is_empty() {
        return String::new();
    }
    let target = accept_text(query, suggestion);
    let query_len = query.chars().count();
    if target.chars().count() <= query_len {
        return String::new();
    }
    let cut = target.char_indices().nth(query_len).map(|(index, _)| index).unwrap_or(target.len());
    if pystr::lower(&target[..cut]) != pystr::lower(query) {
        return String::new();
    }
    target[cut..].to_string()
}

/// What the input becomes when a row is accepted.
///
/// Written in the same form as the query -- tilde in, tilde out -- and always
/// ending in a separator, so accepting a folder immediately lists what is
/// inside it and another accept walks one level deeper.
pub fn accept_text(query: &str, suggestion: &Suggestion) -> String {
    let text = if prefers_tilde(query) { suggestion.shown().to_string() } else { suggestion.path.clone() };
    format!("{}/", text.trim_end_matches('/'))
}

fn prefers_tilde(query: &str) -> bool {
    let stripped = pystr::strip(query);
    if stripped.starts_with('~') {
        return true;
    }
    !stripped.starts_with('/')
}

/// The folder a typed input names, whether or not it exists.
pub fn resolve_input(query: &str, base: &str) -> String {
    let text = pystr::strip(query);
    if text.is_empty() {
        return String::new();
    }
    let expanded = pyos::expanduser(&pyos::expandvars(text));
    absolute(&expanded, base)
}
