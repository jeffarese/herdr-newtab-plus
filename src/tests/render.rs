//! Ported from tests/test_render.py.
#![allow(unused_imports)]

use crate::testutil::*;

use crate::agents::Agent;
use crate::paint::{cells_of, style_data, CONTINUATION};
use crate::paths::{Scored, SOURCE_RECENT, SOURCE_ROOT, SOURCE_SPACE};
use crate::pyjson::Value;
use crate::render;
use crate::session::{Target, SMART, SPACE, TAB};
use crate::textutil::{display_width, strip_ansi};
use crate::theme::{Theme, UNDERLINE};

fn theme() -> Theme {
    Theme::new(None, None, None)
}

fn pills() -> Theme {
    Theme::new(None, Some(&Value::str("237")), None)
}

fn row_full(name: &str, source: &'static str, detail: &str, is_git: bool, path: Option<&str>) -> Scored {
    let path = path.map(str::to_string).unwrap_or_else(|| format!("/a/{}", name));
    Scored::plain(suggestion_full(&path, name, source, detail, is_git))
}

fn row(name: &str) -> Scored {
    row_full(name, SOURCE_ROOT, "", false, None)
}

fn row_from(name: &str, source: &'static str) -> Scored {
    row_full(name, source, "", false, None)
}

fn plain(text: &str) -> String {
    strip_ansi(text)
}

fn width(text: &str) -> isize {
    display_width(text) as isize
}

/// Python's `text.index(needle)`, counted in characters rather than bytes.
fn char_index(text: &str, needle: &str) -> usize {
    text[..text.find(needle).expect("substring present")].chars().count()
}

/// Python's `text[start:end]`, counted in characters.
fn char_slice(text: &str, start: isize, end: isize) -> String {
    let count = text.chars().count() as isize;
    let clamp = |i: isize| if i < 0 { (count + i).max(0) } else { i.min(count) };
    let (start, end) = (clamp(start), clamp(end));
    if start >= end {
        return String::new();
    }
    text.chars().skip(start as usize).take((end - start) as usize).collect()
}

/// The characters a row draws on a background: what its pills cover.
fn painted(text: &str, width: isize) -> String {
    let (cells, _) = cells_of(text, width as usize);
    cells
        .iter()
        .filter(|cell| style_data(cell.style).bg.as_deref().is_some_and(|bg| !bg.is_empty()))
        .filter(|cell| cell.ch != CONTINUATION)
        .map(|cell| cell.ch)
        .collect()
}

fn iconic() -> Theme {
    let mut theme = pills();
    theme.agent_icons = true;
    theme
}

fn crowded() -> Vec<Agent> {
    let mut many = vec![Agent::new("", "shell")];
    for i in 0..12 {
        many.push(Agent::new(&format!("k{}", i), &format!("agent-{}", i)));
    }
    many
}

fn tab_target(label: &str) -> Target {
    Target {
        kind: TAB,
        folder: "/a".to_string(),
        workspace_id: Some("w1".to_string()),
        workspace_label: Some(label.to_string()),
    }
}

fn space_target(folder: &str, label: &str) -> Target {
    Target { kind: SPACE, folder: folder.to_string(), workspace_id: None, workspace_label: Some(label.to_string()) }
}

mod layout {
    use super::*;

    #[test]
    fn a_tall_popup_shows_every_part() {
        let layout = render::compute_layout(90, 24, false);
        assert!(layout.rules && layout.chips && layout.target && layout.footer);
    }

    #[test]
    fn chrome_sheds_from_the_bottom_as_it_shrinks() {
        assert!(!render::compute_layout(90, 8, false).footer);
        assert!(!render::compute_layout(90, 6, false).target);
        assert!(!render::compute_layout(90, 4, false).chips);
    }

    #[test]
    fn the_list_never_disappears() {
        for height in 1..30 {
            assert!(render::compute_layout(90, height, false).list_height >= 1);
        }
    }

    #[test]
    fn the_list_is_capped_however_tall_the_popup_is() {
        for height in [20, 28, 60] {
            assert_eq!(render::compute_layout(105, height, false).list_height, render::MAX_LIST);
        }
    }

