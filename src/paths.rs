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
use crate::fxhash::FxHashMap;
#[cfg(test)]
use crate::pyjson::Value;
use crate::pyos;
use crate::pystr;
#[cfg(test)]
use crate::session::space_folders;
use crate::session::{clear_resolved, normalize, SpaceMap};
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

    #[cfg(test)]
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
    with_home(|home| home.to_string())
}

/// `f(home())`, without copying it.
pub fn with_home<R>(f: impl FnOnce(&str) -> R) -> R {
    HOME.with(|home| {
        if home.borrow().is_empty() {
            *home.borrow_mut() = pyos::expanduser("~");
        }
        f(&home.borrow())
    })
}

/// Fold a different home in, as the tests and the bench do.
#[allow(dead_code)]
pub fn set_home(path: &str) {
    HOME.with(|home| *home.borrow_mut() = path.to_string());
}

/// The path as a person would write it: `~` for home.
pub fn display(path: &str) -> String {
    with_home(|root| {
        if path == root {
            return "~".to_string();
        }
        if let Some(rest) = path.strip_prefix(root) {
            if rest.starts_with('/') {
                return format!("~{}", rest);
            }
        }
        path.to_string()
    })
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
    crate::lookups::ask(|| !path.is_empty() && std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false))
}

fn scan_subdirectories(directory: &str, hidden: bool) -> Vec<(String, bool)> {
    let Ok(scan) = crate::lookups::ask(|| std::fs::read_dir(if directory.is_empty() { "\0" } else { directory }))
    else {
        return Vec::new();
    };
    let mut names: Vec<String> = Vec::new();
    for entry in scan {
        let Ok(entry) = entry else { return Vec::new() };
        let name = name_of(&entry);
        if !hidden && name.starts_with('.') {
            continue;
        }
        if !entry_is_dir(&entry) {
            continue;
        }
        names.push(name);
    }
    // One stat per folder, for its .git: the bulk of a big root's cost, and
    // independent folder to folder. Each is resolved from the open directory
    // rather than from `/`: a quarter to a half cheaper, the deeper the root.
    let gits: Vec<bool> = crate::lookups::ask_many(names.len(), || {
        let opened = std::fs::File::open(directory).ok();
        crate::par::map_chunks_from(&names, STAT_PARALLEL, |chunk| {
            chunk.iter().map(|name| git_inside(opened.as_ref(), directory, name)).collect()
        })
    });
    let mut entries: Vec<(String, bool)> = names.into_iter().zip(gits).collect();
    entries.sort_by_cached_key(|(name, _)| pystr::lower(name));
    entries
}

/// `isdir(join(directory, name, ".git"))`, asked of `dir`, the directory
/// already open, as a path relative to it. A path the whole of which would be
/// too long for the kernel is asked the long way, so it fails as it would.
fn git_inside(dir: Option<&std::fs::File>, directory: &str, name: &str) -> bool {
    use std::os::fd::AsRawFd;
    let whole = directory.len() + name.len() + "/".len() + "/.git".len();
    let relative = std::ffi::CString::new(format!("{name}/.git"));
    let (Some(dir), Ok(relative), true) = (dir, relative, whole < libc::PATH_MAX as usize) else {
        return isdir(&pyos::join(&pyos::join(directory, name), ".git"));
    };
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: a valid directory descriptor, a NUL-terminated path and room
    // for the answer, which is read only when the call says it wrote one.
    let found = unsafe { libc::fstatat(dir.as_raw_fd(), relative.as_ptr(), stat.as_mut_ptr(), 0) } == 0;
    found && (unsafe { stat.assume_init() }.st_mode & libc::S_IFMT) == libc::S_IFDIR
}

/// Below this many paths, asking the filesystem on one thread is quicker
/// than starting more. A stat costs microseconds, far more than a row's
/// scoring, so the bar is lower than `par::MIN_PARALLEL`.
pub(crate) const STAT_PARALLEL: usize = 64;

/// Every root's listing, read on whichever thread calls it, for
/// `seed_listings` to hand to the picker's thread. The startup reads them
/// while Herdr answers the snapshot request.
pub fn prefetch_roots(roots: &[String], hidden: bool) -> Vec<(String, Vec<(String, bool)>)> {
    let mut out: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    for raw in roots {
        let directory = normalize(raw);
        if directory.is_empty() || out.iter().any(|(known, _)| *known == directory) || !isdir(&directory) {
            continue;
        }
        let listing = scan_subdirectories(&directory, hidden);
        out.push((directory, listing));
    }
    out
}

