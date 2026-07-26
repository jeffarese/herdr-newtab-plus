from __future__ import annotations

import os
import re
import unittest

from herdr_newtab import paths
from herdr_newtab.app import CANCEL, CONFIRM
from herdr_newtab.config import Config
from herdr_newtab.keys import MOUSE, PRESS, WHEEL_DOWN, Event
from herdr_newtab.paths import SOURCE_ROOT, Suggestion
from herdr_newtab.session import SPACE, TAB
from herdr_newtab.state import Recents

from .fixtures import Tree, pane, picker, snapshot


def catalog(*names):
    return [Suggestion("/a/" + name, name, SOURCE_ROOT) for name in names]


def plain(frame):
    """A frame with its colors stripped, so text can be asserted on."""
    return re.sub(r"\x1b\[[0-9;]*m", "", "\n".join(frame))


def press(app, *keys):
    for key in keys:
        outcome = app.handle_key(key)
        if outcome:
            return outcome
    return None


class TypingTest(unittest.TestCase):
    def setUp(self):
        self.app = picker(catalog("alpha", "beta"))

    def test_typing_filters(self):
        self.app.insert("alp")
        self.assertEqual([row.suggestion.name for row in self.app.rows], ["alpha"])

    def test_backspace_widens_again(self):
        self.app.insert("alp")
        press(self.app, "backspace", "backspace", "backspace")
        self.assertEqual(len(self.app.rows), 2)

    def test_control_characters_never_reach_the_query(self):
        self.app.insert("a\x00b")
        self.assertEqual(self.app.query, "ab")

    def test_a_pasted_newline_becomes_a_space(self):
        self.app.insert("a\nb", pasted=True)
        self.assertEqual(self.app.query, "a b")

    def test_delete_word_walks_back_a_path_segment(self):
        self.app.set_query("~/workspace/atlas/")
        self.app.delete_word()
        self.assertEqual(self.app.query, "~/workspace/")
        self.app.delete_word()
        self.assertEqual(self.app.query, "~/")

    def test_ctrl_u_clears(self):
        self.app.insert("alpha")
        press(self.app, "ctrl+u")
        self.assertEqual(self.app.query, "")

    def test_cursor_keys_stay_inside_the_query(self):
        self.app.insert("ab")
        press(self.app, "left", "left", "left")
        self.assertEqual(self.app.cursor, 0)
        press(self.app, "end")
        self.assertEqual(self.app.cursor, 2)


class MovementTest(unittest.TestCase):
    def setUp(self):
        self.app = picker(catalog("a", "b", "c"))

    def test_down_moves_and_wraps(self):
        press(self.app, "down", "down", "down")
        self.assertEqual(self.app.selected, 0)

    def test_up_wraps_backwards(self):
        press(self.app, "up")
        self.assertEqual(self.app.selected, 2)

    def test_filtering_resets_the_selection(self):
        press(self.app, "down")
        self.app.insert("a")
        self.assertEqual(self.app.selected, 0)

    def test_wheel_scrolls_without_wrapping(self):
        self.app.handle_mouse(Event(MOUSE, WHEEL_DOWN, 1, 1))
        self.assertEqual(self.app.selected, 2)


class AgentChipTest(unittest.TestCase):
    def setUp(self):
        self.app = picker(catalog("a"))

    def test_shift_tab_cycles_backwards(self):
        press(self.app, "shift+tab")
        self.assertEqual(self.app.agent().label, "pi")

    def test_tab_cycles_when_there_is_nothing_to_complete(self):
        start = self.app.chip
        press(self.app, "tab")
        self.assertEqual(self.app.chip, (start + 1) % 4)

    def test_the_chip_row_wraps(self):
        for _ in range(4):
            press(self.app, "tab")
        self.assertEqual(self.app.chip, 0)


class CompletionTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("alpha", "alpha/inner")
        paths.clear_cache()
        self.app = picker(base=self.tree.root)

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def test_tab_completes_before_it_touches_the_agent(self):
        chip = self.app.chip
        self.app.set_query(self.tree.path("alp"))
        self.assertTrue(self.app.ghost())
        press(self.app, "tab")
        self.assertEqual(self.app.query, self.tree.path("alpha") + os.sep)
        self.assertEqual(self.app.chip, chip)

    def test_a_second_tab_then_moves_the_agent(self):
        self.app.set_query(self.tree.path("alpha") + os.sep)
        chip = self.app.chip
        # The folder itself leads the list, so there is nothing left to extend.
        self.assertEqual(self.app.ghost(), "")
        press(self.app, "tab")
        self.assertNotEqual(self.app.chip, chip)

    def test_right_at_the_end_accepts_the_row(self):
        self.app.set_query(self.tree.path("alp"))
        press(self.app, "right")
        self.assertEqual(self.app.query, self.tree.path("alpha") + os.sep)

    def test_right_inside_the_query_just_moves_the_cursor(self):
        self.app.set_query(self.tree.path("alp"), cursor=0)
        press(self.app, "right")
        self.assertEqual(self.app.cursor, 1)
        self.assertEqual(self.app.query, self.tree.path("alp"))

    def test_accepting_drills_into_the_folder(self):
        self.app.set_query(self.tree.path("alp"))
        press(self.app, "right")
        names = [row.suggestion.name for row in self.app.rows]
        self.assertEqual(names[0], "alpha")
        self.assertIn("inner", names)


class TargetTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("open", "fresh")
        paths.clear_cache()
        self.snap = snapshot([pane("w1:p1", self.tree.path("open"))])

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def app(self, **kwargs):
        return picker(base=self.tree.root, snap=self.snap, **kwargs)

    def test_a_folder_with_a_space_becomes_a_tab(self):
        app = self.app()
        app.set_query(self.tree.path("open"))
        self.assertEqual(app.target().kind, TAB)

    def test_a_folder_without_one_becomes_a_space(self):
        app = self.app()
        app.set_query(self.tree.path("fresh"))
        self.assertEqual(app.target().kind, SPACE)

    def test_the_target_follows_the_highlighted_row(self):
        app = self.app()
        app.set_query(self.tree.root + os.sep)
        kinds = []
        for _ in range(3):
            if app.target():
                kinds.append((app.selected_row().suggestion.name, app.target().kind))
            app.move(1)
        self.assertIn(("open", TAB), kinds)
        self.assertIn(("fresh", SPACE), kinds)

    def test_confirming_a_real_folder_returns_a_choice(self):
        app = self.app()
        app.set_query(self.tree.path("fresh"))
        self.assertEqual(press(app, "enter"), CONFIRM)
        choice = app.choice()
        self.assertEqual(choice.folder, self.tree.path("fresh"))
        self.assertEqual(choice.target.kind, SPACE)

    def test_confirming_a_folder_that_is_not_there_refuses(self):
        app = self.app()
        app.set_query("/no/such/folder")
        self.assertIsNone(app.choice())
        self.assertIn("not a folder", app.status or "")

    def test_an_empty_query_refuses(self):
        app = picker(base=self.tree.root, snap=self.snap)
        self.assertIsNone(app.choice())


class RememberedAgentTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("one", "two")
        paths.clear_cache()
        self.recents = Recents(None)
        self.recents.touch(self.tree.path("one"), "codex")
        self.recents.touch(self.tree.path("two"), "claude")

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def test_the_chip_follows_the_highlighted_folder(self):
        app = picker(base=self.tree.root, recents=self.recents)
        app.set_query(self.tree.path("one"))
        self.assertEqual(app.agent().kind, "codex")
        app.set_query(self.tree.path("two"))
        self.assertEqual(app.agent().kind, "claude")

    def test_moving_the_chips_by_hand_pins_the_choice(self):
        app = picker(base=self.tree.root, recents=self.recents)
        app.set_query(self.tree.path("one"))
        press(app, "shift+tab")  # always the chips, never the completion
        pinned = app.agent().kind
        app.set_query(self.tree.path("two"))
        self.assertEqual(app.agent().kind, pinned)

    def test_a_configured_default_beats_the_memory(self):
        app = picker(
            base=self.tree.root,
            recents=self.recents,
            config=Config({"roots": [], "default_agent": "none"}),
        )
        app.set_query(self.tree.path("one"))
        self.assertTrue(app.agent().is_none)


class MouseTest(unittest.TestCase):
    def setUp(self):
        self.app = picker(catalog("a", "b", "c"))
        self.app.frame(80, 20)  # settle the row and chip geometry

    def test_clicking_a_row_selects_it_then_commits(self):
        row = self.app._row_top + 2 + 1  # zero-based row -> one-based y
        self.assertIsNone(self.app.handle_mouse(Event(MOUSE, PRESS, 4, row)))
        self.assertEqual(self.app.selected, 2)
        self.assertEqual(self.app.handle_mouse(Event(MOUSE, PRESS, 4, row)), CONFIRM)

    def test_clicking_a_chip_picks_that_agent(self):
        start, _ = self.app._chip_spans[2]
        self.app.handle_mouse(Event(MOUSE, PRESS, start + 1, self.app._chip_row + 1))
        self.assertEqual(self.app.chip, 2)

    def test_clicking_past_the_list_does_nothing(self):
        self.assertIsNone(self.app.handle_mouse(Event(MOUSE, PRESS, 4, 19)))