    #[test]
    fn the_rows_past_the_cap_become_air_rather_than_more_folders() {
        let layout = render::compute_layout(105, 28, false);
        assert!(layout.spare > 0);
        // Every row is accounted for: input, rule, list, air, rule, chips,
        // target, note, hints.
        let chrome = 2 + 2 + 1 + 1 + 1;
        assert_eq!(layout.list_height + layout.spare + chrome, 28);
    }

    #[test]
    fn the_mode_note_only_appears_when_the_rows_are_going_spare() {
        assert!(render::compute_layout(105, 28, false).note);
        assert!(!render::compute_layout(75, 12, false).note);
    }

    #[test]
    fn a_small_popup_still_gives_the_list_what_is_left() {
        let layout = render::compute_layout(75, 12, false);
        assert_eq!(layout.list_height, 6);
        assert_eq!(layout.spare, 0);
    }
}

mod rows {
    use super::*;

    fn render_row(theme: &Theme, row: &Scored, width: isize, selected: bool) -> String {
        render::render_row(theme, row, width, selected, None, None, None)
    }

    #[test]
    fn a_row_fits_its_width() {
        let text = plain(&render_row(&theme(), &row("alpha"), 60, false));
        assert!(width(&text) <= 60);
    }

    #[test]
    fn a_space_badge_names_the_space() {
        let text = plain(&render_row(&theme(), &row_full("a", SOURCE_SPACE, "Atlas", false, None), 60, false));
        assert!(text.contains("Atlas"));
    }

    #[test]
    fn a_recent_row_says_so() {
        assert!(plain(&render_row(&theme(), &row_from("a", SOURCE_RECENT), 60, false)).contains("recent"));
    }

    #[test]
    fn a_git_folder_is_badged_when_nothing_else_claims_the_slot() {
        let git = row_full("a", SOURCE_ROOT, "", true, None);
        assert!(plain(&render_row(&theme(), &git, 60, false)).contains("git"));
    }

    #[test]
    fn the_selected_row_is_marked() {
        let text = plain(&render_row(&theme(), &row("alpha"), 60, true));
        assert!(text.starts_with(render::SELECTED_BAR));
    }

    #[test]
    fn a_long_name_is_truncated_rather_than_wrapped() {
        let text = plain(&render_row(&theme(), &row(&"x".repeat(200)), 60, false));
        assert!(width(&text) <= 60);
    }

    #[test]
    fn a_narrow_row_drops_the_badge_before_the_name() {
        let text = plain(&render_row(&theme(), &row_from("alpha", SOURCE_RECENT), 30, false));
        assert!(text.contains("alpha"));
        assert!(width(&text) <= 30);
    }

    #[test]
    fn the_name_column_follows_the_rows_on_screen() {
        let narrow = render::name_column(&[row("ab"), row("cd")], 80);
        let wide = render::name_column(&[row("ab"), row("a-very-long-folder-name")], 80);
        assert!(narrow < wide);
    }
}

mod row_decoration {
    use super::*;

    #[test]
    fn the_folders_agent_leads_the_row_as_a_logo() {
        let text = plain(&render::render_row(&iconic(), &row("alpha"), 80, false, Some(12), Some("claude"), None));
        assert!(text.starts_with("  \u{e1a0}  alpha"), "{:?}", text);
        assert!(!text.contains("claude")); // the logo already says it
    }

    #[test]
    fn rows_without_an_agent_keep_the_gutter() {
        let theme = iconic();
        let bare = plain(&render::render_row(&theme, &row("alpha"), 80, false, Some(12), None, None));
        let marked = plain(&render::render_row(&theme, &row("alpha"), 80, false, Some(12), Some("codex"), None));
        assert_eq!(char_index(&bare, "alpha"), char_index(&marked, "alpha"));
    }

    #[test]
    fn without_the_font_the_agent_is_named_at_the_end() {
        let text = plain(&render::render_row(
            &theme(),
            &row_from("alpha", SOURCE_RECENT),
            80,
            false,
            Some(12),
            Some("codex"),
            None,
        ));
        assert!(text.starts_with("  alpha"));
        assert!(text.trim_end().ends_with("codex · recent"), "{:?}", text);
    }

