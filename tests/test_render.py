from __future__ import annotations

import unittest

from herdr_newtab import render
from herdr_newtab.agents import Agent
from herdr_newtab.paint import cells_of
from herdr_newtab.paths import SOURCE_RECENT, SOURCE_ROOT, SOURCE_SPACE, Scored, Suggestion
from herdr_newtab.session import SPACE, TAB, Target
from herdr_newtab.textutil import display_width, strip_ansi
from herdr_newtab.theme import UNDERLINE, Theme

from .fixtures import CHIPS, picker

THEME = Theme()
PILLS = Theme(selection_background="237")


def row(name, source=SOURCE_ROOT, detail="", is_git=False, path=None):
    return Scored(
        Suggestion(path or ("/a/" + name), name, source, detail, is_git),
        0,
        (),
        (),
    )


def plain(text):
    return strip_ansi(text)


def painted(text, width):
    """The characters a row draws on a background: what its pills cover."""
    cells, _ = cells_of(text, width)
    return "".join(char for char, style in cells if style[1])


class LayoutTest(unittest.TestCase):
    def test_a_tall_popup_shows_every_part(self):
        layout = render.compute_layout(90, 24)
        self.assertTrue(layout.rules and layout.chips and layout.target and layout.footer)

    def test_chrome_sheds_from_the_bottom_as_it_shrinks(self):
        self.assertFalse(render.compute_layout(90, 8).footer)
        self.assertFalse(render.compute_layout(90, 6).target)
        self.assertFalse(render.compute_layout(90, 4).chips)

    def test_the_list_never_disappears(self):
        for height in range(1, 30):
            self.assertGreaterEqual(render.compute_layout(90, height).list_height, 1)

    def test_the_list_is_capped_however_tall_the_popup_is(self):
        for height in (20, 28, 60):
            self.assertEqual(render.compute_layout(105, height).list_height, render.MAX_LIST)

    def test_the_rows_past_the_cap_become_air_rather_than_more_folders(self):
        layout = render.compute_layout(105, 28)
        self.assertGreater(layout.spare, 0)
        # Every row is accounted for: input, rule, list, air, rule, chips,
        # target, note, hints.
        chrome = 2 + 2 + 1 + 1 + 1
        self.assertEqual(layout.list_height + layout.spare + chrome, 28)

    def test_the_mode_note_only_appears_when_the_rows_are_going_spare(self):
        self.assertTrue(render.compute_layout(105, 28).note)
        self.assertFalse(render.compute_layout(75, 12).note)

    def test_a_small_popup_still_gives_the_list_what_is_left(self):
        layout = render.compute_layout(75, 12)
        self.assertEqual(layout.list_height, 6)
        self.assertEqual(layout.spare, 0)


class RowTest(unittest.TestCase):
    def test_a_row_fits_its_width(self):
        text = plain(render.render_row(THEME, row("alpha"), 60, False))
        self.assertLessEqual(display_width(text), 60)

    def test_a_space_badge_names_the_space(self):
        text = plain(render.render_row(THEME, row("a", SOURCE_SPACE, "Atlas"), 60, False))
        self.assertIn("Atlas", text)

    def test_a_recent_row_says_so(self):
        self.assertIn("recent", plain(render.render_row(THEME, row("a", SOURCE_RECENT), 60, False)))

    def test_a_git_folder_is_badged_when_nothing_else_claims_the_slot(self):
        self.assertIn("git", plain(render.render_row(THEME, row("a", is_git=True), 60, False)))

    def test_the_selected_row_is_marked(self):
        text = plain(render.render_row(THEME, row("alpha"), 60, True))
        self.assertTrue(text.startswith(render.SELECTED_BAR))

    def test_a_long_name_is_truncated_rather_than_wrapped(self):
        text = plain(render.render_row(THEME, row("x" * 200), 60, False))
        self.assertLessEqual(display_width(text), 60)

    def test_a_narrow_row_drops_the_badge_before_the_name(self):
        text = plain(render.render_row(THEME, row("alpha", SOURCE_RECENT), 30, False))
        self.assertIn("alpha", text)
        self.assertLessEqual(display_width(text), 30)

    def test_the_name_column_follows_the_rows_on_screen(self):
        narrow = render.name_column([row("ab"), row("cd")], 80)
        wide = render.name_column([row("ab"), row("a-very-long-folder-name")], 80)
        self.assertLess(narrow, wide)