class CreateModeTest(unittest.TestCase):
    """^T decides between letting the plugin choose and forcing either answer."""

    def setUp(self):
        self.tree = Tree("open", "fresh")
        paths.clear_cache()
        self.snap = snapshot([pane("w1:p1", self.tree.path("open"))])

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def app(self, **kwargs):
        return picker(base=self.tree.root, snap=self.snap, **kwargs)

    def test_it_starts_on_what_the_config_says(self):
        self.assertEqual(self.app().create_mode, "smart")
        self.assertEqual(self.app(config=Config({"create": "tab"})).create_mode, "tab")

    def test_a_remembered_mode_wins_over_the_config(self):
        recents = Recents(None)
        recents.remember_create_mode("space")
        self.assertEqual(self.app(recents=recents).create_mode, "space")

    def test_ctrl_t_cycles_through_the_three(self):
        app = self.app()
        seen = [app.create_mode]
        for _ in range(3):
            press(app, "ctrl+t")
            seen.append(app.create_mode)
        self.assertEqual(seen, ["smart", TAB, SPACE, "smart"])

    def test_forcing_a_space_stops_reusing_the_one_the_folder_has(self):
        app = self.app()
        app.set_query(self.tree.path("open"))
        self.assertEqual(app.target().kind, TAB)
        press(app, "ctrl+t", "ctrl+t")  # smart -> tab -> space
        self.assertEqual(app.target().kind, SPACE)
        self.assertEqual(app.choice().target.kind, SPACE)

    def test_forcing_a_tab_lands_a_fresh_folder_in_the_focused_space(self):
        app = self.app()
        app.set_query(self.tree.path("fresh"))
        self.assertEqual(app.target().kind, SPACE)
        press(app, "ctrl+t")
        target = app.target()
        self.assertEqual(target.kind, TAB)
        self.assertEqual(target.workspace_id, "w1")
        self.assertEqual(app.choice().target.workspace_id, "w1")

    def test_the_mode_is_handed_to_the_state_so_it_outlives_the_popup(self):
        recents = Recents(None)
        app = self.app(recents=recents)
        press(app, "ctrl+t")
        self.assertEqual(recents.create_mode(), TAB)

    def test_the_frame_badges_an_override_and_the_footer_names_the_mode(self):
        app = self.app()
        app.set_query(self.tree.path("fresh"))
        self.assertIn(("^T", "smart"), app._hints())
        self.assertNotIn("[tab]", plain(app.frame(80, 20)))  # nothing to badge yet
        press(app, "ctrl+t")
        self.assertIn(("^T", TAB), app._hints())
        self.assertIn("[tab]", plain(app.frame(80, 20)))

    def test_a_forced_mode_survives_moving_through_the_list(self):
        app = self.app()
        press(app, "ctrl+t", "ctrl+t")
        app.set_query(self.tree.root + os.sep)
        app.move(1)
        self.assertEqual(app.create_mode, SPACE)
        self.assertEqual(app.target().kind, SPACE)


class HelpPanelTest(unittest.TestCase):
    def setUp(self):
        self.app = picker(catalog("alpha", "beta"))

    def test_a_question_mark_opens_it_while_the_input_is_empty(self):
        self.app.insert("?")
        self.assertTrue(self.app.help)
        self.assertEqual(self.app.query, "")

    def test_a_question_mark_is_a_path_character_once_something_is_typed(self):
        self.app.insert("a")
        self.app.insert("?")
        self.assertFalse(self.app.help)
        self.assertEqual(self.app.query, "a?")

    def test_a_pasted_question_mark_is_never_a_key(self):
        self.app.insert("?", pasted=True)
        self.assertFalse(self.app.help)
        self.assertEqual(self.app.query, "?")

    def test_f1_opens_it_whatever_is_in_the_input(self):
        self.app.set_query("~/w")
        press(self.app, "f1")
        self.assertTrue(self.app.help)

    def test_anything_closes_it_again(self):
        for close in ("esc", "f1", "down", "enter"):
            self.app.help = True
            self.assertIsNone(press(self.app, close))
            self.assertFalse(self.app.help, close)

    def test_typing_closes_it_without_reaching_the_query(self):
        self.app.help = True
        self.app.insert("x")
        self.assertFalse(self.app.help)
        self.assertEqual(self.app.query, "")

    def test_enter_behind_the_panel_does_not_create(self):
        self.app.help = True
        self.assertIsNone(press(self.app, "enter"))

    def test_ctrl_c_still_cancels_from_behind_it(self):
        self.app.help = True
        self.assertEqual(press(self.app, "ctrl+c"), CANCEL)

    def test_a_click_closes_it_rather_than_landing_on_a_row(self):
        self.app.help = True
        self.assertIsNone(self.app.handle_mouse(Event(MOUSE, PRESS, 4, 4)))
        self.assertFalse(self.app.help)

    def test_the_panel_marks_the_mode_the_picker_is_on(self):
        press(self.app, "ctrl+t")  # smart -> tab
        self.app.help = True
        marked = [line for line in plain(self.app.frame(107, 28)).split("\n") if "▌" in line]
        self.assertEqual(len(marked), 1)
        self.assertIn("tab", marked[0])

    def test_the_panel_explains_every_mode_and_takes_the_whole_popup(self):
        self.app.help = True
        text = plain(self.app.frame(107, 28))
        for mode in ("smart", TAB, SPACE):
            self.assertIn(mode, text)
        self.assertNotIn("alpha", text)  # the list is gone while it is up

    def test_the_corner_names_the_key_that_actually_works(self):
        self.assertEqual(self.app._guide(), "? help")
        self.app.set_query("~/w")
        self.assertEqual(self.app._guide(), "F1 help")

    def test_the_way_in_is_on_screen_in_every_state(self):
        for query in ("", "~/w"):
            self.app.set_query(query)
            self.assertIn("help", plain(self.app.frame(107, 28)).split("\n")[-1])


