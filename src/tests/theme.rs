//! Ported from the Python release's tests/test_theme.py.
#![allow(unused_imports)]

use crate::pyjson::{Map, Value};
use crate::testutil::*;
use crate::theme::{default_dim, default_ghost, default_muted, default_rule, Theme, DEFAULTS};

// The three tiers, brightest first. Everything the picker draws in words sits in
// one of them, so their order is the whole contract.
const TIERS: [&str; 3] = ["muted", "dim", "ghost"];

/// The 256-colour index a role resolves to.
fn level(theme: &Theme, role: &str) -> i64 {
    let fg = theme.fg(role);
    let (_, last) = fg.rsplit_once(';').expect("a 256-colour escape");
    last.trim_end_matches('m').parse().expect("a colour index")
}

fn default(role: &str) -> &'static str {
    DEFAULTS.iter().find(|(key, _)| *key == role).map(|(_, value)| *value).expect("a default")
}

fn appearance(name: &str) -> Theme {
    Theme::new(None, None, Some(name))
}

fn overrides(text: &str) -> Map {
    json(text).as_dict().expect("a JSON object").clone()
}

// -- ReadabilityTest ---------------------------------------------------------

#[test]
fn no_tier_is_left_to_the_terminals_decoration_slot() {
    // `bright_black` is the one ANSI slot themes treat as furniture: Tokyo
    // Night and its relatives set it to a near-background navy, which made
    // every label drawn in it unreadable.
    for role in TIERS {
        assert!(!["bright_black", "black", "8", "0"].contains(&default(role)), "{role}");
    }
}

#[test]
fn the_tiers_stay_in_order_on_a_dark_background() {
    let theme = appearance("dark");
    assert!(level(&theme, "muted") > level(&theme, "dim"));
    assert!(level(&theme, "dim") > level(&theme, "ghost"));
}

#[test]
fn and_the_other_way_round_on_a_light_one() {
    // Fainter means lighter here, so the same order runs the other way.
    let theme = appearance("light");
    assert!(level(&theme, "muted") < level(&theme, "dim"));
    assert!(level(&theme, "dim") < level(&theme, "ghost"));
}

#[test]
fn an_unknown_background_keeps_the_neutral_defaults() {
    let theme = Theme::new(None, None, None);
    for role in TIERS {
        assert_eq!(theme.fg(role), Theme::new(None, None, None).fg(role), "{role}");
    }
    assert_eq!(default_muted(None), None);
    assert_eq!(default_dim(None), None);
    assert_eq!(default_ghost(None), None);
}

#[test]
fn the_config_still_wins() {
    let theme = Theme::new(Some(&overrides(r##"{"dim": "#ff0000"}"##)), None, Some("dark"));
    assert!(theme.fg("dim").contains("2;255;0;0"));
}

#[test]
fn the_rules_sit_below_every_tier_of_text() {
    let (dark, light) = (appearance("dark"), appearance("light"));
    assert!(level(&dark, "rule") < level(&dark, "ghost"));
    assert!(level(&light, "rule") > level(&light, "ghost"));
    assert_eq!(default_rule(None), None);
}

// -- AgentColourTest ---------------------------------------------------------

#[test]
fn an_agent_is_drawn_in_its_vendors_colour() {
    let theme = Theme::new(None, None, None);
    assert_eq!(theme.agent_role(Some("claude")), "agent_claude");
    assert_eq!(theme.agent_role(Some("Claude Code")), "agent_claude");
    assert_eq!(theme.agent_role(Some("cursor-agent")), "agent_cursor");
    assert!(theme.fg("agent_claude").contains("2;217;119;87"));
}

#[test]
fn a_monochrome_mark_takes_the_text_colour() {
    let theme = Theme::new(None, None, None);
    assert_eq!(theme.agent_role(Some("pi")), "agent_pi");
    assert_eq!(theme.fg("agent_pi"), "");
}

#[test]
fn an_agent_without_a_colour_falls_back_to_the_agent_role() {
    assert_eq!(Theme::new(None, None, None).agent_role(Some("droid")), "agent");
    assert_eq!(Theme::new(None, None, None).agent_role(None), "agent");
}

#[test]
fn the_config_can_recolour_an_agent() {
    let theme = Theme::new(Some(&overrides(r##"{"agent_codex": "#ff00ff"}"##)), None, None);
    assert!(theme.fg(&theme.agent_role(Some("codex"))).contains("2;255;0;255"));
}

#[test]
fn logos_only_appear_once_they_are_switched_on() {
    let mut theme = Theme::new(None, None, None);
    assert_eq!(theme.logo(Some("claude")), "");
    theme.agent_icons = true;
    assert_eq!(theme.logo(Some("claude")), "\u{e1a0}");
    assert_eq!(theme.logo(Some("droid")), "");
}
