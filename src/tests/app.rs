//! Ported from the Python release's tests/test_app.py.
#![allow(unused_imports)]

use crate::agents::Agent;
use crate::app::{Outcome, Picker};
use crate::keys::MouseKind;
use crate::paths::{self, Row, SOURCE_ROOT};
use crate::session::{SPACE, TAB};
use crate::state::Recents;
use crate::testutil::*;

const SEP: &str = "/";

fn catalog(names: &[&str]) -> Vec<Row> {
    names.iter().map(|name| suggestion(&format!("/a/{}", name), name, SOURCE_ROOT)).collect()
}

fn press(app: &mut Picker, keys: &[&str]) -> Option<Outcome> {
    for key in keys {
        if let Some(outcome) = app.handle_key(key) {
            return Some(outcome);
        }
    }
    None
}

fn names(app: &Picker) -> Vec<String> {
    app.rows.iter().map(|row| row.suggestion.name.clone()).collect()
}

fn click(app: &mut Picker, x: isize, y: isize) -> Option<Outcome> {
    app.handle_mouse(MouseKind::Press, x as i128, y as i128)
}

mod typing {
    use super::*;

    fn app() -> Picker {
        picker(catalog(&["alpha", "beta"]))
    }

    #[test]
    fn typing_filters() {
        let mut app = app();
        app.insert("alp", false);
        assert_eq!(names(&app), vec!["alpha"]);
    }

    #[test]
    fn backspace_widens_again() {
        let mut app = app();
        app.insert("alp", false);
        press(&mut app, &["backspace", "backspace", "backspace"]);
        assert_eq!(app.rows.len(), 2);
    }

    #[test]
    fn control_characters_never_reach_the_query() {
        let mut app = app();
        app.insert("a\x00b", false);
        assert_eq!(app.query, "ab");
    }

    #[test]
    fn a_pasted_newline_becomes_a_space() {
        let mut app = app();
        app.insert("a\nb", true);
        assert_eq!(app.query, "a b");
    }

    #[test]
    fn delete_word_walks_back_a_path_segment() {
        let mut app = app();
        app.set_query("~/workspace/atlas/".to_string(), None);
        app.delete_word();
        assert_eq!(app.query, "~/workspace/");
        app.delete_word();
        assert_eq!(app.query, "~/");
    }

    #[test]
    fn ctrl_u_clears() {
        let mut app = app();
        app.insert("alpha", false);
        press(&mut app, &["ctrl+u"]);
        assert_eq!(app.query, "");
    }

    #[test]
    fn cursor_keys_stay_inside_the_query() {
        let mut app = app();
        app.insert("ab", false);
        press(&mut app, &["left", "left", "left"]);
        assert_eq!(app.cursor, 0);
        press(&mut app, &["end"]);
        assert_eq!(app.cursor, 2);
    }
}

mod movement {
    use super::*;

    fn app() -> Picker {
        picker(catalog(&["a", "b", "c"]))
    }

