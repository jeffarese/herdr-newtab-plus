//! Helpers shared by the unit tests: an environment lock, temp directories,
//! and the fake session the Python release's `tests/fixtures.py` built.
#![allow(dead_code)]

use crate::pyjson::{Map, Value};
use std::ffi::{OsStr, OsString};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

static ENV: Mutex<()> = Mutex::new(());
static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Hold this for the whole of a test that reads or changes environment
/// variables, or the process-wide caches, so tests that run in parallel do not
/// see each other's values.
pub fn env_lock() -> MutexGuard<'static, ()> {
    ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Sets environment variables and puts the old values back on drop.
/// Hold `env_lock()` for as long as this lives.
#[derive(Default)]
pub struct EnvVars {
    saved: Vec<(String, Option<OsString>)>,
}

impl EnvVars {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: &str, value: impl AsRef<OsStr>) -> &mut Self {
        self.save(key);
        std::env::set_var(key, value);
        self
    }

    pub fn remove(&mut self, key: &str) -> &mut Self {
        self.save(key);
        std::env::remove_var(key);
        self
    }

    fn save(&mut self, key: &str) {
        if !self.saved.iter().any(|(k, _)| k == key) {
            self.saved.push((key.to_string(), std::env::var_os(key)));
        }
    }
}

impl Drop for EnvVars {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..).rev() {
            match value {
                Some(value) => std::env::set_var(&key, value),
                None => std::env::remove_var(&key),
            }
        }
    }
}

/// A throwaway directory tree, removed on drop. Its path has its symlinks
/// followed already, as the Python tests' `Tree` did, so macOS's `/var` ->
/// `/private/var` never makes two spellings of one folder.
pub struct TempDir(String);

impl TempDir {
    pub fn new() -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("newtab-test-{}-{}", std::process::id(), n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        let real = std::fs::canonicalize(&path).expect("canonical temp dir");
        TempDir(real.to_string_lossy().into_owned())
    }

    /// A tree with these folders made under it, parents included.
    pub fn with(names: &[&str]) -> Self {
        let dir = TempDir::new();
        for name in names {
            std::fs::create_dir_all(dir.join(name)).expect("create folder");
        }
        dir
    }

    pub fn path(&self) -> &str {
        &self.0
    }

    pub fn join(&self, name: &str) -> String {
        format!("{}/{}", self.0, name)
    }

    pub fn touch(&self, name: &str, text: &str) {
        std::fs::write(self.join(name), text).expect("write file");
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A short, unused Unix socket path. macOS limits socket paths to about 104
/// bytes, and its temp directory alone uses half of that.
pub fn socket_path() -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = format!("/tmp/ntp-{}-{}.sock", std::process::id(), n);
    let _ = std::fs::remove_file(&path);
    path
}

fn dict(pairs: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key, value);
    }
    Value::Dict(map)
}

/// `tests/fixtures.py`'s `pane()`.
pub fn pane(pane_id: &str, cwd: &str, workspace_id: &str) -> Value {
    dict(vec![
        ("pane_id", Value::str(pane_id)),
        ("workspace_id", Value::str(workspace_id)),
        ("cwd", Value::str(cwd)),
    ])
}

/// `tests/fixtures.py`'s default single space, Atlas.
pub fn atlas() -> Vec<Value> {
    vec![dict(vec![
        ("workspace_id", Value::str("w1")),
        ("label", Value::str("Atlas")),
        ("number", Value::Int(1)),
        ("focused", Value::Bool(true)),
    ])]
}

/// `tests/fixtures.py`'s `snapshot()`.
pub fn snapshot(panes: Vec<Value>, workspaces: Option<Vec<Value>>) -> Value {
    let focused = panes
        .first()
        .map(|pane| pane.get("pane_id").clone())
        .unwrap_or(Value::Null);
    dict(vec![
        ("version", Value::str("0.7.5")),
        ("focused_workspace_id", Value::str("w1")),
        ("focused_pane_id", focused),
        ("workspaces", Value::List(workspaces.unwrap_or_else(atlas))),
        ("panes", Value::List(panes)),
        ("agents", Value::List(Vec::new())),
    ])
}