    #[test]
    fn an_agent_standing_in_the_folder_shows_its_status() {
        let space = row_full("a", SOURCE_SPACE, "Atlas", false, None);
        let text = plain(&render::render_row(&theme(), &space, 60, false, None, None, Some("blocked")));
        assert!(text.trim_end().ends_with("◉ Atlas"), "{:?}", text);
        let working = plain(&render::render_row(&theme(), &row("a"), 60, false, None, None, Some("working")));
        assert!(working.trim_end().ends_with("◐"), "{:?}", working);
    }

    #[test]
    fn an_unreported_status_says_nothing() {
        let space = row_full("a", SOURCE_SPACE, "Atlas", false, None);
        let text = plain(&render::render_row(&theme(), &space, 60, false, None, None, Some("none")));
        assert!(text.trim_end().ends_with(" Atlas"));
        assert!(!text.contains("◉"));
    }

    #[test]
    fn a_decorated_row_still_fits_its_width() {
        let long = row_full("a-long-folder-name", SOURCE_SPACE, "A Space", false, None);
        for theme in [theme(), iconic()] {
            for w in (20..110).step_by(7) {
                for selected in [false, true] {
                    let text =
                        plain(&render::render_row(&theme, &long, w, selected, None, Some("claude"), Some("working")));
                    assert!(width(&text) <= w, "{:?}", (w, selected));
                    if selected && !theme.selection_bg.is_empty() {
                        assert_eq!(width(&text), w); // a band, edge to edge
                    }
                }
            }
        }
    }
}

mod empty_list {
    use super::*;

    #[test]
    fn it_says_what_happened_and_what_to_do() {
        let rows: Vec<String> = render::render_empty(&theme(), 80, 5, "zzz").iter().map(|line| plain(line)).collect();
        assert_eq!(rows.len(), 5);
        assert!(rows[0].contains("no folder matches"));
        assert!(rows[1].contains("⏎ still creates it"));
    }

    #[test]
    fn a_one_row_list_keeps_the_headline() {
        let rows = render::render_empty(&theme(), 80, 1, "");
        assert_eq!(rows.len(), 1);
        assert!(plain(&rows[0]).contains("no folders to suggest"));
    }
}

mod input {
    use super::*;

    #[test]
    fn an_empty_query_shows_the_placeholder() {
        assert!(plain(&render::render_input(&theme(), 70, "", 0, "")).contains("folder"));
    }

    #[test]
    fn the_ghost_follows_the_query() {
        let text = plain(&render::render_input(&theme(), 70, "~/work", 6, "space/"));
        assert!(text.contains("~/work"));
        assert!(text.contains("space/"));
    }

    #[test]
    fn a_long_query_keeps_the_cursor_on_screen() {
        let query = format!("/{}", "a".repeat(200));
        let text = plain(&render::render_input(&theme(), 40, &query, query.chars().count(), ""));
        assert!(width(&text) <= 40);
        assert!(text.trim_end().ends_with('a'));
    }
}

mod chips_tests {
    use super::*;

    #[test]
    fn every_agent_is_shown_when_they_fit() {
        let text = plain(&render::render_chips(&theme(), 90, &chips(), 1, false));
        for chip in chips() {
            assert!(text.contains(&chip.label));
        }
    }

    #[test]
    fn a_crowded_row_still_shows_the_selection() {
        let text = plain(&render::render_chips(&theme(), 44, &crowded(), 10, false));
        assert!(text.contains("agent-9")); // chips[10], the shell leading the row
        assert!(text.contains('‹'));
        assert!(text.contains('›'));
        assert!(width(&text) <= 44);
    }

    #[test]
    fn only_the_selected_chip_is_marked_as_skipping() {
        let text = plain(&render::render_chips(&theme(), 90, &chips(), 1, true));
        assert!(text.contains("claude!"));
        assert!(!text.contains("codex!"));
    }

    #[test]
    fn a_shell_is_never_marked() {
        assert!(!plain(&render::render_chips(&theme(), 90, &chips(), 0, true)).contains("shell!"));
    }

    #[test]
    fn a_crowded_row_stays_inside_its_width_when_marked() {
        let text = plain(&render::render_chips(&theme(), 44, &crowded(), 10, true));
        assert!(text.contains("agent-9!")); // chips[10], the shell leading the row
        assert!(width(&text) <= 44);
    }
}

mod chip_span {
    use super::*;