    #[test]
    fn down_moves_and_wraps() {
        let mut app = app();
        press(&mut app, &["down", "down", "down"]);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn up_wraps_backwards() {
        let mut app = app();
        press(&mut app, &["up"]);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn filtering_resets_the_selection() {
        let mut app = app();
        press(&mut app, &["down"]);
        app.insert("a", false);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn wheel_scrolls_without_wrapping() {
        let mut app = app();
        app.handle_mouse(MouseKind::WheelDown, 1, 1);
        assert_eq!(app.selected, 2);
    }
}

mod agent_chip {
    use super::*;

    fn app() -> Picker {
        picker(catalog(&["a"]))
    }

    #[test]
    fn shift_tab_cycles_backwards() {
        let mut app = app();
        press(&mut app, &["shift+tab"]);
        assert_eq!(app.agent().label, "pi");
    }

    #[test]
    fn tab_cycles_when_there_is_nothing_to_complete() {
        let mut app = app();
        let start = app.chip;
        press(&mut app, &["tab"]);
        assert_eq!(app.chip, (start + 1) % 4);
    }

    #[test]
    fn the_chip_row_wraps() {
        let mut app = app();
        for _ in 0..4 {
            press(&mut app, &["tab"]);
        }
        assert_eq!(app.chip, 0);
    }
}

mod completion {
    use super::*;

    fn setup() -> (TempDir, Picker) {
        let tree = TempDir::with(&["alpha", "alpha/inner"]);
        paths::clear_cache();
        let app = PickerSpec { base: tree.path().to_string(), ..Default::default() }.build();
        (tree, app)
    }

    #[test]
    fn tab_completes_before_it_touches_the_agent() {
        let (tree, mut app) = setup();
        let chip = app.chip;
        app.set_query(tree.join("alp"), None);
        assert!(!app.ghost().is_empty());
        press(&mut app, &["tab"]);
        assert_eq!(app.query, tree.join("alpha") + SEP);
        assert_eq!(app.chip, chip);
    }

    #[test]
    fn a_second_tab_then_moves_the_agent() {
        let (tree, mut app) = setup();
        app.set_query(tree.join("alpha") + SEP, None);
        let chip = app.chip;
        // The folder itself leads the list, so there is nothing left to extend.
        assert_eq!(app.ghost(), "");
        press(&mut app, &["tab"]);
        assert_ne!(app.chip, chip);
    }

    #[test]
    fn right_at_the_end_accepts_the_row() {
        let (tree, mut app) = setup();
        app.set_query(tree.join("alp"), None);
        press(&mut app, &["right"]);
        assert_eq!(app.query, tree.join("alpha") + SEP);
    }

    #[test]
    fn right_inside_the_query_just_moves_the_cursor() {
        let (tree, mut app) = setup();
        app.set_query(tree.join("alp"), Some(0));
        press(&mut app, &["right"]);
        assert_eq!(app.cursor, 1);
        assert_eq!(app.query, tree.join("alp"));
    }

    #[test]
    fn accepting_drills_into_the_folder() {
        let (tree, mut app) = setup();
        app.set_query(tree.join("alp"), None);
        press(&mut app, &["right"]);
        let names = names(&app);
        assert_eq!(names[0], "alpha");
        assert!(names.iter().any(|name| name == "inner"));
    }
}

/// The tree and snapshot TargetTest and CreateModeTest share: a folder with a
/// space open in it, and one without.
fn open_and_fresh() -> (TempDir, crate::pyjson::Value) {
    let tree = TempDir::with(&["open", "fresh"]);
    paths::clear_cache();
    let snap = snapshot(vec![pane("w1:p1", &tree.join("open"), "w1")], None);
    (tree, snap)
}

mod target {
    use super::*;

    fn app(tree: &TempDir, snap: &crate::pyjson::Value) -> Picker {
        PickerSpec { base: tree.path().to_string(), snap: Some(snap.clone()), ..Default::default() }.build()
    }

    #[test]
    fn a_folder_with_a_space_becomes_a_tab() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("open"), None);
        assert_eq!(app.target().unwrap().kind, TAB);
    }

    #[test]
    fn a_folder_without_one_becomes_a_space() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("fresh"), None);
        assert_eq!(app.target().unwrap().kind, SPACE);
    }

    #[test]
    fn the_target_follows_the_highlighted_row() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.path().to_string() + SEP, None);
        let mut kinds: Vec<(String, &str)> = Vec::new();
        for _ in 0..3 {
            if let Some(target) = app.target() {
                kinds.push((app.selected_row().unwrap().suggestion.name.clone(), target.kind));
            }
            app.move_by(1, true);
        }
        assert!(kinds.contains(&("open".to_string(), TAB)));
        assert!(kinds.contains(&("fresh".to_string(), SPACE)));
    }

    #[test]
    fn confirming_a_real_folder_returns_a_choice() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("fresh"), None);
        assert_eq!(press(&mut app, &["enter"]), Some(Outcome::Confirm));
        let choice = app.choice().unwrap();
        assert_eq!(choice.folder, tree.join("fresh"));
        assert_eq!(choice.target.kind, SPACE);
    }

    #[test]
    fn confirming_a_folder_that_is_not_there_refuses() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query("/no/such/folder".to_string(), None);
        assert!(app.choice().is_none());
        assert!(app.status.clone().unwrap_or_default().contains("not a folder"));
    }

    #[test]
    fn an_empty_query_refuses() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        assert!(app.choice().is_none());
    }
}

mod remembered_agent {
    use super::*;

    fn setup() -> (TempDir, Recents) {
        let tree = TempDir::with(&["one", "two"]);
        paths::clear_cache();
        let mut recents = Recents::new(None);
        recents.touch(&tree.join("one"), "codex");
        recents.touch(&tree.join("two"), "claude");
        (tree, recents)
    }