/// `isdir` of every path, normalised, and of the `.git` inside it: what the
/// catalog asks of a recent, open or frecent folder before it becomes a row.
/// Asked on all cores, on whichever thread calls it.
pub fn ask_dirs(paths: &[String]) -> FxHashMap<String, bool> {
    let mut wanted: Vec<String> = Vec::with_capacity(paths.len() * 2);
    for path in paths {
        let folder = normalize(path);
        if folder.is_empty() {
            continue;
        }
        wanted.push(pyos::join(&folder, ".git"));
        wanted.push(folder);
    }
    ask_all(wanted)
}

/// `isdir` of the `.git` inside every path, normalised: all the catalog
/// asks of a folder that has a space, which is a row whether or not it is
/// still there. Asked on all cores, on whichever thread calls it.
pub fn ask_gits(paths: &[String]) -> FxHashMap<String, bool> {
    let wanted = paths.iter().map(|path| normalize(path)).filter(|folder| !folder.is_empty());
    ask_all(wanted.map(|folder| pyos::join(&folder, ".git")).collect())
}

/// `isdir` of every path in `wanted`, once each.
fn ask_all(mut wanted: Vec<String>) -> FxHashMap<String, bool> {
    wanted.sort_unstable();
    wanted.dedup();
    let answers: Vec<bool> = crate::lookups::ask_many(wanted.len(), || {
        crate::par::map_chunks_from(&wanted, STAT_PARALLEL, |chunk| chunk.iter().map(|path| isdir(path)).collect())
    });
    wanted.into_iter().zip(answers).collect()
}