    #[test]
    fn each_span_covers_its_own_chip() {
        let all = chips();
        for theme in [theme(), iconic()] {
            let (text, spans) = render::chips_row(&theme, 90, &all, 1, true);
            let flat = plain(&text);
            for (index, (chip, &(start, end))) in all.iter().zip(spans.iter()).enumerate() {
                let label = render::chip_label(chip, index == 1, true);
                let covered = char_slice(&flat, start, end);
                assert!(covered.contains(&label), "{:?}", (chip, covered));
            }
        }
    }

    #[test]
    fn the_selected_chip_is_a_pill() {
        let (text, spans) = render::chips_row(&pills(), 90, &chips(), 2, false);
        assert_eq!(painted(&text, 90), " codex ");
        let (start, end) = spans[2];
        assert_eq!(char_slice(&plain(&text), start, end), " codex ");
    }

    #[test]
    fn without_a_selection_colour_the_label_is_underlined() {
        let text = render::render_chips(&theme(), 90, &chips(), 2, false);
        assert_eq!(painted(&text, 90), "");
        assert!(text.contains(UNDERLINE));
    }

    #[test]
    fn logos_lead_every_agent_and_a_shell_gets_a_dollar() {
        let text = plain(&render::render_chips(&iconic(), 90, &chips(), 0, false));
        assert!(text.contains("$  shell"));
        assert!(text.contains("\u{e1a0}  claude"));
        assert!(text.contains("\u{e1a1}  codex"));
    }

    #[test]
    fn a_chip_scrolled_out_of_view_spans_nothing() {
        let (text, spans) = render::chips_row(&theme(), 44, &crowded(), 10, false);
        assert_eq!(spans[0], (-1, -1));
        let (start, end) = spans[10];
        assert!(char_slice(&plain(&text), start, end).contains("agent-9"));
    }
}

mod target_line {
    use super::*;

    #[test]
    fn skipping_is_spelled_out_next_to_the_agent() {
        let agent = Agent::new("claude", "claude");
        let text =
            plain(&render::render_target(&theme(), 80, Some(&tab_target("Atlas")), Some(&agent), "", true, SMART));
        assert!(text.contains("claude (skip permissions)"));
    }

    #[test]
    fn it_is_silent_when_the_toggle_is_off() {
        let agent = Agent::new("claude", "claude");
        let text =
            plain(&render::render_target(&theme(), 80, Some(&tab_target("Atlas")), Some(&agent), "", false, SMART));
        assert!(!text.contains("skip permissions"));
    }

    #[test]
    fn a_tab_names_the_space_it_lands_in() {
        let agent = Agent::new("", "shell");
        let text =
            plain(&render::render_target(&theme(), 80, Some(&tab_target("Atlas")), Some(&agent), "", false, SMART));
        assert!(text.contains("new tab in"));
        assert!(text.contains("Atlas"));
    }

    #[test]
    fn a_new_space_says_so() {
        let target = space_target("/a/thing", "thing");
        let text = plain(&render::render_target(&theme(), 80, Some(&target), None, "", false, SMART));
        assert!(text.contains("new space"));
        assert!(text.contains("thing"));
    }

    #[test]
    fn the_agent_is_named_when_one_is_chosen() {
        let agent = Agent::new("claude", "claude");
        let target = space_target("/a", "a");
        let text = plain(&render::render_target(&theme(), 80, Some(&target), Some(&agent), "", false, SMART));
        assert!(text.contains("claude"));
    }

    #[test]
    fn the_switch_sits_at_the_end_of_the_line() {
        let text = plain(&render::render_target(&pills(), 80, Some(&tab_target("Atlas")), None, "", false, TAB));
        assert!(text.starts_with("  → new tab in Atlas"));
        assert!(text.trim_end().ends_with("smart   tab   space"));
        assert_eq!(width(&text), 80);
        assert!(!text.contains('[')); // nothing left for a badge to say
    }

    #[test]
    fn the_switch_marks_only_the_current_mode_with_a_pill() {
        for mode in [SMART, TAB, SPACE] {
            let (raw, w) = render::mode_switch(&pills(), mode);
            assert_eq!(plain(&raw), " smart   tab   space ");
            assert_eq!(w, width(&plain(&raw)));
            assert_eq!(painted(&raw, w), format!(" {} ", mode));
        }
    }