    #[test]
    fn the_chip_follows_the_highlighted_folder() {
        let (tree, recents) = setup();
        let mut app =
            PickerSpec { base: tree.path().to_string(), recents: Some(recents), ..Default::default() }.build();
        app.set_query(tree.join("one"), None);
        assert_eq!(app.agent().kind, "codex");
        app.set_query(tree.join("two"), None);
        assert_eq!(app.agent().kind, "claude");
    }

    #[test]
    fn moving_the_chips_by_hand_pins_the_choice() {
        let (tree, recents) = setup();
        let mut app =
            PickerSpec { base: tree.path().to_string(), recents: Some(recents), ..Default::default() }.build();
        app.set_query(tree.join("one"), None);
        press(&mut app, &["shift+tab"]); // always the chips, never the completion
        let pinned = app.agent().kind.clone();
        app.set_query(tree.join("two"), None);
        assert_eq!(app.agent().kind, pinned);
    }

    #[test]
    fn a_configured_default_beats_the_memory() {
        let (tree, recents) = setup();
        let mut app = PickerSpec {
            base: tree.path().to_string(),
            recents: Some(recents),
            config: Some(config(r#"{"roots": [], "default_agent": "none"}"#)),
            ..Default::default()
        }
        .build();
        app.set_query(tree.join("one"), None);
        assert!(app.agent().is_none());
    }
}

mod row_decoration {
    use super::*;

    fn line_with<'a>(lines: &[&'a str], needle: &str) -> &'a str {
        lines.iter().find(|line| line.contains(needle)).copied().expect("a line with the needle")
    }

    #[test]
    fn each_row_shows_the_agent_its_folder_was_last_opened_with() {
        let mut recents = Recents::new(None);
        recents.touch("/a/b", "codex");
        let mut app =
            PickerSpec { catalog: catalog(&["a", "b"]), recents: Some(recents), ..Default::default() }.build();
        app.theme.agent_icons = true;
        let text = plain(&app.frame(80, 20));
        let lines: Vec<&str> = text.lines().collect();
        let row_a = line_with(&lines, " a ");
        let row_b = line_with(&lines, " b ");
        assert!(!row_a.contains('\u{e1a1}'));
        assert!(row_b.contains('\u{e1a1}'));
    }

    #[test]
    fn no_memory_is_shown_when_the_memory_is_not_used() {
        let mut recents = Recents::new(None);
        recents.touch("/a/b", "codex");
        let mut app = PickerSpec {
            catalog: catalog(&["b"]),
            recents: Some(recents),
            config: Some(config(r#"{"roots": [], "default_agent": "none"}"#)),
            ..Default::default()
        }
        .build();
        assert!(!plain(&app.frame(80, 20)).contains("codex ·"));
    }

    #[test]
    fn a_folder_with_an_agent_in_it_shows_the_agents_status() {
        let mut snap = snapshot(vec![pane("w1:p1", "/a/b", "w1")], None);
        set(&mut snap, "agents", json(r#"[{"pane_id": "w1:p1", "agent": "claude", "agent_status": "blocked"}]"#));
        let mut app = PickerSpec { catalog: catalog(&["a", "b"]), snap: Some(snap), ..Default::default() }.build();
        let text = plain(&app.frame(80, 20));
        let lines: Vec<&str> = text.lines().collect();
        assert!(line_with(&lines, " b ").trim_end().ends_with('◉'));
        assert!(!line_with(&lines, " a ").contains('◉'));
    }
}

mod mouse {
    use super::*;

    fn app() -> Picker {
        let mut app = picker(catalog(&["a", "b", "c"]));
        app.frame(80, 20); // settle the row and chip geometry
        app
    }

    #[test]
    fn clicking_a_row_selects_it_then_commits() {
        let mut app = app();
        let row = app.row_top + 2 + 1; // zero-based row -> one-based y
        assert_eq!(click(&mut app, 4, row), None);
        assert_eq!(app.selected, 2);
        assert_eq!(click(&mut app, 4, row), Some(Outcome::Confirm));
    }

    #[test]
    fn clicking_a_chip_picks_that_agent() {
        let mut app = app();
        let (start, _) = app.chip_spans[2];
        let y = app.chip_row + 1;
        click(&mut app, start + 1, y);
        assert_eq!(app.chip, 2);
    }

    #[test]
    fn a_click_lands_on_the_chip_it_hits_when_the_row_is_scrolled() {
        let mut app = app();
        let mut many = vec![Agent::new("", "shell")];
        many.extend((0..12).map(|i| Agent::new(&format!("k{}", i), &format!("agent-{}", i))));
        app.chips = many;
        app.chip = 10;
        app.frame(44, 20);
        let (start, _end) = app.chip_spans[9];
        assert!(start >= 0);
        let y = app.chip_row + 1;
        click(&mut app, start + 1, y);
        assert_eq!(app.chip, 9);
        // A chip scrolled out of view cannot be clicked by accident.
        assert_eq!(app.chip_spans[0], (-1, -1));
    }

    #[test]
    fn clicking_past_the_list_does_nothing() {
        let mut app = app();
        assert_eq!(click(&mut app, 4, 19), None);
    }
}

/// ^T decides between letting the plugin choose and forcing either answer.
mod create_mode {
    use super::*;

    fn app(tree: &TempDir, snap: &crate::pyjson::Value) -> Picker {
        spec(tree, snap).build()
    }

    fn spec(tree: &TempDir, snap: &crate::pyjson::Value) -> PickerSpec {
        PickerSpec { base: tree.path().to_string(), snap: Some(snap.clone()), ..Default::default() }
    }

    #[test]
    fn it_starts_on_what_the_config_says() {
        let (tree, snap) = open_and_fresh();
        assert_eq!(app(&tree, &snap).create_mode, "smart");
        let configured = PickerSpec { config: Some(config(r#"{"create": "tab"}"#)), ..spec(&tree, &snap) }.build();
        assert_eq!(configured.create_mode, "tab");
    }

    #[test]
    fn a_remembered_mode_wins_over_the_config() {
        let (tree, snap) = open_and_fresh();
        let mut recents = Recents::new(None);
        recents.remember_create_mode("space");
        assert_eq!(PickerSpec { recents: Some(recents), ..spec(&tree, &snap) }.build().create_mode, "space");
    }

    #[test]
    fn ctrl_t_cycles_through_the_three() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        let mut seen = vec![app.create_mode];
        for _ in 0..3 {
            press(&mut app, &["ctrl+t"]);
            seen.push(app.create_mode);
        }
        assert_eq!(seen, vec!["smart", TAB, SPACE, "smart"]);
    }

    #[test]
    fn forcing_a_space_stops_reusing_the_one_the_folder_has() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("open"), None);
        assert_eq!(app.target().unwrap().kind, TAB);
        press(&mut app, &["ctrl+t", "ctrl+t"]); // smart -> tab -> space
        assert_eq!(app.target().unwrap().kind, SPACE);
        assert_eq!(app.choice().unwrap().target.kind, SPACE);
    }

    #[test]
    fn forcing_a_tab_lands_a_fresh_folder_in_the_focused_space() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("fresh"), None);
        assert_eq!(app.target().unwrap().kind, SPACE);
        press(&mut app, &["ctrl+t"]);
        let target = app.target().unwrap();
        assert_eq!(target.kind, TAB);
        assert_eq!(target.workspace_id.as_deref(), Some("w1"));
        assert_eq!(app.choice().unwrap().target.workspace_id.as_deref(), Some("w1"));
    }

    #[test]
    fn the_mode_is_handed_to_the_state_so_it_outlives_the_popup() {
        // The Python test kept its own reference to the Recents it passed in;
        // in Rust the picker owns it, so it is read back through the picker.
        let (tree, snap) = open_and_fresh();
        let mut app = PickerSpec { recents: Some(Recents::new(None)), ..spec(&tree, &snap) }.build();
        press(&mut app, &["ctrl+t"]);
        assert_eq!(app.recents.create_mode(), Some(TAB));
    }

    #[test]
    fn the_switch_moves_with_the_mode_and_the_footer_names_it() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        app.set_query(tree.join("fresh"), None);
        assert!(app.hints().contains(&("^T".to_string(), "smart".to_string())));
        assert!(plain(&app.frame(80, 20)).contains("smart   tab   space"));
        press(&mut app, &["ctrl+t"]);
        assert!(app.hints().contains(&("^T".to_string(), TAB.to_string())));
        assert!(plain(&app.frame(80, 20)).contains("smart   tab   space"));
        // A popup too narrow for the switch still says an override is on.
        assert!(plain(&app.frame(44, 20)).contains("[tab]"));
    }

    #[test]
    fn a_forced_mode_survives_moving_through_the_list() {
        let (tree, snap) = open_and_fresh();
        let mut app = app(&tree, &snap);
        press(&mut app, &["ctrl+t", "ctrl+t"]);
        app.set_query(tree.path().to_string() + SEP, None);
        app.move_by(1, true);
        assert_eq!(app.create_mode, SPACE);
        assert_eq!(app.target().unwrap().kind, SPACE);
    }
}

