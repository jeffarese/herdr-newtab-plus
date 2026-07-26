from __future__ import annotations

import unittest

from herdr_newtab import render
from herdr_newtab.agents import Agent
from herdr_newtab.paths import SOURCE_RECENT, SOURCE_ROOT, SOURCE_SPACE, Scored, Suggestion
from herdr_newtab.session import SPACE, TAB, Target
from herdr_newtab.textutil import display_width, strip_ansi
from herdr_newtab.theme import Theme

from .fixtures import CHIPS, picker

THEME = Theme()


def row(name, source=SOURCE_ROOT, detail="", is_git=False, path=None):
    return Scored(
        Suggestion(path or ("/a/" + name), name, source, detail, is_git),
        0,
        (),
        (),
    )


def plain(text):
    return strip_ansi(text)


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


class RowTest(unittest.TestCase):
    def test_a_row_fits_its_width(self):
        text = plain(render.render_row(THEME, row("alpha"), 60, False))
        self.assertLessEqual(display_width(text), 60)

    def test_a_space_badge_names_the_space(self):
        text = plain(render.render_row(THEME, row("a", SOURCE_SPACE, "WeekGen"), 60, False))
        self.assertIn("WeekGen", text)

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
        self.assertIn("agent-10", text)
        self.assertLessEqual(display_width(text), 44)


class TargetLineTest(unittest.TestCase):
    def test_a_tab_names_the_space_it_lands_in(self):
        text = plain(
            render.render_target(THEME, 80, Target(TAB, "/a", "w1", "WeekGen"), Agent("", "shell"))
        )
        self.assertIn("new tab in", text)
        self.assertIn("WeekGen", text)

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