    #[test]
    fn without_a_selection_colour_the_current_mode_is_underlined() {
        let (raw, _) = render::mode_switch(&theme(), TAB);
        assert_eq!(raw.matches(UNDERLINE).count(), 1);
        let at = raw.find(UNDERLINE).unwrap();
        assert!(plain(&raw[at..]).starts_with("tab"));
    }

    #[test]
    fn a_popup_too_narrow_for_the_switch_badges_an_override() {
        let text = plain(&render::render_target(&theme(), 40, Some(&tab_target("Atlas")), None, "", false, "tab"));
        assert!(text.contains("new tab in"));
        assert!(text.contains("[tab]"));
        assert!(width(&text) <= 40);
    }

    #[test]
    fn the_automatic_decision_is_not_badged() {
        let text = plain(&render::render_target(&theme(), 80, Some(&tab_target("Atlas")), None, "", false, SMART));
        assert!(!text.contains('['));
    }

    #[test]
    fn an_error_replaces_the_line() {
        let text = plain(&render::render_target(&theme(), 80, None, None, "/x does not exist", false, SMART));
        assert!(text.contains("does not exist"));
    }

    #[test]
    fn it_fits_a_narrow_popup() {
        let agent = Agent::new("claude", "c");
        let target = tab_target("A Very Long Space Name");
        let text = plain(&render::render_target(&theme(), 24, Some(&target), Some(&agent), "", false, SMART));
        assert!(width(&text) <= 24);
    }
}

mod mode_note {
    use super::*;

    #[test]
    fn each_mode_is_spelled_out_in_words() {
        for mode in [SMART, TAB, SPACE] {
            let text = plain(&render::render_mode_note(&theme(), 105, mode));
            assert!(text.contains(mode));
            let start: String = render::mode_note(mode).chars().take(20).collect();
            assert!(text.contains(&start));
        }
    }

    #[test]
    fn it_fits_a_narrower_popup() {
        for w in 20..106 {
            let note = plain(&render::render_mode_note(&theme(), w, TAB));
            assert!(width(&note) <= w);
        }
    }
}

mod footer {
    use super::*;

    const HINTS: [(&str, &str); 7] = [
        ("⇥", "agent"),
        ("↑↓", "folder"),
        ("→", "complete"),
        ("^T", "smart"),
        ("^Y", "skip perms"),
        ("⏎", "create"),
        ("esc", ""),
    ];

    fn hints() -> Vec<(String, String)> {
        HINTS.iter().map(|(key, label)| (key.to_string(), label.to_string())).collect()
    }

    fn footer(width: isize) -> String {
        plain(&render::render_footer(&theme(), width, &hints(), "1/8", ""))
    }

    fn guided(width: isize) -> String {
        plain(&render::render_footer(&theme(), width, &hints(), "1/8", "? help"))
    }

    fn kept(text: &str) -> usize {
        HINTS.iter().filter(|(key, _)| text.contains(key)).count()
    }

    #[test]
    fn a_wide_row_keeps_every_hint_and_the_counter() {
        let text = footer(100);
        for (key, _) in HINTS {
            assert!(text.contains(key));
        }
        assert!(text.trim_end().ends_with("1/8"));
    }

    #[test]
    fn the_popups_own_width_fits_all_the_keys_that_do_something() {
        // 75 is what Herdr's 80-cell popup leaves the picker's inner area.
        let text = footer(75);
        for key in ["⇥", "↑↓", "→", "^T", "^Y", "⏎"] {
            assert!(text.contains(key));
        }
        assert!(text.contains("create"));
    }

    #[test]
    fn the_gap_closes_up_before_a_hint_is_given_up() {
        let (wide, tight) = (footer(100), footer(75));
        assert!(wide.contains("agent   ↑↓"));
        assert!(tight.contains("agent  ↑↓"));
    }

    #[test]
    fn a_shed_hint_never_leaves_its_key_behind() {
        for w in 12..90 {
            let text = footer(w);
            for (key, label) in HINTS {
                if !label.is_empty() && text.contains(key) {
                    assert!(text.contains(label), "{:?} kept {:?} without its label", w, key);
                }
            }
        }
    }