mod help_panel {
    use super::*;

    fn app() -> Picker {
        picker(catalog(&["alpha", "beta"]))
    }

    #[test]
    fn a_question_mark_opens_it_while_the_input_is_empty() {
        let mut app = app();
        app.insert("?", false);
        assert!(app.help);
        assert_eq!(app.query, "");
    }

    #[test]
    fn a_question_mark_is_a_path_character_once_something_is_typed() {
        let mut app = app();
        app.insert("a", false);
        app.insert("?", false);
        assert!(!app.help);
        assert_eq!(app.query, "a?");
    }

    #[test]
    fn a_pasted_question_mark_is_never_a_key() {
        let mut app = app();
        app.insert("?", true);
        assert!(!app.help);
        assert_eq!(app.query, "?");
    }

    #[test]
    fn f1_opens_it_whatever_is_in_the_input() {
        let mut app = app();
        app.set_query("~/w".to_string(), None);
        press(&mut app, &["f1"]);
        assert!(app.help);
    }

    #[test]
    fn anything_closes_it_again() {
        let mut app = app();
        for close in ["esc", "f1", "down", "enter"] {
            app.help = true;
            assert_eq!(press(&mut app, &[close]), None, "{}", close);
            assert!(!app.help, "{}", close);
        }
    }

    #[test]
    fn typing_closes_it_without_reaching_the_query() {
        let mut app = app();
        app.help = true;
        app.insert("x", false);
        assert!(!app.help);
        assert_eq!(app.query, "");
    }