def iconic():
    theme = Theme(selection_background="237")
    theme.agent_icons = True
    return theme


class RowDecorationTest(unittest.TestCase):
    def test_the_folders_agent_leads_the_row_as_a_logo(self):
        text = plain(render.render_row(iconic(), row("alpha"), 80, False, 12, "claude"))
        self.assertTrue(text.startswith("  \ue1a0  alpha"), repr(text))
        self.assertNotIn("claude", text)  # the logo already says it

    def test_rows_without_an_agent_keep_the_gutter(self):
        theme = iconic()
        bare = plain(render.render_row(theme, row("alpha"), 80, False, 12))
        marked = plain(render.render_row(theme, row("alpha"), 80, False, 12, "codex"))
        self.assertEqual(bare.index("alpha"), marked.index("alpha"))

    def test_without_the_font_the_agent_is_named_at_the_end(self):
        text = plain(render.render_row(THEME, row("alpha", SOURCE_RECENT), 80, False, 12, "codex"))
        self.assertTrue(text.startswith("  alpha"))
        self.assertTrue(text.rstrip().endswith("codex · recent"), repr(text))

    def test_an_agent_standing_in_the_folder_shows_its_status(self):
        space = row("a", SOURCE_SPACE, "Atlas")
        text = plain(render.render_row(THEME, space, 60, False, None, None, "blocked"))
        self.assertTrue(text.rstrip().endswith("◉ Atlas"), repr(text))
        working = plain(render.render_row(THEME, row("a"), 60, False, None, None, "working"))
        self.assertTrue(working.rstrip().endswith("◐"), repr(working))

    def test_an_unreported_status_says_nothing(self):
        text = plain(
            render.render_row(THEME, row("a", SOURCE_SPACE, "Atlas"), 60, False, None, None, "none")
        )
        self.assertTrue(text.rstrip().endswith(" Atlas"))
        self.assertNotIn("◉", text)

    def test_a_decorated_row_still_fits_its_width(self):
        for theme in (THEME, iconic()):
            for width in range(20, 110, 7):
                for selected in (False, True):
                    text = plain(
                        render.render_row(
                            theme,
                            row("a-long-folder-name", SOURCE_SPACE, "A Space"),
                            width,
                            selected,
                            None,
                            "claude",
                            "working",
                        )
                    )
                    self.assertLessEqual(display_width(text), width, (width, selected))
                    if selected and theme.selection_bg:
                        self.assertEqual(display_width(text), width)  # a band, edge to edge


class EmptyListTest(unittest.TestCase):
    def test_it_says_what_happened_and_what_to_do(self):
        rows = [plain(line) for line in render.render_empty(THEME, 80, 5, "zzz")]
        self.assertEqual(len(rows), 5)
        self.assertIn("no folder matches", rows[0])
        self.assertIn("⏎ still creates it", rows[1])

    def test_a_one_row_list_keeps_the_headline(self):
        rows = render.render_empty(THEME, 80, 1, "")
        self.assertEqual(len(rows), 1)
        self.assertIn("no folders to suggest", plain(rows[0]))


class InputTest(unittest.TestCase):
    def test_an_empty_query_shows_the_placeholder(self):
        self.assertIn("folder", plain(render.render_input(THEME, 70, "", 0, "")))

    def test_the_ghost_follows_the_query(self):
        text = plain(render.render_input(THEME, 70, "~/work", 6, "space/"))
        self.assertIn("~/work", text)
        self.assertIn("space/", text)

    def test_a_long_query_keeps_the_cursor_on_screen(self):
        query = "/" + "a" * 200
        text = plain(render.render_input(THEME, 40, query, len(query), ""))
        self.assertLessEqual(display_width(text), 40)
        self.assertTrue(text.rstrip().endswith("a"))