class SkipPermissionsTest(unittest.TestCase):
    def setUp(self):
        # A real folder, so the target line says what Enter would do rather
        # than reporting a path that does not exist.
        self.tree = Tree("alpha")
        paths.clear_cache()
        self.app = picker(base=self.tree.root)
        self.app.set_query(self.tree.path("alpha"))
        self.app.chip = 1  # claude, which has a known flag

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def test_it_starts_off(self):
        self.assertFalse(self.app.skip_permissions)
        self.assertFalse(self.app.skipping())

    def test_ctrl_y_arms_and_disarms_it(self):
        press(self.app, "ctrl+y")
        self.assertTrue(self.app.skipping())
        press(self.app, "ctrl+y")
        self.assertFalse(self.app.skipping())

    def test_an_agent_with_no_flag_refuses_and_says_why(self):
        self.app.chip = 3  # pi
        press(self.app, "ctrl+y")
        self.assertFalse(self.app.skip_permissions)
        self.assertIn("no skip-permissions flag for pi", self.app.error())

    def test_a_plain_shell_refuses_too(self):
        self.app.chip = 0
        press(self.app, "ctrl+y")
        self.assertFalse(self.app.skip_permissions)
        self.assertIn("plain shell", self.app.error())

    def test_cycling_onto_an_agent_without_a_flag_does_not_claim_to_skip(self):
        press(self.app, "ctrl+y")
        self.app.chip = 3  # pi
        self.assertFalse(self.app.skipping())
        # ...and cycling back finds the setting where it was left.
        self.app.chip = 1
        self.assertTrue(self.app.skipping())

    def test_the_choice_carries_it(self):
        press(self.app, "ctrl+y")
        self.assertTrue(self.app.choice().skip_permissions)

    def test_an_agent_without_a_flag_never_reaches_the_choice(self):
        press(self.app, "ctrl+y")
        self.app.chip = 3  # pi
        self.assertFalse(self.app.choice().skip_permissions)

    def test_the_chip_row_and_target_line_both_show_it(self):
        press(self.app, "ctrl+y")
        frame = "\n".join(self.app.frame(80, 20))
        self.assertIn("claude!", frame)
        self.assertIn("skip permissions", frame)

    def test_a_click_still_lands_on_the_marked_chip(self):
        press(self.app, "ctrl+y")
        frame = self.app.frame(80, 20)  # measures the spans
        self.assertTrue(any("claude!" in line for line in frame))
        start, end = self.app._chip_spans[2]  # codex, after the widened claude
        self.app.handle_mouse(Event(MOUSE, PRESS, start + 1, self.app._chip_row + 1))
        self.assertEqual(self.app.agent().kind, "codex")
        self.assertLess(start, end)

    def test_the_footer_only_offers_the_key_where_it_would_work(self):
        self.app.chip = 3  # pi
        self.assertNotIn("^Y", " ".join(key for key, _ in self.app._hints()))
        self.app.chip = 1
        self.assertIn("^Y", " ".join(key for key, _ in self.app._hints()))


class CancelTest(unittest.TestCase):
    def test_escape_and_ctrl_c_cancel(self):
        for key in ("esc", "ctrl+c"):
            self.assertEqual(picker(catalog("a")).handle_key(key), CANCEL)


if __name__ == "__main__":
    unittest.main()