/// Cache listings read by `prefetch_roots`, as `subdirectories` would have.
pub fn seed_listings(listings: Vec<(String, Vec<(String, bool)>)>, hidden: bool) {
    LISTINGS.with(|cache| {
        let mut cache = cache.borrow_mut();
        for (directory, listing) in listings {
            cache.entry((directory, hidden)).or_insert_with(|| Rc::new(listing));
        }
    });
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
    let scan = match crate::lookups::ask(|| std::fs::read_dir(if folder.is_empty() { "\0" } else { folder })) {
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
    let Ok(file) = crate::lookups::ask(|| std::fs::File::open(path)) else { return String::new() };
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

/// The catalog, built in the order it is shown: recents, open spaces,
/// zoxide, roots. The startup builds it step by step, so that the root rows
/// are made while zoxide is still answering and the first frame can be
/// drawn before it is in; `build_catalog_with` is the same steps in one go.
pub struct CatalogBuilder<'a> {
    owned: &'a SpaceMap,
    /// Rows are shared once built, but only the builder holds them until
    /// it is done, so a duplicate can still update the row it repeats.
    out: Vec<Row>,
    seen: FxHashMap<String, usize>,
    /// `isdir` answers asked ahead of time, on all cores.
    asked: FxHashMap<String, bool>,
}

/// A root's rows, made ahead of the rows before them in the catalog: all of
/// a row's work except checking it against those, which needs zoxide's
/// answer. A listing with a path that is not already normal (a folder named
/// `$HOME`, say) could normalise onto a sibling, so it keeps the plain
/// listing and is added row by row, exactly as before.
pub struct RootRows {
    rows: Vec<Row>,
    listed: Option<Vec<(String, bool)>>,
}

impl<'a> CatalogBuilder<'a> {
    pub fn new(owned: &'a SpaceMap, asked: FxHashMap<String, bool>) -> CatalogBuilder<'a> {
        CatalogBuilder { owned, out: Vec::new(), seen: FxHashMap::default(), asked }
    }

    /// Folders picked recently, then every folder that has a space.
    pub fn add_recents_and_spaces(&mut self, recents: &[String]) {
        self.ask_ahead(recents);
        let owned: Vec<String> = self.owned.iter().map(|(folder, _)| folder.clone()).collect();
        self.ask_gits_ahead(&owned);
        for path in recents {
            if self.is_directory(&normalize(path)) {
                self.add(path, SOURCE_RECENT, "", false, false);
            }
        }
        for (folder, space) in self.owned.iter() {
            self.add(folder, SOURCE_SPACE, &space.label, false, false);
        }
    }

    /// zoxide's folders, with the `ask_dirs` answers asked alongside them.
    pub fn add_zoxide(&mut self, zoxide: Option<Vec<String>>, asked: FxHashMap<String, bool>) {
        self.asked.extend(asked);
        let Some(zoxide) = zoxide else { return };
        self.ask_ahead(&zoxide);
        for path in zoxide {
            if self.is_directory(&normalize(&path)) {
                self.add(&path, SOURCE_ZOXIDE, "", false, false);
            }
        }
    }

    /// Every child of the roots, made as rows the way `add_listed` would.
    pub fn prepare_roots(&self, roots: &[String], hidden: bool) -> RootRows {
        let listed = root_folders(roots, hidden);
        if !listed.iter().all(|(path, _)| crate::session::is_normal(path)) {
            return RootRows { rows: Vec::new(), listed: Some(listed) };
        }
        let rows = listed
            .into_iter()
            .map(|(path, is_git)| {
                let (source, detail) = match self.owned.get(&path) {
                    Some(space) => (SOURCE_SPACE, space.label.clone()),
                    None => (SOURCE_ROOT, String::new()),
                };
                let name = pyos::basename(&path);
                let name = if name.is_empty() || with_home(|home| home == path) {
                    folder_name(&path)
                } else {
                    name.to_string()
                };
                Arc::new(Suggestion::with(path, name, source, detail, is_git))
            })
            .collect();
        RootRows { rows, listed: None }
    }

    /// `prepare_roots`, always the row-by-row way, to hold the other to.
    #[cfg(test)]
    pub fn prepare_roots_row_by_row(&self, roots: &[String], hidden: bool) -> RootRows {
        RootRows { rows: Vec::new(), listed: Some(root_folders(roots, hidden)) }
    }

    /// The root rows, last. Their paths are distinct (one listing per root,
    /// one name per entry), so each is checked only against the rows before
    /// it, and nothing is added after them.
    pub fn add_roots(&mut self, prepared: RootRows) {
        if let Some(listed) = prepared.listed {
            for (path, is_git) in listed {
                if crate::session::is_normal(&path) {
                    self.add_listed(path, is_git);
                } else {
                    self.add(&path, SOURCE_ROOT, "", is_git, true);
                }
            }
            return;
        }
        for row in prepared.rows {
            match self.seen.get(&row.path) {
                Some(&index) => self.merge(index, row.source, &row.detail, row.is_git),
                None => self.out.push(row),
            }
        }
    }

    /// The rows a first frame can be drawn from before zoxide is in, copied:
    /// the first `visible` rows of the catalog, and enough rows after them to
    /// make `limit`, so that what the frame shows of the rest -- the count,
    /// the scrollbar -- is what the whole catalog's would be. None when that
    /// is not yet known.
    ///
    /// The rows so far lead the catalog, and nothing added after them comes
    /// before them. A later repeat of one changes its source only when it is
    /// a better one, which none after the recents and spaces is, and marks it
    /// a checkout only when it is one, which it was already asked. zoxide's
    /// rows come next and the roots' last, and zoxide only ever adds rows, so
    /// the roots' rows that are not repeats, when they are in, count towards
    /// `limit` even though they are not the rows that will follow.
    pub fn leading_rows(&self, roots: Option<&RootRows>, visible: usize, limit: usize) -> Option<Vec<Row>> {
        if self.out.len() < visible.min(limit) {
            return None;
        }
        let listed = roots.map(|roots| roots.rows.as_slice()).unwrap_or_default();
        let fresh = listed.iter().filter(|row| !self.seen.contains_key(&row.path));
        let rows: Vec<Row> =
            self.out.iter().chain(fresh).take(limit).map(|row| Arc::new(Suggestion::clone(row))).collect();
        (rows.len() == limit).then_some(rows)
    }

    pub fn finish(self) -> Vec<Row> {
        self.out
    }

    /// A folder seen again: the better source wins, and a checkout stays one.
    fn merge(&mut self, index: usize, source: &'static str, detail: &str, is_git: bool) {
        let existing = Arc::get_mut(&mut self.out[index]).expect("only the builder holds a row");
        if rank(source) < rank(existing.source) {
            existing.source = source;
            if !detail.is_empty() {
                existing.detail = detail.to_string();
            }
        } else if is_git && !existing.is_git {
            existing.is_git = true;
        }
    }

    /// `is_directory(path, false)`, from the answers asked ahead when it can.
    fn is_directory(&self, path: &str) -> bool {
        if let Some(found) = CONTENTS.with(|cache| cache.borrow().get(path).map(|c| c.is_some())) {
            return found;
        }
        self.asked.get(path).copied().unwrap_or_else(|| isdir(path))
    }

    /// Ask, all at once, whatever `ask_dirs` would that is not known yet.
    fn ask_ahead(&mut self, paths: &[String]) {
        let asked = &self.asked;
        let missing: Vec<String> = paths
            .iter()
            .filter(|path| {
                let folder = normalize(path);
                !folder.is_empty() && !asked.contains_key(&folder)
            })
            .cloned()
            .collect();
        self.asked.extend(ask_dirs(&missing));
    }

    /// Ask, all at once, whatever `ask_gits` would that is not known yet.
    fn ask_gits_ahead(&mut self, paths: &[String]) {
        let asked = &self.asked;
        let missing: Vec<String> = paths
            .iter()
            .filter(|path| {
                let folder = normalize(path);
                !folder.is_empty() && !asked.contains_key(&pyos::join(&folder, ".git"))
            })
            .cloned()
            .collect();
        self.asked.extend(ask_gits(&missing));
    }

    /// `git_known` says `is_git` already answers whether `path/.git` is a
    /// directory, as a root's listing does, so it is not asked twice.
    fn add(&mut self, path: &str, source: &'static str, detail: &str, is_git: bool, git_known: bool) {
        let folder = normalize(path);
        if folder.is_empty() {
            return;
        }
        let git_known = git_known && folder == path;
        self.add_folder(folder, None, source, detail, is_git, git_known);
    }

    /// A row of a root's listing, whose path is already normal, as `add`
    /// would make it but without asking again what the listing answered.
    fn add_listed(&mut self, path: String, is_git: bool) {
        let name = pyos::basename(&path).to_string();
        self.add_folder(path, Some(name), SOURCE_ROOT, "", is_git, true);
    }

    /// `name` is the folder's last component when the caller has it; it is
    /// `folder_name` unless the folder is the home directory.
    fn add_folder(
        &mut self,
        folder: String,
        name: Option<String>,
        source: &'static str,
        detail: &str,
        is_git: bool,
        git_known: bool,
    ) {
        let (mut source, mut detail) = (source, detail.to_string());
        if let Some(space) = self.owned.get(&folder) {
            // A folder that already has a space is always shown as one,
            // whichever list it also happens to appear in.
            source = crate::paths::SOURCE_SPACE;
            detail = space.label.clone();
        }
        if let Some(&index) = self.seen.get(&folder) {
            self.merge(index, source, &detail, is_git);
            return;
        }
        let is_git = is_git || (!git_known && self.is_directory(&pyos::join(&folder, ".git")));
        self.seen.insert(folder.clone(), self.out.len());
        let name = match name {
            Some(name) if !with_home(|home| home == folder) && !name.is_empty() => name,
            _ => folder_name(&folder),
        };
        self.out.push(Arc::new(Suggestion::with(folder, name, source, detail, is_git)));
    }
}

/// Assemble the resting list, best source first, one row per folder.
///
/// Order here is the order shown before anything is typed: what you picked
/// last, then what is already open, then the frecent and scanned folders.
/// Once a query exists the score leads and this is only a tie-break.
#[cfg(test)]
pub fn build_catalog(
    snapshot: &Value,
    roots: &[String],
    recents: &[String],
    hidden: bool,
    use_zoxide: bool,
) -> Vec<Row> {
    build_catalog_from(
        snapshot,
        roots,
        recents,
        hidden,
        if use_zoxide { Some(zoxide_folders(ZOXIDE_LIMIT)) } else { None },
    )
}

/// How many of zoxide's folders the catalog takes.
pub const ZOXIDE_LIMIT: usize = 400;

/// `build_catalog`, with zoxide's answer already in hand.
#[cfg(test)]
pub fn build_catalog_from(
    snapshot: &Value,
    roots: &[String],
    recents: &[String],
    hidden: bool,
    zoxide: Option<Vec<String>>,
) -> Vec<Row> {
    build_catalog_with(&space_folders(snapshot), roots, recents, hidden, zoxide, FxHashMap::default())
}

/// `build_catalog`, with the space map, zoxide's answer and some of the
/// `ask_dirs` answers already in hand: the steps `cli::catalog` takes.
#[cfg(test)]
pub fn build_catalog_with(
    owned: &SpaceMap,
    roots: &[String],
    recents: &[String],
    hidden: bool,
    zoxide: Option<Vec<String>>,
    asked: FxHashMap<String, bool>,
) -> Vec<Row> {
    let mut builder = CatalogBuilder::new(owned, asked);
    builder.add_recents_and_spaces(recents);
    let prepared = builder.prepare_roots(roots, hidden);
    builder.add_zoxide(zoxide, FxHashMap::default());
    builder.add_roots(prepared);
    builder.finish()
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