    #[test]
    fn it_never_outgrows_its_width() {
        for w in 12..90 {
            assert!(width(&footer(w)) <= w);
        }
    }

    #[test]
    fn the_way_into_the_help_survives_every_width() {
        for w in 20..90 {
            assert!(guided(w).contains('?'));
        }
    }

    #[test]
    fn it_gives_up_the_guides_label_before_it_gives_up_a_hint() {
        assert!(guided(90).contains("? help"));
        assert!(!guided(60).contains("? help")); // the label goes...
        for w in 20..90 {
            // ...and asking for the label never costs a hint that asking for the
            // bare key would have kept.
            let bare = plain(&render::render_footer(&theme(), w, &hints(), "1/8", "?"));
            assert_eq!(kept(&guided(w)), kept(&bare), "{}", w);
        }
    }

    #[test]
    fn the_guide_still_leaves_room_for_the_counter() {
        assert!(guided(75).trim_end().ends_with("1/8"));
    }
}

mod help_panel {
    use super::*;

    // The size the popup actually ships: Herdr hands the pty `height - 2` rows
    // and `width - 3` columns of the manifest's 110x30, and the picker keeps one
    // column of padding on each side of that.
    fn rows_of(width: isize, height: isize, mode: &str) -> Vec<String> {
        render::render_help(&theme(), width, height, mode).iter().map(|row| plain(row)).collect()
    }

    fn rows() -> Vec<String> {
        rows_of(105, 28, SMART)
    }

    #[test]
    fn it_fills_the_popup_exactly() {
        let rows = rows();
        assert_eq!(rows.len(), 28);
        assert!(!rows.last().unwrap().trim().is_empty(), "the last row is content, not padding");
        for row in &rows {
            assert!(width(row) <= 105);
        }
    }

    #[test]
    fn every_mode_is_spelled_out() {
        let text = rows().join("\n");
        for (name, description) in render::HELP_MODES {
            assert!(text.contains(name));
            let start: String = description.chars().take(20).collect();
            assert!(text.contains(&start));
        }
    }

    #[test]
    fn the_current_mode_is_the_only_one_marked() {
        let marked: Vec<String> = rows_of(105, 28, "space").into_iter().filter(|row| row.contains('▌')).collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].contains("space"));
    }

    #[test]
    fn it_says_how_to_get_out() {
        assert!(rows()[0].contains("esc"));
    }

    #[test]
    fn a_short_popup_keeps_the_modes_and_sheds_the_rest() {
        let rows = rows_of(105, 12, SMART);
        assert_eq!(rows.len(), 12);
        assert!(rows.join("\n").contains("^T"));
        assert!(rows.join("\n").contains("space"));
    }

    #[test]
    fn a_narrow_popup_stays_inside_its_width() {
        for w in 20..106 {
            for row in rows_of(w, 28, SMART) {
                assert!(width(&row) <= w);
            }
        }
    }
}

mod scrollbar {
    use super::*;

    #[test]
    fn a_list_that_fits_has_no_bar() {
        assert!(render::scrollbar_column(5, 10, 0, 10).is_empty());
    }

    #[test]
    fn the_thumb_moves_with_the_offset() {
        let top = render::scrollbar_column(100, 10, 0, 10);
        let bottom = render::scrollbar_column(100, 10, 90, 10);
        assert_eq!(top[0], render::SCROLL_THUMB);
        assert_eq!(bottom[9], render::SCROLL_THUMB);
    }
}

mod frame {
    use super::*;

    #[test]
    fn a_frame_fits_the_popup_it_was_asked_for() {
        let mut app = picker(vec![row("alpha").suggestion, row("beta").suggestion]);
        for (w, h) in [(92, 20), (60, 10), (40, 6), (30, 3)] {
            let lines = app.frame(w, h);
            assert!(lines.len() as isize <= h, "{:?}", (w, h));
            for line in &lines {
                assert!(width(&plain(line)) <= w, "{:?}", (w, h, line));
            }
        }
    }

    #[test]
    fn the_hints_change_with_what_tab_would_do() {
        let mut app = picker(vec![row("alpha").suggestion]);
        let without = plain(&app.frame(92, 20).concat());
        assert!(without.contains("⇥ agent"));
    }
}
