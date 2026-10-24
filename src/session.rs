//! What the running session already looks like, and what a pick should do to it.
//!
//! Herdr does not record a folder on a workspace: only panes carry a `cwd`. So
//! a space's folders are exactly the folders its panes are sitting in, which is
//! both cheaper than guessing a single canonical root and more useful -- a
//! space with a worktree checked out alongside its main tree offers both.
//!
//! That also gives the tab-versus-space decision an exact rule instead of a
//! heuristic: a folder already has a space when some pane in that space is
//! standing in it. Everything else is a new space.

use crate::pyjson::{py_eq, py_str, Map, Value};
use crate::pyos;
use crate::pystr;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

pub const TAB: &str = "tab";
pub const SPACE: &str = "space";
pub const SMART: &str = "smart";

/// What `create` can be, in the order the picker's own key cycles them: the
/// automatic decision first, then each way of overriding it.
pub const MODES: [&str; 3] = [SMART, TAB, SPACE];

/// The canonical spelling of a mode, or None when it is not one.
pub fn mode_of(value: &str) -> Option<&'static str> {
    MODES.iter().copied().find(|mode| *mode == value)
}

/// The mode after this one. An unrecognised mode behaves as `SMART` does.
pub fn next_mode(mode: &str) -> &'static str {
    let index = MODES.iter().position(|m| *m == mode).unwrap_or(0);
    MODES[(index + 1) % MODES.len()]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Space {
    pub workspace_id: String,
    pub label: String,
    pub number: i128,
    pub focused: bool,
}

/// What creating a tab for the picked folder will actually do.
#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    pub kind: &'static str, // TAB | SPACE
    pub folder: String,
    pub workspace_id: Option<String>, // the space a TAB lands in
    pub workspace_label: Option<String>,
}

fn list<'a>(snapshot: &'a Value, key: &str) -> &'a [Value] {
    snapshot.get(key).list_or_empty()
}

fn nonempty_str(value: &Value) -> Option<&str> {
    value.as_str().filter(|text| !text.is_empty())
}

pub fn spaces(snapshot: &Value) -> Vec<Space> {
    let mut out = Vec::new();
    for raw in list(snapshot, "workspaces") {
        if raw.as_dict().is_none() {
            continue;
        }
        let Some(workspace_id) = raw.get("workspace_id").as_str() else { continue };
        let label = raw.get("label");
        let number = match raw.get("number") {
            Value::Int(number) => *number,
            Value::Bool(flag) => *flag as i128,
            _ => 0,
        };
        out.push(Space {
            workspace_id: workspace_id.to_string(),
            label: if label.truthy() { py_str(label) } else { workspace_id.to_string() },
            number,
            focused: raw.get("focused").truthy(),
        });
    }
    out
}

pub fn focused_workspace_id(snapshot: &Value) -> Option<String> {
    if let Some(value) = nonempty_str(snapshot.get("focused_workspace_id")) {
        return Some(value.to_string());
    }
    let known = spaces(snapshot);
    if let Some(space) = known.iter().find(|space| space.focused) {
        return Some(space.workspace_id.clone());
    }
    known.first().map(|space| space.workspace_id.clone())
}

/// Where the focused pane is standing.
///
/// This is the folder a relative query is relative to. The picker's own
/// process runs with the plugin directory as its cwd, which would make `./`
/// mean something nobody typed it to mean.
pub fn focused_cwd(snapshot: &Value) -> Option<String> {
    let focused = snapshot.get("focused_pane_id");
    let mut fallback: Option<String> = None;
    for raw in list(snapshot, "panes") {
        if raw.as_dict().is_none() {
            continue;
        }
        let Some(cwd) = nonempty_str(raw.get("cwd")) else { continue };
        if py_eq(raw.get("pane_id"), focused) || raw.get("focused").truthy() {
            return Some(normalize(cwd));
        }
        if fallback.is_none() {
            fallback = Some(normalize(cwd));
        }
    }
    fallback
}

/// Folders mapped to the space that owns them, in the order a dict would
/// keep them: first insertion fixes the position, a later assignment only
/// replaces the value. The catalog lists spaces in this order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SpaceMap {
    entries: Vec<(String, Space)>,
    index: crate::fxhash::FxHashMap<String, usize>,
}

impl SpaceMap {
    pub fn get(&self, key: &str) -> Option<&Space> {
        self.index.get(key).map(|&at| &self.entries[at].1)
    }

