//! Vendor logos from the Herdr Agent Icons Max font, the ones herdr-bar uses.
//!
//! The font is herdr-radar's (MIT; the marks belong to their owners). It puts
//! one logo per agent in a private-use block starting at U+E1A0, in the order
//! of `VENDORS` -- the same table herdr-bar and herdr-radar read, so a terminal
//! set up for either already draws these.
//!
//! Nothing here installs a font or touches the terminal's configuration.
//! `auto` looks for the font file where the platform keeps per-user and
//! system fonts, and falls back to plain labels when it is not there; `font`
//! forces the logos, for a picker reached over ssh from a terminal that has
//! the font locally.
//!
//! Codepoints: https://github.com/hhdebb/herdr-radar/blob/main/tools/codepoints.toml

use crate::pystr;
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

pub const FAMILY: &str = "Herdr Agent Icons Max";
pub const FILE_PREFIX: &str = "HerdrAgentIconsMax";
pub const FIRST: u32 = 0xE1A0;

pub const VENDORS: &[&str] = &[
    "claude",
    "codex",
    "opencode",
    "omp",
    "cline",
    "mastracode",
    "kimi",
    "kilo",
    "maki",
    "pi",
    "hermes",
    "cursor",
    "copilot",
    "deepseek",
    "gemini",
    "gpt",
    "qwen",
    "grok",
    "agy",
    "kiro",
    "amp",
    "devin",
    "qodercli",
    "glm",
];

/// A logo is drawn one cell wide, but terminals let an icon glyph spill into
/// the blank cell after it -- which is what makes it legible at text size. Two
/// spaces after a logo keep the spill off whatever follows.
pub const GAP: &str = "  ";

/// The agent as the logo and colour tables know it: `Claude Code` is `claude`.
pub fn agent_kind(agent: Option<&str>) -> String {
    let lowered = pystr::lower(pystr::strip(agent.unwrap_or("")));
    let mut kind = match lowered.find(':') {
        Some(index) => lowered[..index].to_string(),
        None => lowered,
    };
    if kind.ends_with(" code") {
        kind.truncate(kind.len() - " code".len());
    }
    match kind.as_str() {
        "cursor-agent" => "cursor".to_string(),
        "open-code" => "opencode".to_string(),
        "qoder" => "qodercli".to_string(),
        _ => kind,
    }
}

pub fn logo_for(agent: Option<&str>) -> Option<char> {
    let kind = agent_kind(agent);
    VENDORS.iter().position(|vendor| *vendor == kind).and_then(|index| char::from_u32(FIRST + index as u32))
}

#[cfg(test)]
pub fn is_logo(c: char) -> bool {
    let code = c as u32;
    (FIRST..FIRST + VENDORS.len() as u32).contains(&code)
}

fn font_dirs() -> Vec<String> {
    let home = crate::pyos::expanduser("~");
    if cfg!(target_os = "macos") {
        return vec![format!("{}/Library/Fonts", home), "/Library/Fonts".to_string()];
    }
    // Only the top of the per-user folders: anything deeper, or system-wide,
    // is fontconfig's to answer.
    let data = std::env::var("XDG_DATA_HOME")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| format!("{}/.local/share", home));
    vec![format!("{}/fonts", data), format!("{}/.fonts", home)]
}

fn has_font_file(directory: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(directory) else { return false };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with(FILE_PREFIX) && name.ends_with(".ttf")
    })
}

/// Whether this machine has the font, by file first and fontconfig second.
pub fn detect() -> bool {
    #[cfg(test)]
    if let Some(directories) = TEST_FONT_DIRS.lock().unwrap_or_else(|poison| poison.into_inner()).clone() {
        // Tests look in these folders only, and never ask fontconfig, so this
        // machine's own fonts stay out of the answer.
        return directories.iter().any(|directory| has_font_file(directory));
    }
    detect_in(&font_dirs())
}

