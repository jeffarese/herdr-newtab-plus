//! User configuration.
//!
//! Read from `$HERDR_PLUGIN_CONFIG_DIR/config.json`. Unknown keys are
//! ignored, and a broken file falls back to defaults instead of taking the
//! picker down. Values keep their JSON type where the Python release tested
//! identity (`zoxide is True`), so a config means exactly what it used to.

use crate::pyjson::{self, py_eq, Map, Value};
use crate::pystr;
use crate::session::{mode_of, SMART};

/// Scanned one level deep for folders to offer before anything is typed.
/// Only the ones that exist are used, so the default list can be generous.
pub const DEFAULT_ROOTS: [&str; 6] = ["~/workspace", "~/projects", "~/src", "~/code", "~/dev", "~/repos"];

#[derive(Clone, Debug, PartialEq)]
pub enum Agents {
    Auto,
    Listed(Vec<String>),
}

#[derive(Clone, Debug)]
pub struct Config {
    pub roots: Vec<String>,
    /// The starting point only: the picker's own toggle overrides it, and what
    /// it was left on is remembered in the state file.
    pub create: &'static str,
    pub agents: Agents,
    pub default_agent: String,
    /// kind -> argv that starts that agent with its permission prompts off,
    /// in the order the file listed them.
    pub skip_permission_flags: Vec<(String, Vec<String>)>,
    /// true | false | anything else meaning "when zoxide is installed".
    pub zoxide: Value,
    pub hidden: bool,
    pub mouse: bool,
    pub max_suggestions: usize,
    /// true | false | "auto" -- or a number equal to one of those, kept as
    /// written, as `preview in (True, False, "auto")` let through.
    pub preview: Value,
    pub agent_icons: &'static str,
    pub colors: Map,
    pub selection_background: Value,
}

/// Python's `int(value)` for whatever JSON put there; None where it raises.
fn py_int(value: &Value) -> Option<i128> {
    match value {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(*b as i128),
        Value::BigInt(text) => Some(if text.starts_with('-') { i128::MIN } else { i128::MAX }),
        Value::Float(f) if f.is_finite() => Some(f.trunc() as i128),
        Value::Str(s) => pystr::parse_int(s),
        _ => None,
    }
}

fn strings(items: &[Value]) -> Vec<String> {
    items.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect()
}

impl Default for Config {
    fn default() -> Self {
        Config::from_values(&Map::new())
    }
}

impl Config {
    pub fn from_values(values: &Map) -> Config {
        let get = |key: &str| values.get(key);

        let roots = match get("roots") {
            Some(Value::List(items)) => strings(items),
            Some(_) => DEFAULT_ROOTS.iter().map(|r| r.to_string()).collect(),
            None => DEFAULT_ROOTS.iter().map(|r| r.to_string()).collect(),
        };

        let create = get("create").and_then(|v| v.as_str()).and_then(mode_of).unwrap_or(SMART);

        let agents = match get("agents") {
            Some(Value::List(items)) => Agents::Listed(strings(items)),
            _ => Agents::Auto,
        };

        let default_agent = match get("default_agent") {
            Some(Value::Str(s)) => s.clone(),
            Some(_) => "last".to_string(),
            None => "last".to_string(),
        };

        // A malformed entry is dropped rather than repaired: the values become
        // argv for a real agent, so a half-understood one is worse than none.
        let mut skip_permission_flags: Vec<(String, Vec<String>)> = Vec::new();
        if let Some(Value::Dict(flags)) = get("skip_permission_flags") {
            for (kind, argv) in flags.iter() {
                let Value::List(argv) = argv else { continue };
                if argv.iter().all(|flag| flag.as_str().is_some()) {
                    let kept: Vec<String> = argv
                        .iter()
                        .filter_map(|f| f.as_str())
                        .filter(|f| !f.is_empty())
                        .map(|f| f.to_string())
                        .collect();
                    skip_permission_flags.push((kind.clone(), kept));
                }
            }
        }

        let zoxide = get("zoxide").cloned().unwrap_or_else(|| Value::str("auto"));
        let hidden = get("hidden").map(|v| v.truthy()).unwrap_or(false);
        let mouse = get("mouse").map(|v| v.truthy()).unwrap_or(true);

        let max_suggestions = match get("max_suggestions") {
            None => 200,
            Some(value) => py_int(value).map(|v| v.max(20)).unwrap_or(200),
        };
        let max_suggestions = usize::try_from(max_suggestions).unwrap_or(usize::MAX);

        let preview = match get("preview") {
            Some(value)
                if py_eq(value, &Value::Bool(true))
                    || py_eq(value, &Value::Bool(false))
                    || py_eq(value, &Value::str("auto")) =>
            {
                value.clone()
            }
            _ => Value::str("auto"),
        };

        let agent_icons = match get("agent_icons") {
            Some(Value::Bool(true)) => "font",
            Some(Value::Bool(false)) => "none",
            Some(Value::Str(s)) if s == "font" => "font",
            Some(Value::Str(s)) if s == "none" => "none",
            _ => "auto",
        };

        let colors = match get("colors") {
            Some(Value::Dict(map)) => map.clone(),
            _ => Map::new(),
        };
        let selection_background = get("selection_background").cloned().unwrap_or_else(|| Value::str("auto"));

        Config {
            roots,
            create,
            agents,
            default_agent,
            skip_permission_flags,
            zoxide,
            hidden,
            mouse,
            max_suggestions,
            preview,
            agent_icons,
            colors,
            selection_background,
        }
    }

    pub fn path(config_dir: Option<&str>) -> Option<String> {
        let directory = match config_dir {
            Some(directory) => directory.to_string(),
            None => std::env::var("HERDR_PLUGIN_CONFIG_DIR").unwrap_or_default(),
        };
        if directory.is_empty() {
            return None;
        }
        Some(crate::pyos::join(&directory, "config.json"))
    }

    pub fn load(config_dir: Option<&str>) -> Config {
        let Some(path) = Config::path(config_dir) else { return Config::default() };
        let Ok(data) = std::fs::read(&path) else { return Config::default() };
        // Read as UTF-8, BOM and all: Python's json refuses a BOM in text.
        let Ok(text) = String::from_utf8(data) else { return Config::default() };
        match pyjson::loads(&text) {
            Ok(Value::Dict(map)) => Config::from_values(&map),
            _ => Config::default(),
        }
    }

    pub fn wants_zoxide(&self, installed: bool) -> bool {
        if self.zoxide.is_true() {
            return true;
        }
        if self.zoxide.is_false() {
            return false;
        }
        installed
    }

    pub fn colors_has(&self, role: &str) -> bool {
        self.colors.contains_key(role)
    }
}