/// A space record.
pub fn space(workspace_id: &str, label: &str, number: i128, focused: bool) -> Value {
    dict(vec![
        ("workspace_id", Value::str(workspace_id)),
        ("label", Value::str(label)),
        ("number", Value::Int(number)),
        ("focused", Value::Bool(focused)),
    ])
}

/// Set a key on a dict value in place.
pub fn set(value: &mut Value, key: &str, item: Value) {
    if let Some(map) = value.as_dict_mut() {
        map.insert(key, item);
    }
}

/// Parse a JSON literal, for tests that read better written as JSON.
pub fn json(text: &str) -> Value {
    crate::pyjson::loads(text).expect("test JSON parses")
}

// -- the picker fixtures, as tests/fixtures.py had them ----------------------

use crate::agents::Agent;
use crate::app::Picker;
use crate::config::Config;
use crate::paths::{Row, Suggestion};
use crate::state::Recents;
use crate::theme::Theme;
use std::sync::Arc;

/// `fixtures.CHIPS`: a shell, then claude, codex and pi.
pub fn chips() -> Vec<Agent> {
    vec![Agent::new("", "shell"), Agent::new("claude", "claude"), Agent::new("codex", "codex"), Agent::new("pi", "pi")]
}

/// `Suggestion(path, name, source)` as a catalog row.
pub fn suggestion(path: &str, name: &str, source: &'static str) -> Row {
    Arc::new(Suggestion::with(path.to_string(), name.to_string(), source, String::new(), false))
}

/// `Suggestion(path, name, source, detail, is_git)` as a catalog row.
pub fn suggestion_full(path: &str, name: &str, source: &'static str, detail: &str, is_git: bool) -> Row {
    Arc::new(Suggestion::with(path.to_string(), name.to_string(), source, detail.to_string(), is_git))
}

/// `Config(values)` from a JSON object literal.
pub fn config(text: &str) -> Config {
    Config::from_values(json(text).as_dict().expect("a JSON object"))
}

/// `fixtures.picker()`: every argument optional, as the Python had it.
pub struct PickerSpec {
    pub catalog: Vec<Row>,
    pub base: String,
    pub snap: Option<Value>,
    pub config: Option<Config>,
    pub recents: Option<Recents>,
    pub chips: Option<Vec<Agent>>,
    pub theme: Option<Theme>,
}

impl Default for PickerSpec {
    fn default() -> Self {
        PickerSpec {
            catalog: Vec::new(),
            base: "/tmp".to_string(),
            snap: None,
            config: None,
            recents: None,
            chips: None,
            theme: None,
        }
    }
}

impl PickerSpec {
    pub fn build(self) -> Picker {
        Picker::new(
            self.snap.unwrap_or_else(|| snapshot(vec![pane("w1:p1", "/nowhere", "w1")], None)),
            self.catalog,
            self.chips.unwrap_or_else(chips),
            self.config.unwrap_or_else(|| config(r#"{"roots": []}"#)),
            self.recents.unwrap_or_else(|| Recents::new(None)),
            self.theme.unwrap_or_else(|| Theme::new(None, None, None)),
            self.base,
        )
    }
}

/// `fixtures.picker(catalog)`.
pub fn picker(catalog: Vec<Row>) -> Picker {
    PickerSpec { catalog, ..Default::default() }.build()
}

/// A frame with its colors stripped, so text can be asserted on:
/// `re.sub(r"\x1b\[[0-9;]*m", "", "\n".join(frame))`.
pub fn plain(frame: &[String]) -> String {
    plain_text(&frame.join("\n"))
}

pub fn plain_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == '\x1b' && chars.get(index + 1) == Some(&'[') {
            let mut end = index + 2;
            while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == ';') {
                end += 1;
            }
            if chars.get(end) == Some(&'m') {
                index = end + 1;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}
