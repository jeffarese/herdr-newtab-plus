//! Ported from the Python release's tests/test_icons.py.
#![allow(unused_imports)]

use crate::icons;
use crate::pyjson::Value;
use crate::testutil::*;
use std::sync::MutexGuard;

// -- LogoTest ------------------------------------------------------------------

#[test]
fn the_codepoints_match_herdr_radar() {
    // herdr-bar and herdr-radar read the same table, so a terminal mapped
    // for either draws these.
    assert_eq!(icons::logo_for(Some("claude")), Some('\u{e1a0}'));
    assert_eq!(icons::logo_for(Some("codex")), Some('\u{e1a1}'));
}

#[test]
fn the_names_an_agent_goes_by_find_the_same_logo() {
    let logo = |name: &str| icons::logo_for(Some(name));
    assert_eq!(logo("Claude Code"), logo("claude"));
    assert_eq!(logo("cursor-agent"), logo("cursor"));
    assert_eq!(logo("open-code"), logo("opencode"));
    assert_eq!(logo("qoder"), logo("qodercli"));
    assert_eq!(logo("codex:gpt-5"), logo("codex"));
}

#[test]
fn an_agent_the_font_does_not_know_has_no_logo() {
    assert_eq!(icons::logo_for(Some("droid")), None);
    assert_eq!(icons::logo_for(Some("")), None);
    assert_eq!(icons::logo_for(None), None);
}

#[test]
fn every_logo_is_recognised_as_one() {
    for vendor in icons::VENDORS {
        assert!(icons::is_logo(icons::logo_for(Some(vendor)).unwrap()), "{vendor}");
    }
    assert!(!icons::is_logo('a'));
    assert!(!icons::is_logo(char::from_u32(icons::FIRST + icons::VENDORS.len() as u32).unwrap()));
}

// -- DetectionTest -----------------------------------------------------------

/// The Python setUp: a fresh cache, and detection pointed at an empty font
/// folder with fontconfig out of the picture (`icons::TEST_FONT_DIRS` stands
/// in for patching `_font_dirs` and `subprocess.run`). Undone on drop.
struct Detection {
    fonts: TempDir,
    _lock: MutexGuard<'static, ()>,
}

impl Detection {
    fn new() -> Self {
        let lock = env_lock();
        icons::clear_cache();
        let fonts = TempDir::new();
        *icons::TEST_FONT_DIRS.lock().unwrap() = Some(vec![fonts.path().to_string()]);
        Detection { fonts, _lock: lock }
    }

    fn add(&self, name: &str) {
        self.fonts.touch(name, "");
    }
}

impl Drop for Detection {
    fn drop(&mut self) {
        *icons::TEST_FONT_DIRS.lock().unwrap_or_else(|p| p.into_inner()) = None;
        icons::clear_cache();
    }
}

#[test]
fn the_font_file_is_enough() {
    let fonts = Detection::new();
    assert!(!icons::detect());
    fonts.add("HerdrAgentIconsMax-7526f2a2.ttf");
    assert!(icons::detect());
}

#[test]
fn another_font_is_not_mistaken_for_it() {
    let fonts = Detection::new();
    fonts.add("HerdrAgentIcons.otf");
    assert!(!icons::detect());
}

#[test]
fn auto_follows_the_font_and_the_answer_is_kept() {
    let fonts = Detection::new();
    assert!(!icons::enabled("auto"));
    fonts.add("HerdrAgentIconsMax-Regular.ttf");
    assert!(!icons::enabled("auto")); // asked once per popup
    icons::clear_cache();
    assert!(icons::enabled("auto"));
}

#[test]
fn font_and_none_do_not_look() {
    // Python made `detect` raise. Here the font is there while these are
    // asked; had any of them looked, the cache would hold that answer and
    // `auto` would still say yes once the font is gone.
    let fonts = Detection::new();
    fonts.add("HerdrAgentIconsMax-Regular.ttf");
    assert!(icons::enabled("font"));
    assert!(!icons::enabled("none"));
    assert!(!icons::enabled("nonsense"));
    std::fs::remove_file(fonts.fonts.join("HerdrAgentIconsMax-Regular.ttf")).unwrap();
    assert!(!icons::enabled("auto"));
}

#[test]
fn a_prefetch_gives_the_same_answer() {
    let fonts = Detection::new();
    fonts.add("HerdrAgentIconsMax-Regular.ttf");
    icons::prefetch();
    assert!(icons::font_available());
}

// -- ConfigTest --------------------------------------------------------------

#[test]
fn it_defaults_to_auto() {
    assert_eq!(crate::config::Config::default().agent_icons, "auto");
}

#[test]
fn booleans_mean_always_and_never() {
    assert_eq!(config(r#"{"agent_icons": true}"#).agent_icons, "font");
    assert_eq!(config(r#"{"agent_icons": false}"#).agent_icons, "none");
}

#[test]
fn anything_else_falls_back_to_auto() {
    assert_eq!(config(r#"{"agent_icons": "sometimes"}"#).agent_icons, "auto");
    assert_eq!(config(r#"{"agent_icons": "none"}"#).agent_icons, "none");
}