    #[test]
    fn enter_behind_the_panel_does_not_create() {
        let mut app = app();
        app.help = true;
        assert_eq!(press(&mut app, &["enter"]), None);
    }

    #[test]
    fn ctrl_c_still_cancels_from_behind_it() {
        let mut app = app();
        app.help = true;
        assert_eq!(press(&mut app, &["ctrl+c"]), Some(Outcome::Cancel));
    }

    #[test]
    fn a_click_closes_it_rather_than_landing_on_a_row() {
        let mut app = app();
        app.help = true;
        assert_eq!(click(&mut app, 4, 4), None);
        assert!(!app.help);
    }

    #[test]
    fn the_panel_marks_the_mode_the_picker_is_on() {
        let mut app = app();
        press(&mut app, &["ctrl+t"]); // smart -> tab
        app.help = true;
        let text = plain(&app.frame(107, 28));
        let marked: Vec<&str> = text.split('\n').filter(|line| line.contains('▌')).collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].contains("tab"));
    }

    #[test]
    fn the_panel_explains_every_mode_and_takes_the_whole_popup() {
        let mut app = app();
        app.help = true;
        let text = plain(&app.frame(107, 28));
        for mode in ["smart", TAB, SPACE] {
            assert!(text.contains(mode), "{}", mode);
        }
        assert!(!text.contains("alpha")); // the list is gone while it is up
    }

    #[test]
    fn the_corner_names_the_key_that_actually_works() {
        let mut app = app();
        assert_eq!(app.guide(), "? help");
        app.set_query("~/w".to_string(), None);
        assert_eq!(app.guide(), "F1 help");
    }

    #[test]
    fn the_way_in_is_on_screen_in_every_state() {
        let mut app = app();
        for query in ["", "~/w"] {
            app.set_query(query.to_string(), None);
            let text = plain(&app.frame(107, 28));
            assert!(text.split('\n').next_back().unwrap().contains("help"), "{:?}", query);
        }
    }
}

mod skip_permissions {
    use super::*;

    fn setup() -> (TempDir, Picker) {
        // A real folder, so the target line says what Enter would do rather
        // than reporting a path that does not exist.
        let tree = TempDir::with(&["alpha"]);
        paths::clear_cache();
        let mut app = PickerSpec { base: tree.path().to_string(), ..Default::default() }.build();
        app.set_query(tree.join("alpha"), None);
        app.chip = 1; // claude, which has a known flag
        (tree, app)
    }

    #[test]
    fn it_starts_off() {
        let (_tree, app) = setup();
        assert!(!app.skip_permissions);
        assert!(!app.skipping());
    }

