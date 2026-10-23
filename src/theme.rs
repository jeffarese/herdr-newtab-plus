//! Colors.
//!
//! Defaults are plain ANSI colors, so the picker inherits whatever theme the
//! terminal (and therefore Herdr) already uses. Anything can be overridden with
//! a hex value from the plugin config.
//!
//! Three tiers of foreground, so a row reads in order: the folder name (text),
//! the path and badge you scan once the name has landed (muted), and the rules
//! and separators that only exist to divide them (dim).

use crate::icons::{agent_kind, logo_for};
use crate::pyjson::{self, Map, Value};
use crate::pystr;
use std::collections::HashMap;

pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const UNDERLINE: &str = "\x1b[4m";

fn named(spec: &str) -> Option<u32> {
    Some(match spec {
        "black" => 0,
        "red" => 1,
        "green" => 2,
        "yellow" => 3,
        "blue" => 4,
        "magenta" => 5,
        "cyan" => 6,
        "white" => 7,
        "bright_black" | "gray" | "grey" => 8,
        "bright_red" => 9,
        "bright_green" => 10,
        "bright_yellow" => 11,
        "bright_blue" => 12,
        "bright_magenta" => 13,
        "bright_cyan" => 14,
        "bright_white" => 15,
        _ => return None,
    })
}

pub const DEFAULTS: &[(&str, &str)] = &[
    ("accent", "bright_blue"),
    ("text", "default"),
    ("muted", "244"),
    // Grays rather than `bright_black`, which is the one ANSI slot themes
    // treat as decoration: Tokyo Night and its relatives set it to a
    // near-background navy. A gray from the cube is the same gray whatever
    // the terminal's palette says.
    ("dim", "245"),
    ("match", "bright_blue"),
    // The inline completion after the cursor: present enough to read, faint
    // enough that it never looks like something you typed.
    ("ghost", "244"),
    ("agent", "bright_magenta"),
    ("space", "bright_cyan"),
    ("error", "bright_red"),
    // An agent about to start with its permission prompts off. Deliberately
    // not the agent color: the point is that the chip stops looking ordinary.
    ("warn", "bright_yellow"),
    // The rules between the input, the list and the chrome.
    ("rule", "240"),
    // Agent status, in herdr's own sidebar colours.
    ("blocked", "bright_red"),
    ("working", "#c78a1f"),
    ("done", "bright_cyan"),
    ("idle", "bright_green"),
    // Each agent in its vendor's colour, the same table herdr-bar uses.
    ("agent_claude", "#D97757"),
    ("agent_codex", "#10A37F"),
    ("agent_kimi", "#9B8AFB"),
    ("agent_gemini", "#4285F4"),
    ("agent_cursor", "#A78BFA"),
    ("agent_opencode", "#38BDF8"),
    ("agent_pi", "default"),
    ("agent_grok", "default"),
];

/// `STATUS_GLYPHS.get(status, "")`.
pub fn status_glyph(status: &str) -> &'static str {
    match status {
        "blocked" => "◉",
        "working" => "◐",
        "done" => "●",
        "idle" => "✓",
        _ => "",
    }
}

fn hex2(text: &str) -> Option<i128> {
    pystr::parse_int_base(text, 16)
}

/// The SGR parameters for a color spec, or None for 'default'.
fn parse_spec(spec: &str) -> Option<String> {
    let spec = pystr::lower(pystr::strip(spec));
    if spec.is_empty() || matches!(spec.as_str(), "default" | "none" | "reset" | "inherit") {
        return None;
    }
    let chars: Vec<char> = spec.chars().collect();
    if spec.starts_with('#') && chars.len() == 7 {
        let part = |a: usize, b: usize| -> String { chars[a..b].iter().collect() };
        let red = hex2(&part(1, 3))?;
        let green = hex2(&part(3, 5))?;
        let blue = hex2(&part(5, 7))?;
        return Some(format!("2;{};{};{}", red, green, blue));
    }
    if let Some(code) = named(&spec) {
        return Some(format!("5;{}", code));
    }
    if pystr::str_isdigit(&spec) {
        // int() reads decimal digits only; a superscript is a digit it
        // rejects, which the Python release let escape as an error.
        if let Some(value) = pystr::parse_int(&spec) {
            if (0..=255).contains(&value) {
                return Some(format!("5;{}", value));
            }
        }
    }
    None
}