class ChipsTest(unittest.TestCase):
    def test_every_agent_is_shown_when_they_fit(self):
        text = plain(render.render_chips(THEME, 90, CHIPS, 1))
        for chip in CHIPS:
            self.assertIn(chip.label, text)

    def test_a_crowded_row_still_shows_the_selection(self):
        many = [Agent("", "shell")] + [Agent("k%d" % i, "agent-%d" % i) for i in range(12)]
        text = plain(render.render_chips(THEME, 44, many, 10))
        self.assertIn("agent-9", text)  # chips[10], the shell leading the row
        self.assertIn("‹", text)
        self.assertIn("›", text)
        self.assertLessEqual(display_width(text), 44)

    def test_only_the_selected_chip_is_marked_as_skipping(self):
        text = plain(render.render_chips(THEME, 90, CHIPS, 1, True))
        self.assertIn("claude!", text)
        self.assertNotIn("codex!", text)

    def test_a_shell_is_never_marked(self):
        self.assertNotIn("shell!", plain(render.render_chips(THEME, 90, CHIPS, 0, True)))

    def test_a_crowded_row_stays_inside_its_width_when_marked(self):
        many = [Agent("", "shell")] + [Agent("k%d" % i, "agent-%d" % i) for i in range(12)]
        text = plain(render.render_chips(THEME, 44, many, 10, True))
        self.assertIn("agent-9!", text)  # chips[10], the shell leading the row
        self.assertLessEqual(display_width(text), 44)


class ChipSpanTest(unittest.TestCase):
    def test_each_span_covers_its_own_chip(self):
        for theme in (THEME, iconic()):
            text, spans = render.chips_row(theme, 90, CHIPS, 1, True)
            flat = plain(text)
            for chip, (start, end) in zip(CHIPS, spans):
                label = render.chip_label(chip, chip is CHIPS[1], True)
                self.assertIn(label, flat[start:end], (chip, flat[start:end]))

    def test_the_selected_chip_is_a_pill(self):
        text, spans = render.chips_row(PILLS, 90, CHIPS, 2)
        self.assertEqual(painted(text, 90), " codex ")
        start, end = spans[2]
        self.assertEqual(plain(text)[start:end], " codex ")

    def test_without_a_selection_colour_the_label_is_underlined(self):
        text = render.render_chips(THEME, 90, CHIPS, 2)
        self.assertEqual(painted(text, 90), "")
        self.assertIn(UNDERLINE, text)

    def test_logos_lead_every_agent_and_a_shell_gets_a_dollar(self):
        text = plain(render.render_chips(iconic(), 90, CHIPS, 0))
        self.assertIn("$  shell", text)
        self.assertIn("\ue1a0  claude", text)
        self.assertIn("\ue1a1  codex", text)

    def test_a_chip_scrolled_out_of_view_spans_nothing(self):
        many = [Agent("", "shell")] + [Agent("k%d" % i, "agent-%d" % i) for i in range(12)]
        text, spans = render.chips_row(THEME, 44, many, 10)
        self.assertEqual(spans[0], (-1, -1))
        start, end = spans[10]
        self.assertIn("agent-9", plain(text)[start:end])