    #[test]
    fn ctrl_y_arms_and_disarms_it() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        assert!(app.skipping());
        press(&mut app, &["ctrl+y"]);
        assert!(!app.skipping());
    }

    #[test]
    fn an_agent_with_no_flag_refuses_and_says_why() {
        let (_tree, mut app) = setup();
        app.chip = 3; // pi
        press(&mut app, &["ctrl+y"]);
        assert!(!app.skip_permissions);
        assert!(app.error().contains("no skip-permissions flag for pi"));
    }

    #[test]
    fn a_plain_shell_refuses_too() {
        let (_tree, mut app) = setup();
        app.chip = 0;
        press(&mut app, &["ctrl+y"]);
        assert!(!app.skip_permissions);
        assert!(app.error().contains("plain shell"));
    }

    #[test]
    fn cycling_onto_an_agent_without_a_flag_does_not_claim_to_skip() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        app.chip = 3; // pi
        assert!(!app.skipping());
        // ...and cycling back finds the setting where it was left.
        app.chip = 1;
        assert!(app.skipping());
    }

    #[test]
    fn the_choice_carries_it() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        assert!(app.choice().unwrap().skip_permissions);
    }

    #[test]
    fn an_agent_without_a_flag_never_reaches_the_choice() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        app.chip = 3; // pi
        assert!(!app.choice().unwrap().skip_permissions);
    }

    #[test]
    fn the_chip_row_and_target_line_both_show_it() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        let frame = app.frame(80, 20).join("\n");
        assert!(frame.contains("claude!"));
        assert!(frame.contains("skip permissions"));
    }

    #[test]
    fn a_click_still_lands_on_the_marked_chip() {
        let (_tree, mut app) = setup();
        press(&mut app, &["ctrl+y"]);
        let frame = app.frame(80, 20); // measures the spans
        assert!(frame.iter().any(|line| line.contains("claude!")));
        let (start, end) = app.chip_spans[2]; // codex, after the widened claude
        let y = app.chip_row + 1;
        click(&mut app, start + 1, y);
        assert_eq!(app.agent().kind, "codex");
        assert!(start < end);
    }

    #[test]
    fn the_footer_only_offers_the_key_where_it_would_work() {
        let (_tree, mut app) = setup();
        let keys = |app: &Picker| app.hints().into_iter().map(|(key, _)| key).collect::<Vec<_>>().join(" ");
        app.chip = 3; // pi
        assert!(!keys(&app).contains("^Y"));
        app.chip = 1;
        assert!(keys(&app).contains("^Y"));
    }
}

mod cancel {
    use super::*;

    #[test]
    fn escape_and_ctrl_c_cancel() {
        for key in ["esc", "ctrl+c"] {
            assert_eq!(picker(catalog(&["a"])).handle_key(key), Some(Outcome::Cancel), "{}", key);
        }
    }
}

/// The startup can draw its first frame from the catalog's leading rows:
/// the rows on screen, then any rows to make the list's length. It hands
/// the picker the whole catalog before the first key, and that picker must
/// be the one made from the whole catalog: the same frame, rows and chip,
/// and the same answer to a key.
#[test]
fn a_picker_given_the_rest_of_its_catalog_is_the_one_made_whole() {
    let named: Vec<String> = (0..30).map(|index| format!("folder-{index:02}")).collect();
    let full = catalog(&named.iter().map(String::as_str).collect::<Vec<_>>());
    let make = |rows: Vec<Row>| {
        let mut recents = Recents::new(None);
        recents.touch("/a/folder-00", "claude");
        recents.touch("/elsewhere", "codex");
        PickerSpec {
            catalog: rows,
            config: Some(config(r#"{"roots": [], "max_suggestions": 20, "default_agent": "last"}"#)),
            recents: Some(recents),
            ..Default::default()
        }
        .build()
    };
    let mut whole = make(full.clone());
    // What is on screen, then other rows standing in for the ones below it.
    let mut leading: Vec<Row> =
        full[..crate::render::MAX_LIST as usize].iter().map(|row| std::sync::Arc::new((**row).clone())).collect();
    leading.extend(catalog(&["x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10"]));
    let mut early = make(leading);
    for (width, height) in [(100, 24), (100, 40), (60, 16)] {
        assert_eq!(early.frame(width, height), whole.frame(width, height), "{width}x{height}");
    }
    early.set_catalog(full.clone());
    assert_eq!(early.frame(100, 24), whole.frame(100, 24));
    assert_eq!((names(&early), early.selected, early.chip), (names(&whole), whole.selected, whole.chip));
    assert_eq!(whole.chip, 1, "row 0 remembers claude");
    early.insert("29", false);
    whole.insert("29", false);
    assert_eq!(names(&early), names(&whole));
    assert!(
        names(&early).contains(&"folder-29".to_string()),
        "a row the first frame did not have: {:?}",
        names(&early)
    );
}