/// `_parse(value)` for any JSON value: str() first, as the Python did.
pub fn parse_value(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    parse_spec(&pyjson::py_str(value))
}

#[derive(Clone, Debug)]
pub struct Theme {
    fg: HashMap<String, String>,
    pub selection_bg: String,
    /// Vendor logos are a font question, not a colour one: the caller decides
    /// whether the font is there and says so here, once.
    pub agent_icons: bool,
}

impl Theme {
    pub fn new(overrides: Option<&Map>, selection_background: Option<&Value>, appearance: Option<&str>) -> Theme {
        let mut merged: Vec<(String, String)> = DEFAULTS.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        fn set(merged: &mut Vec<(String, String)>, key: &str, value: String) {
            if let Some(slot) = merged.iter_mut().find(|(k, _)| k == key) {
                slot.1 = value;
            } else {
                merged.push((key.to_string(), value));
            }
        }
        for (role, spec) in [
            ("muted", default_muted(appearance)),
            ("dim", default_dim(appearance)),
            ("ghost", default_ghost(appearance)),
            ("rule", default_rule(appearance)),
        ] {
            if let Some(spec) = spec {
                set(&mut merged, role, spec.to_string());
            }
        }
        if let Some(overrides) = overrides {
            for (key, value) in overrides.iter() {
                if let Value::Str(spec) = value {
                    set(&mut merged, key, spec.clone());
                }
            }
        }
        let fg = merged
            .into_iter()
            .map(|(role, spec)| (role, parse_spec(&spec).map(|p| format!("\x1b[38;{}m", p)).unwrap_or_default()))
            .collect();
        let selection = selection_background.and_then(parse_value);
        Theme { fg, selection_bg: selection.map(|p| format!("\x1b[48;{}m", p)).unwrap_or_default(), agent_icons: false }
    }

    pub fn fg(&self, role: &str) -> &str {
        self.fg.get(role).map(|s| s.as_str()).unwrap_or("")
    }

    /// The role an agent is drawn in: its vendor's colour, or `agent`.
    pub fn agent_role(&self, kind: Option<&str>) -> String {
        let role = format!("agent_{}", agent_kind(kind));
        if self.fg.contains_key(&role) {
            role
        } else {
            "agent".to_string()
        }
    }

    /// The agent's logo when logos are on and the font has one, else empty.
    pub fn logo(&self, kind: Option<&str>) -> String {
        if !self.agent_icons {
            return String::new();
        }
        logo_for(kind).map(|c| c.to_string()).unwrap_or_default()
    }

    pub fn paint(&self, role: &str, text: &str) -> String {
        self.paint_bold(role, text, false)
    }

    pub fn paint_bold(&self, role: &str, text: &str, bold: bool) -> String {
        let color = self.fg(role);
        if color.is_empty() && !bold {
            return text.to_string();
        }
        format!("{}{}{}{}", if bold { BOLD } else { "" }, color, text, RESET)
    }
}

/// The secondary foreground: legible, still clearly below the title. The
/// neutral 244 survives either background, so an unknown appearance keeps the
/// default rather than guessing.
pub fn default_muted(appearance: Option<&str>) -> Option<&'static str> {
    match appearance {
        Some("dark") => Some("249"),
        Some("light") => Some("240"),
        _ => None,
    }
}

/// The third tier: below the secondary text, still a thing you can read.
pub fn default_dim(appearance: Option<&str>) -> Option<&'static str> {
    match appearance {
        Some("dark") => Some("246"),
        Some("light") => Some("243"),
        _ => None,
    }
}

/// The inline completion: one step below `dim`, on whichever side is fainter.
pub fn default_ghost(appearance: Option<&str>) -> Option<&'static str> {
    match appearance {
        Some("dark") => Some("244"),
        Some("light") => Some("245"),
        _ => None,
    }
}

/// The rules: visible as a line, quiet enough to never be read as text.
pub fn default_rule(appearance: Option<&str>) -> Option<&'static str> {
    match appearance {
        Some("dark") => Some("239"),
        Some("light") => Some("250"),
        _ => None,
    }
}

/// A selection background that suits the terminal's own background.
pub fn default_selection_background(appearance: Option<&str>) -> Option<&'static str> {
    match appearance {
        Some("dark") => Some("237"),
        Some("light") => Some("254"),
        _ => None,
    }
}