/// Where `detect` looks while a test has set it. Hold `env_lock()` around it.
#[cfg(test)]
pub(crate) static TEST_FONT_DIRS: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn detect_in(directories: &[String]) -> bool {
    if directories.iter().any(|directory| has_font_file(directory)) {
        return true;
    }
    if cfg!(target_os = "macos") {
        return false;
    }
    fc_match()
}

fn fc_match() -> bool {
    let child = std::process::Command::new("fc-match")
        .args(["--format", "%{family}", FAMILY])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn();
    let Ok(child) = child else { return false };
    let (sender, receiver) = mpsc::channel();
    let pid = child.id();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    match receiver.recv_timeout(Duration::from_secs(1)) {
        Ok(Ok(output)) => {
            let text = String::from_utf8_lossy(&output.stdout);
            output.status.success() && text.split(',').any(|part| pystr::strip(part) == FAMILY)
        }
        Ok(Err(_)) => false,
        Err(_) => {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
            false
        }
    }
}

enum State {
    Unknown,
    Looking(mpsc::Receiver<bool>),
    Known(bool),
}

static STATE: Mutex<State> = Mutex::new(State::Unknown);

/// Start looking now, so the answer is ready by the time the frame needs it.
///
/// `fc-match` is a few milliseconds on Linux and nothing it reads depends on
/// the session, so it overlaps the snapshot request instead of following it.
pub fn prefetch() {
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    if !matches!(*state, State::Unknown) {
        return;
    }
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(detect());
    });
    *state = State::Looking(receiver);
}

pub fn font_available() -> bool {
    let mut state = STATE.lock().unwrap_or_else(|poison| poison.into_inner());
    let found = match &*state {
        State::Known(found) => return *found,
        State::Looking(receiver) => match receiver.recv_timeout(Duration::from_secs(2)) {
            Ok(found) => found,
            Err(_) => detect(),
        },
        State::Unknown => detect(),
    };
    *state = State::Known(found);
    found
}

#[cfg(test)]
pub fn clear_cache() {
    *STATE.lock().unwrap_or_else(|poison| poison.into_inner()) = State::Unknown;
}

/// `auto` follows the font, `font` forces logos, anything else is text only.
pub fn enabled(mode: &str) -> bool {
    match mode {
        "font" => true,
        "auto" => font_available(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn logo(kind: &str) -> Option<char> {
        logo_for(Some(kind))
    }

    #[test]
    fn the_codepoints_match_herdr_radar() {
        assert_eq!(logo("claude"), Some('\u{E1A0}'));
        assert_eq!(logo("codex"), Some('\u{E1A1}'));
    }

    #[test]
    fn the_names_an_agent_goes_by_find_the_same_logo() {
        assert_eq!(logo("Claude Code"), logo("claude"));
        assert_eq!(logo("cursor-agent"), logo("cursor"));
        assert_eq!(logo("open-code"), logo("opencode"));
        assert_eq!(logo("qoder"), logo("qodercli"));
        assert_eq!(logo("codex:gpt-5"), logo("codex"));
    }

    #[test]
    fn an_agent_the_font_does_not_know_has_no_logo() {
        assert_eq!(logo("droid"), None);
        assert_eq!(logo(""), None);
        assert_eq!(logo_for(None), None);
    }

    #[test]
    fn every_logo_is_recognised_as_one() {
        for vendor in VENDORS {
            assert!(is_logo(logo(vendor).unwrap()), "{vendor}");
        }
        assert!(!is_logo('a'));
        assert!(!is_logo(char::from_u32(FIRST + VENDORS.len() as u32).unwrap()));
    }

    #[test]
    fn the_font_file_is_enough_and_another_is_not() {
        let dir = crate::testutil::TempDir::new();
        let path = dir.path().to_string();
        std::fs::write(format!("{}/HerdrAgentIcons.otf", path), "").unwrap();
        assert!(!has_font_file(&path));
        std::fs::write(format!("{}/HerdrAgentIconsMax-7526f2a2.ttf", path), "").unwrap();
        assert!(has_font_file(&path));
    }

    #[test]
    fn font_and_none_do_not_look() {
        assert!(enabled("font"));
        assert!(!enabled("none"));
        assert!(!enabled("nonsense"));
    }
}