    pub fn insert(&mut self, key: String, space: Space) {
        match self.index.get(&key) {
            Some(&at) => self.entries[at].1 = space,
            None => {
                self.index.insert(key.clone(), self.entries.len());
                self.entries.push((key, space));
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Space)> {
        self.entries.iter().map(|(key, space)| (key, space))
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Every folder a pane is standing in, mapped to the space that owns it.
///
/// A folder open in more than one space resolves to the focused one, so the
/// picker sends you where you were rather than to whichever space sorted
/// first.
pub fn space_folders(snapshot: &Value) -> SpaceMap {
    let by_id: HashMap<String, Space> =
        spaces(snapshot).into_iter().map(|space| (space.workspace_id.clone(), space)).collect();
    let mut found = SpaceMap::default();
    for raw in list(snapshot, "panes") {
        if raw.as_dict().is_none() {
            continue;
        }
        let Some(cwd) = nonempty_str(raw.get("cwd")) else { continue };
        let Some(space) = raw.get("workspace_id").as_str().and_then(|id| by_id.get(id)) else {
            continue;
        };
        // Indexed under both spellings so a lookup finds the space whichever
        // side of a symlink the folder was picked from. (The Python release
        // walked a set here, so which spelling came first varied by run; this
        // keeps the one as written first.)
        for key in folder_keys(cwd) {
            let replace = match found.get(&key) {
                None => true,
                Some(current) => space.focused && !current.focused,
            };
            if replace {
                found.insert(key, space.clone());
            }
        }
    }
    found
}

/// Which status a folder shows when several agents stand in it: the one that
/// wants you first, in herdr-bar's order.
pub const STATUS_ORDER: [&str; 4] = ["blocked", "done", "working", "idle"];

fn rank(status: &str) -> usize {
    STATUS_ORDER.iter().position(|s| *s == status).unwrap_or(STATUS_ORDER.len())
}

/// An agent standing in a folder, as the preview lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderAgent {
    pub kind: String,   // "claude", "codex", ... or "" when Herdr did not say
    pub name: String,   // the agent's own name when it has one, otherwise its kind
    pub status: String, // one of STATUS_ORDER, or "" when Herdr reports none
}

/// Every agent the snapshot knows of, with the folder it is in and its status.
///
/// Agents carry a pane rather than a folder, so the folder is the agent's own
/// `cwd` when the snapshot has one and its pane's otherwise. A pane that
/// reports an agent status but has no agent record of its own is counted too,
/// once. A status Herdr does not report, or reports as `none`, is "".
fn agent_records(snapshot: &Value) -> Vec<(&Value, String, String)> {
    let mut panes: Vec<(&str, &Value)> = Vec::new();
    let mut by_id: HashMap<&str, usize> = HashMap::new();
    for raw in list(snapshot, "panes") {
        if raw.as_dict().is_none() {
            continue;
        }
        if let Some(pane_id) = raw.get("pane_id").as_str() {
            // A later pane with the same id replaces the earlier one in place,
            // as a dict assignment would.
            match by_id.get(pane_id) {
                Some(&index) => panes[index].1 = raw,
                None => {
                    by_id.insert(pane_id, panes.len());
                    panes.push((pane_id, raw));
                }
            }
        }
    }

    let agents: Vec<&Value> = list(snapshot, "agents").iter().filter(|raw| raw.as_dict().is_some()).collect();
    // A pane is claimed by an agent whose pane_id equals it, and only a
    // string equals a string.
    let claimed: crate::fxhash::FxHashSet<&str> = agents.iter().filter_map(|raw| raw.get("pane_id").as_str()).collect();
    let mut records: Vec<&Value> = agents.clone();
    for (pane_id, pane) in &panes {
        if claimed.contains(pane_id) {
            continue;
        }
        if pane.get("agent_status").truthy() || pane.get("agent").truthy() {
            records.push(pane);
        }
    }

    let mut out = Vec::new();
    for raw in records {
        let status = {
            let first = raw.get("agent_status");
            if first.truthy() {
                first
            } else {
                raw.get("status")
            }
        };
        let status = status.as_str().filter(|s| STATUS_ORDER.contains(s)).unwrap_or("").to_string();
        let pane = raw
            .get("pane_id")
            .as_str()
            .and_then(|id| by_id.get(id))
            .map(|&index| panes[index].1)
            .filter(|pane| pane.truthy());
        let empty = Value::Dict(Map::new());
        let pane = pane.unwrap_or(&empty);
        let cwd = [raw.get("foreground_cwd"), raw.get("cwd"), pane.get("foreground_cwd"), pane.get("cwd")]
            .into_iter()
            .find_map(nonempty_str)
            .map(|cwd| cwd.to_string());
        if let Some(cwd) = cwd {
            out.push((raw, cwd, status));
        }
    }
    out
}

/// Both spellings of a folder a session reports, so a folder picked from
/// either side of a symlink finds what is standing in it.
fn folder_keys(cwd: &str) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    for key in [normalize(cwd), resolved(cwd)] {
        if !key.is_empty() && !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
}

/// Every folder an agent is standing in, mapped to its most urgent status.
#[cfg(test)]
pub fn folder_statuses(snapshot: &Value) -> HashMap<String, String> {
    folder_statuses_and_agents(snapshot).0
}

/// Every folder an agent is standing in, mapped to all of them, most urgent first.
#[cfg(test)]
pub fn folder_agents(snapshot: &Value) -> HashMap<String, Vec<FolderAgent>> {
    folder_statuses_and_agents(snapshot).1
}

/// `folder_statuses` and `folder_agents`, from one walk of the agents: the
/// picker wants both of every snapshot.
pub fn folder_statuses_and_agents(snapshot: &Value) -> (HashMap<String, String>, HashMap<String, Vec<FolderAgent>>) {
    let mut statuses: HashMap<String, String> = HashMap::new();
    let mut agents: HashMap<String, Vec<FolderAgent>> = HashMap::new();
    for (raw, cwd, status) in agent_records(snapshot) {
        let keys = folder_keys(&cwd);
        if !status.is_empty() {
            for key in &keys {
                let replace = match statuses.get(key) {
                    None => true,
                    Some(current) => rank(&status) < rank(current),
                };
                if replace {
                    statuses.insert(key.clone(), status.clone());
                }
            }
        }
        let kind = raw.get("agent").as_str().unwrap_or("").to_string();
        let name = nonempty_str(raw.get("name")).map(|n| n.to_string()).unwrap_or_else(|| kind.clone());
        let agent = FolderAgent { kind, name, status };
        for key in keys {
            agents.entry(key).or_default().push(agent.clone());
        }
    }
    for found in agents.values_mut() {
        found.sort_by_key(|agent| rank(&agent.status));
    }
    (statuses, agents)
}

/// Names already taken by live agents, so a new one can avoid them.
pub fn live_agent_names(snapshot: &Value) -> HashSet<String> {
    list(snapshot, "agents")
        .iter()
        .filter(|raw| raw.as_dict().is_some())
        .filter_map(|raw| nonempty_str(raw.get("name")).map(|name| name.to_string()))
        .collect()
}

/// Whether `normalize` would hand `path` back unchanged: absolute, nothing
/// to expand or strip, and nothing for normpath to fold. Every scanned root
/// row is one, and the slow path's allocations add up over thousands.
pub(crate) fn is_normal(path: &str) -> bool {
    let Some(last) = path.chars().next_back() else { return false };
    path.starts_with('/')
        && (path.len() == 1 || last != '/')
        && !pystr::is_space(last)
        && !path.contains('$')
        && !path.contains("//")
        && !path.contains("/./")
        && !path.contains("/../")
        && !path.ends_with("/.")
        && !path.ends_with("/..")
}

/// A comparable form of a path: expanded, absolute, no trailing slash.
///
/// Symlinks are left alone, because this is also the path handed to Herdr
/// when a space is created, and the folder the user picked is the one they
/// should see. Matching an existing space additionally compares `resolved`
/// forms.
pub fn normalize(path: &str) -> String {
    if is_normal(path) {
        return path.to_string();
    }
    normalize_slow(path)
}

/// `normalize` the long way, for every path.
pub(crate) fn normalize_slow(path: &str) -> String {
    let expanded = pyos::expanduser(&pyos::expandvars(pystr::strip(path)));
    if expanded.is_empty() {
        return String::new();
    }
    let absolute = pyos::abspath(&expanded);
    let trimmed = absolute.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

// Following symlinks costs one lstat per path component, and the paths asked
// about are the same on every frame: the panes in the snapshot, which cannot
// change while the picker is up, and the folder under the cursor, which comes
// back around as soon as the selection does. So each answer is worked out
// once. Same bargain as the directory listings in `paths`: the picker is modal
// and short-lived, and the rescan key clears both.
thread_local! {
    pub(crate) static RESOLVED: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Paths found to exist and not be symlinks, so that folders sharing a
    /// parent do not follow it again.
    static REAL: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// The same folder with every symlink followed.
///
/// A shell resolves its own cwd, so a pane started in `/tmp/x` reports
/// `/private/tmp/x` on macOS. Without this, picking that folder a second time
/// would not recognise the space it already has and would open a duplicate.
pub fn resolved(path: &str) -> String {
    let folder = normalize(path);
    if folder.is_empty() {
        return String::new();
    }
    if let Some(found) = RESOLVED.with(|cache| cache.borrow().get(&folder).cloned()) {
        return found;
    }
    let real = REAL.with(|known| pyos::realpath_with(&folder, &mut known.borrow_mut()));
    let trimmed = real.trim_end_matches('/');
    let found = if trimmed.is_empty() { "/".to_string() } else { trimmed.to_string() };
    RESOLVED.with(|cache| cache.borrow_mut().insert(folder, found.clone()));
    found
}

/// Forget every followed symlink, so the next question is asked afresh.
pub fn clear_resolved() {
    RESOLVED.with(|cache| cache.borrow_mut().clear());
    REAL.with(|known| known.borrow_mut().clear());
}

/// Decide whether the picked folder becomes a tab or a whole new space.
#[cfg(test)]
pub fn resolve_target(folder: &str, snapshot: &Value, mode: &str) -> Target {
    resolve_target_with(folder, snapshot, mode, &space_folders(snapshot))
}

/// `resolve_target`, against a space map worked out once by the caller.
pub fn resolve_target_with(folder: &str, snapshot: &Value, mode: &str, known: &SpaceMap) -> Target {
    let normalized = normalize(folder);
    let base = pyos::basename(&normalized);
    let label = if base.is_empty() { normalized.clone() } else { base.to_string() };

    if mode == SPACE {
        // Labelled like any other new space: the line above the hints names
        // the space Enter is about to make, whether it was inferred or
        // insisted on.
        return Target { kind: SPACE, folder: normalized, workspace_id: None, workspace_label: Some(label) };
    }

    let found = known.get(&normalized).or_else(|| known.get(&resolved(folder)));

    if mode == TAB {
        // Always a tab: in the space that already owns the folder when there
        // is one, otherwise in whichever space is focused.
        let workspace_id = match found {
            Some(space) => Some(space.workspace_id.clone()),
            None => focused_workspace_id(snapshot),
        };
        let Some(workspace_id) = workspace_id else {
            // Nothing to put a tab in yet.
            return Target { kind: SPACE, folder: normalized, workspace_id: None, workspace_label: None };
        };
        let label = match found {
            Some(space) => Some(space.label.clone()),
            None => label_of(snapshot, &workspace_id),
        };
        return Target { kind: TAB, folder: normalized, workspace_id: Some(workspace_id), workspace_label: label };
    }

    if let Some(space) = found {
        return Target {
            kind: TAB,
            folder: normalized,
            workspace_id: Some(space.workspace_id.clone()),
            workspace_label: Some(space.label.clone()),
        };
    }
    Target { kind: SPACE, folder: normalized, workspace_id: None, workspace_label: Some(label) }
}

fn label_of(snapshot: &Value, workspace_id: &str) -> Option<String> {
    spaces(snapshot).into_iter().find(|space| space.workspace_id == workspace_id).map(|space| space.label)
}

/// `re.sub(r"[^a-z0-9_-]+", "-", text)`.
fn name_strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_run = false;
    for c in text.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-' {
            out.push(c);
            in_run = false;
        } else if !in_run {
            out.push('-');
            in_run = true;
        }
    }
    out
}

fn strip_dash_underscore(text: &str) -> &str {
    text.trim_matches(|c| c == '-' || c == '_')
}

fn first_is_alpha(text: &str) -> bool {
    text.chars().next().map(pystr::is_alpha).unwrap_or(false)
}

/// A legal, unused agent name derived from the folder being opened.
///
/// Herdr requires `[a-z][a-z0-9_-]{0,31}` for an agent name.
pub fn agent_name(folder: &str, kind: &str, taken: &[String]) -> String {
    let normalized = normalize(folder);
    let mut base = strip_dash_underscore(&name_strip(&pystr::lower(pyos::basename(&normalized)))).to_string();
    if base.is_empty() || !first_is_alpha(&base) {
        base = if base.is_empty() { kind.to_string() } else { format!("{}-{}", kind, base) };
    }
    let cleaned = name_strip(&base);
    let mut base: String = strip_dash_underscore(&cleaned).chars().take(32).collect();
    if base.is_empty() {
        base = "agent".to_string();
    }
    if !first_is_alpha(&base) {
        let head: String = base.chars().take(31).collect();
        base = format!("a{}", head);
    }

    let used: HashSet<&str> = taken.iter().map(|name| name.as_str()).collect();
    if !used.contains(base.as_str()) {
        return base;
    }
    for suffix in 2..100 {
        let tail = format!("-{}", suffix);
        let head: String = base.chars().take(32 - tail.len()).collect();
        let candidate = head + &tail;
        if !used.contains(candidate.as_str()) {
            return candidate;
        }
    }
    base.chars().take(32).collect()
}

/// Pull (tab_id, root pane id) out of a tab.create or workspace.create reply.
pub fn created_ids(result: &Value) -> (Option<String>, Option<String>) {
    let tab = result.get("tab");
    let tab_id = if tab.as_dict().is_some() { tab.get("tab_id").as_str() } else { None };
    let pane = result.get("root_pane");
    let pane_id = if pane.as_dict().is_some() { pane.get("pane_id").as_str() } else { None };
    (tab_id.map(|s| s.to_string()), pane_id.map(|s| s.to_string()))
}