class TargetLineTest(unittest.TestCase):
    def test_skipping_is_spelled_out_next_to_the_agent(self):
        text = plain(
            render.render_target(
                THEME, 80, Target(TAB, "/a", "w1", "Atlas"), Agent("claude", "claude"), "", True
            )
        )
        self.assertIn("claude (skip permissions)", text)

    def test_it_is_silent_when_the_toggle_is_off(self):
        text = plain(
            render.render_target(
                THEME, 80, Target(TAB, "/a", "w1", "Atlas"), Agent("claude", "claude")
            )
        )
        self.assertNotIn("skip permissions", text)

    def test_a_tab_names_the_space_it_lands_in(self):
        text = plain(
            render.render_target(THEME, 80, Target(TAB, "/a", "w1", "Atlas"), Agent("", "shell"))
        )
        self.assertIn("new tab in", text)
        self.assertIn("Atlas", text)

    def test_a_new_space_says_so(self):
        text = plain(
            render.render_target(
                THEME, 80, Target(SPACE, "/a/thing", workspace_label="thing"), None
            )
        )
        self.assertIn("new space", text)
        self.assertIn("thing", text)

    def test_the_agent_is_named_when_one_is_chosen(self):
        text = plain(
            render.render_target(
                THEME, 80, Target(SPACE, "/a", workspace_label="a"), Agent("claude", "claude")
            )
        )
        self.assertIn("claude", text)

    def test_the_switch_sits_at_the_end_of_the_line(self):
        text = plain(
            render.render_target(
                PILLS, 80, Target(TAB, "/a", "w1", "Atlas"), None, "", False, TAB
            )
        )
        self.assertTrue(text.startswith("  → new tab in Atlas"))
        self.assertTrue(text.rstrip().endswith("smart   tab   space"))
        self.assertEqual(display_width(text), 80)
        self.assertNotIn("[", text)  # nothing left for a badge to say

    def test_the_switch_marks_only_the_current_mode_with_a_pill(self):
        for mode in ("smart", TAB, SPACE):
            raw, width = render.mode_switch(PILLS, mode)
            self.assertEqual(plain(raw), " smart   tab   space ")
            self.assertEqual(width, display_width(plain(raw)))
            self.assertEqual(painted(raw, width), " %s " % mode)

    def test_without_a_selection_colour_the_current_mode_is_underlined(self):
        raw, _ = render.mode_switch(THEME, TAB)
        self.assertEqual(raw.count(UNDERLINE), 1)
        self.assertTrue(plain(raw[raw.index(UNDERLINE) :]).startswith("tab"))

    def test_a_popup_too_narrow_for_the_switch_badges_an_override(self):
        text = plain(
            render.render_target(
                THEME, 40, Target(TAB, "/a", "w1", "Atlas"), None, "", False, "tab"
            )
        )
        self.assertIn("new tab in", text)
        self.assertIn("[tab]", text)
        self.assertLessEqual(display_width(text), 40)

    def test_the_automatic_decision_is_not_badged(self):
        text = plain(
            render.render_target(THEME, 80, Target(TAB, "/a", "w1", "Atlas"), None, "", False)
        )
        self.assertNotIn("[", text)

    def test_an_error_replaces_the_line(self):
        text = plain(render.render_target(THEME, 80, None, None, "/x does not exist"))
        self.assertIn("does not exist", text)

    def test_it_fits_a_narrow_popup(self):
        text = plain(
            render.render_target(
                THEME, 24, Target(TAB, "/a", "w1", "A Very Long Space Name"), Agent("claude", "c")
            )
        )
        self.assertLessEqual(display_width(text), 24)


class ModeNoteTest(unittest.TestCase):
    def test_each_mode_is_spelled_out_in_words(self):
        for mode in ("smart", TAB, SPACE):
            text = plain(render.render_mode_note(THEME, 105, mode))
            self.assertIn(mode, text)
            self.assertIn(render.MODE_NOTES[mode][:20], text)

    def test_it_fits_a_narrower_popup(self):
        for width in range(20, 106):
            note = plain(render.render_mode_note(THEME, width, TAB))
            self.assertLessEqual(display_width(note), width)


class FooterTest(unittest.TestCase):
    HINTS = [
        ("⇥", "agent"),
        ("↑↓", "folder"),
        ("→", "complete"),
        ("^T", "smart"),
        ("^Y", "skip perms"),
        ("⏎", "create"),
        ("esc", ""),
    ]

    def footer(self, width):
        return plain(render.render_footer(THEME, width, self.HINTS, "1/8"))

    def test_a_wide_row_keeps_every_hint_and_the_counter(self):
        text = self.footer(100)
        for key, _ in self.HINTS:
            self.assertIn(key, text)
        self.assertTrue(text.rstrip().endswith("1/8"))

    def test_the_popups_own_width_fits_all_the_keys_that_do_something(self):
        # 75 is what Herdr's 80-cell popup leaves the picker's inner area.
        text = self.footer(75)
        for key in ("⇥", "↑↓", "→", "^T", "^Y", "⏎"):
            self.assertIn(key, text)
        self.assertIn("create", text)

    def test_the_gap_closes_up_before_a_hint_is_given_up(self):
        wide, tight = self.footer(100), self.footer(75)
        self.assertIn("agent   ↑↓", wide)
        self.assertIn("agent  ↑↓", tight)

    def test_a_shed_hint_never_leaves_its_key_behind(self):
        for width in range(12, 90):
            text = self.footer(width)
            for key, label in self.HINTS:
                if label and key in text:
                    self.assertIn(label, text, "%r kept %r without its label" % (width, key))

    def test_it_never_outgrows_its_width(self):
        for width in range(12, 90):
            self.assertLessEqual(display_width(self.footer(width)), width)

    def guided(self, width):
        return plain(render.render_footer(THEME, width, self.HINTS, "1/8", "? help"))

    def test_the_way_into_the_help_survives_every_width(self):
        for width in range(20, 90):
            self.assertIn("?", self.guided(width))

    def kept(self, text):
        return sum(1 for key, _ in self.HINTS if key in text)

    def test_it_gives_up_the_guides_label_before_it_gives_up_a_hint(self):
        self.assertIn("? help", self.guided(90))
        self.assertNotIn("? help", self.guided(60))  # the label goes...
        for width in range(20, 90):
            # ...and asking for the label never costs a hint that asking for the
            # bare key would have kept.
            bare = plain(render.render_footer(THEME, width, self.HINTS, "1/8", "?"))
            self.assertEqual(self.kept(self.guided(width)), self.kept(bare), width)

    def test_the_guide_still_leaves_room_for_the_counter(self):
        self.assertTrue(self.guided(75).rstrip().endswith("1/8"))


class HelpPanelTest(unittest.TestCase):
    # The size the popup actually ships: Herdr hands the pty `height - 2` rows
    # and `width - 3` columns of the manifest's 110x30, and the picker keeps one
    # column of padding on each side of that.
    def rows(self, width=105, height=28, mode="smart"):
        return [plain(row) for row in render.render_help(THEME, width, height, mode)]

    def test_it_fills_the_popup_exactly(self):
        rows = self.rows()
        self.assertEqual(len(rows), 28)
        self.assertTrue(rows[-1].strip(), "the last row is content, not padding")
        for row in rows:
            self.assertLessEqual(display_width(row), 105)

    def test_every_mode_is_spelled_out(self):
        text = "\n".join(self.rows())
        for name, description in render.HELP_MODES:
            self.assertIn(name, text)
            self.assertIn(description[:20], text)

    def test_the_current_mode_is_the_only_one_marked(self):
        marked = [row for row in self.rows(mode="space") if "▌" in row]
        self.assertEqual(len(marked), 1)
        self.assertIn("space", marked[0])

    def test_it_says_how_to_get_out(self):
        self.assertIn("esc", self.rows()[0])

    def test_a_short_popup_keeps_the_modes_and_sheds_the_rest(self):
        rows = self.rows(height=12)
        self.assertEqual(len(rows), 12)
        self.assertIn("^T", "\n".join(rows))
        self.assertIn("space", "\n".join(rows))

    def test_a_narrow_popup_stays_inside_its_width(self):
        for width in range(20, 106):
            for row in self.rows(width=width):
                self.assertLessEqual(display_width(row), width)


class ScrollbarTest(unittest.TestCase):
    def test_a_list_that_fits_has_no_bar(self):
        self.assertEqual(render.scrollbar_column(5, 10, 0, 10), {})

    def test_the_thumb_moves_with_the_offset(self):
        top = render.scrollbar_column(100, 10, 0, 10)
        bottom = render.scrollbar_column(100, 10, 90, 10)
        self.assertEqual(top[0], render.SCROLL_THUMB)
        self.assertEqual(bottom[9], render.SCROLL_THUMB)


class FrameTest(unittest.TestCase):
    def test_a_frame_fits_the_popup_it_was_asked_for(self):
        app = picker([row("alpha").suggestion, row("beta").suggestion])
        for width, height in ((92, 20), (60, 10), (40, 6), (30, 3)):
            lines = app.frame(width, height)
            self.assertLessEqual(len(lines), height, (width, height))
            for line in lines:
                self.assertLessEqual(display_width(plain(line)), width, (width, height, line))

    def test_the_hints_change_with_what_tab_would_do(self):
        app = picker([row("alpha").suggestion])
        without = plain("".join(app.frame(92, 20)))
        self.assertIn("⇥ agent", without)


if __name__ == "__main__":
    unittest.main()
