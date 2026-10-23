"""The folder preview: what it reads, how it is laid out, and how it is toggled."""

from __future__ import annotations

import os
import re
import unittest
from unittest import mock

from herdr_newtab import paths, render
from herdr_newtab.config import Config
from herdr_newtab.keys import MOUSE, PRESS, Event
from herdr_newtab.paths import SOURCE_ROOT, Contents, Suggestion
from herdr_newtab.session import FolderAgent, folder_agents
from herdr_newtab.textutil import display_width
from herdr_newtab.theme import Theme

from .fixtures import Tree, pane, picker, snapshot

# What the manifest's 110x30 popup gives the picker.
WIDE = (107, 28)


def plain(text):
    return re.sub(r"\x1b\[[0-9;]*m", "", text)


def touch(path, text=""):
    with open(path, "w") as handle:
        handle.write(text)


class ContentsTest(unittest.TestCase):
    def setUp(self):
        paths.clear_cache()
        self.addCleanup(paths.clear_cache)
        self.tree = Tree("app/src", "app/Tests", "app/.github")
        self.addCleanup(self.tree.close)
        for name in ("README.md", "build.sh", ".env"):
            touch(self.tree.path("app", name))

    def test_directories_come_first_and_each_half_is_sorted_without_case(self):
        listed = paths.contents(self.tree.path("app"))
        self.assertEqual(listed.dirs, (".github", "src", "Tests"))
        self.assertEqual(listed.files, (".env", "build.sh", "README.md"))
        self.assertTrue(listed.readable)

    def test_a_missing_folder_has_no_contents(self):
        self.assertIsNone(paths.contents(self.tree.path("nope")))
        self.assertIsNone(paths.contents(self.tree.path("app", "README.md")))

    def test_a_folder_read_once_answers_whether_it_exists(self):
        folder, file = self.tree.path("app"), self.tree.path("app", "README.md")
        paths.contents(folder)
        paths.contents(file)
        with mock.patch.object(os.path, "isdir", side_effect=AssertionError):
            self.assertTrue(paths.is_directory(folder))
            self.assertFalse(paths.is_directory(file))

    def test_fresh_asks_the_filesystem_whatever_was_read_before(self):
        folder = self.tree.path("app", "src")
        paths.contents(folder)
        os.rmdir(folder)
        self.assertTrue(paths.is_directory(folder))
        self.assertFalse(paths.is_directory(folder, fresh=True))

    def test_the_rescan_key_forgets_what_was_read(self):
        folder = self.tree.path("app")
        paths.contents(folder)
        touch(self.tree.path("app", "new.txt"))
        self.assertNotIn("new.txt", paths.contents(folder).files)
        paths.clear_cache()
        self.assertIn("new.txt", paths.contents(folder).files)

    @unittest.skipIf(hasattr(os, "geteuid") and os.geteuid() == 0, "root reads everything")
    def test_a_folder_that_is_not_ours_to_read_says_so(self):
        folder = self.tree.path("app", "src")
        os.chmod(folder, 0)
        self.addCleanup(os.chmod, folder, 0o755)
        listed = paths.contents(folder)
        self.assertIsNotNone(listed)
        self.assertFalse(listed.readable)


class BranchTest(unittest.TestCase):
    def setUp(self):
        paths.clear_cache()
        self.addCleanup(paths.clear_cache)
        self.tree = Tree("repo/.git", "plain", "worktree", "gitdirs/wt")
        self.addCleanup(self.tree.close)

    def test_a_checkout_names_its_branch(self):
        touch(self.tree.path("repo", ".git", "HEAD"), "ref: refs/heads/feature/login\n")
        self.assertEqual(paths.git_branch(self.tree.path("repo")), "feature/login")

    def test_a_detached_head_is_a_short_hash(self):
        touch(self.tree.path("repo", ".git", "HEAD"), "0123456789abcdef0123456789abcdef01234567\n")
        self.assertEqual(paths.git_branch(self.tree.path("repo")), "0123456")

    def test_a_worktree_follows_its_pointer(self):
        touch(self.tree.path("gitdirs", "wt", "HEAD"), "ref: refs/heads/wip\n")
        touch(self.tree.path("worktree", ".git"), "gitdir: ../gitdirs/wt\n")
        self.assertEqual(paths.git_branch(self.tree.path("worktree")), "wip")

    def test_a_folder_that_is_not_a_checkout_has_none_and_reads_nothing(self):
        folder = self.tree.path("plain")
        listed = paths.contents(folder)
        with mock.patch("builtins.open", side_effect=AssertionError):
            self.assertIsNone(paths.git_branch(folder, listed))
        self.assertIsNone(paths.git_branch(self.tree.path("missing")))


class FolderAgentsTest(unittest.TestCase):
    def test_every_agent_in_a_folder_most_urgent_first(self):
        snap = snapshot([pane("w1:p1", "/work/api"), pane("w1:p2", "/work/api")])
        snap["agents"] = [
            {"pane_id": "w1:p1", "agent": "claude", "agent_status": "idle"},
            {"pane_id": "w1:p2", "agent": "codex", "name": "reviewer", "agent_status": "blocked"},
        ]
        found = folder_agents(snap)["/work/api"]
        self.assertEqual(
            found,
            [FolderAgent("codex", "reviewer", "blocked"), FolderAgent("claude", "claude", "idle")],
        )

    def test_an_agent_without_a_status_is_still_listed(self):
        snap = snapshot([pane("w1:p1", "/work/api")])
        snap["agents"] = [{"pane_id": "w1:p1", "agent": "pi"}]
        self.assertEqual(folder_agents(snap)["/work/api"], [FolderAgent("pi", "pi", "")])

    def test_a_folder_with_nobody_in_it_is_absent(self):
        snap = snapshot([pane("w1:p1", "/work/api")])
        self.assertEqual(folder_agents(snap), {})


class LayoutTest(unittest.TestCase):
    def test_off_unless_asked_for(self):
        layout = render.compute_layout(*WIDE)
        self.assertEqual(layout.preview_width, 0)
        self.assertEqual(layout.list_width, layout.inner_width)

    def test_a_third_of_the_popup_beside_the_list(self):
        layout = render.compute_layout(*WIDE, preview=True)
        self.assertEqual(layout.preview_width, 105 // 3)
        self.assertEqual(
            layout.list_width + len(render.SEPARATOR) + layout.preview_width, layout.inner_width
        )
        self.assertGreaterEqual(layout.list_width, render.MIN_LIST)

    def test_the_shipped_popup_shows_it_by_itself(self):
        self.assertGreaterEqual(WIDE[0] - 2, render.PREVIEW_AUTO)

    def test_a_popup_too_narrow_or_too_short_keeps_the_list_whole(self):
        narrow = render.compute_layout(render.PREVIEW_FITS + 1, 28, preview=True)
        self.assertEqual(narrow.preview_width, 0)
        short = render.compute_layout(107, 5, preview=True)
        self.assertEqual(short.preview_width, 0)

    def test_it_never_changes_the_rows(self):
        for height in (6, 12, 28, 40):
            off = render.compute_layout(107, height)
            on = render.compute_layout(107, height, preview=True)
            self.assertEqual(off._replace(list_width=0, preview_width=0),
                             on._replace(list_width=0, preview_width=0))


class RenderPreviewTest(unittest.TestCase):
    def preview(self, **fields):
        values = dict(
            name="api",
            path="~/work/api",
            contents=Contents(("src", "tests", ".github"), ("README.md", ".env")),
        )
        values.update(fields)
        return render.Preview(**values)

    def lines(self, preview, width=35, height=20, theme=None):
        rows = render.render_preview(theme or Theme(), width, height, preview)
        self.assertEqual(len(rows), height)
        for row in rows:
            self.assertLessEqual(display_width(plain(row)), width, row)
        return [plain(row).rstrip() for row in rows]

    def test_heading_path_rule_then_what_is_inside(self):
        rows = self.lines(self.preview())
        self.assertEqual(rows[0], "api")
        self.assertEqual(rows[1], "~/work/api")
        self.assertTrue(rows[2].startswith("───"))
        self.assertEqual(rows[3:6], ["src/", "tests/", "README.md"])

    def test_dot_files_only_when_hidden_ones_are_offered(self):
        self.assertNotIn(".env", self.lines(self.preview()))
        rows = self.lines(self.preview(hidden=True))
        self.assertIn(".github/", rows)
        self.assertIn(".env", rows)

    def test_the_facts_come_before_the_listing(self):
        rows = self.lines(
            self.preview(
                branch="main",
                space="Atlas",
                agents=[FolderAgent("claude", "claude", "working"),
                        FolderAgent("codex", "api", "blocked")],
            )
        )
        self.assertEqual(rows[3], "branch  main")
        self.assertEqual(rows[4], "space   Atlas")
        self.assertEqual(rows[5], "agents  ◐ claude  working")
        self.assertEqual(rows[6], "        ◉ api  needs you")
        self.assertEqual(rows[7], "")
        self.assertEqual(rows[8], "src/")

    def test_what_does_not_fit_is_counted(self):
        many = Contents(tuple("d%02d" % i for i in range(30)), ())
        rows = self.lines(self.preview(contents=many), height=10)
        self.assertEqual(rows[3:9], ["d%02d/" % i for i in range(6)])
        self.assertEqual(rows[9], "… 24 more")

    def test_empty_missing_and_unreadable_say_which(self):
        self.assertEqual(self.lines(self.preview(contents=Contents((), ())))[3], "empty")
        self.assertEqual(self.lines(self.preview(contents=None))[3], "not a folder yet")
        unreadable = Contents((), (), readable=False)
        self.assertEqual(self.lines(self.preview(contents=unreadable))[3], "not readable")

    def test_nothing_to_preview_is_blank(self):
        self.assertEqual(self.lines(None), [""] * 20)

    def test_an_agent_logo_leads_its_name_with_the_font(self):
        theme = Theme()
        theme.agent_icons = True
        rows = self.lines(
            self.preview(agents=[FolderAgent("claude", "claude", "idle")]), theme=theme
        )
        self.assertIn(theme.logo("claude") + "  claude", rows[3])


class RuleJunctionTest(unittest.TestCase):
    def test_a_junction_lands_on_its_column(self):
        line = plain(render.render_rule(Theme(), 10, 4, "┬"))
        self.assertEqual(line, "────┬─────")
        self.assertEqual(plain(render.render_rule(Theme(), 10)), "─" * 10)


class PickerPreviewTest(unittest.TestCase):
    def setUp(self):
        paths.clear_cache()
        self.addCleanup(paths.clear_cache)
        self.tree = Tree("api/src", "api/tests", "web")
        self.addCleanup(self.tree.close)
        touch(self.tree.path("api", "README.md"))
        self.items = [
            Suggestion(self.tree.path(name), name, SOURCE_ROOT) for name in ("api", "web")
        ]

    def frame(self, app, size=WIDE):
        return [plain(line) for line in app.frame(*size)]

    def test_on_by_default_at_the_shipped_size(self):
        app = picker(self.items)
        lines = self.frame(app)
        self.assertTrue(app._preview_showing)
        body = "\n".join(lines)
        self.assertIn("src/", body)
        self.assertIn("README.md", body)
        for line in lines:
            self.assertLessEqual(display_width(line), WIDE[0], line)

    def test_it_follows_the_selection(self):
        app = picker(self.items)
        self.frame(app)
        app.handle_key("down")
        body = "\n".join(self.frame(app))
        self.assertIn("empty", body)
        self.assertNotIn("README.md", body)

    def test_the_separator_meets_both_rules(self):
        app = picker(self.items)
        lines = self.frame(app)
        column = 1 + render.compute_layout(*WIDE, preview=True).list_width + 1
        self.assertEqual(lines[1][column], "┬")
        self.assertEqual(lines[2][column], "│")
        bottom = next(i for i in range(len(lines) - 1, 1, -1) if "┴" in lines[i])
        self.assertEqual(lines[bottom][column], "┴")

    def test_false_in_the_config_hides_it_until_ctrl_o(self):
        app = picker(self.items, config=Config({"roots": [], "preview": False}))
        self.assertNotIn("┬", self.frame(app)[1])
        app.handle_key("ctrl+o")
        self.assertIn("┬", self.frame(app)[1])
        app.handle_key("ctrl+o")
        self.assertNotIn("┬", self.frame(app)[1])

    def test_ctrl_o_brings_it_up_where_auto_would_not(self):
        size = (render.PREVIEW_AUTO - 1, 28)
        app = picker(self.items)
        self.assertNotIn("┬", self.frame(app, size)[1])
        app.handle_key("ctrl+o")
        self.assertIn("┬", self.frame(app, size)[1])

    def test_the_separator_is_the_scrollbar_beside_a_long_list(self):
        many = [Suggestion(self.tree.path("x%02d" % i), "x%02d" % i, SOURCE_ROOT)
                for i in range(30)]
        app = picker(many)
        lines = self.frame(app)
        column = 1 + render.compute_layout(*WIDE, preview=True).list_width + 1
        marks = [line[column] for line in lines[2:12]]
        self.assertIn(render.SCROLL_THUMB, marks)
        # One vertical line, not a scrollbar and a separator side by side.
        self.assertNotIn(render.SCROLL_TRACK + " " + render.SCROLL_TRACK, lines[11])

    def test_a_click_on_the_preview_or_the_air_under_the_list_selects_nothing(self):
        app = picker(self.items)
        self.frame(app)
        right = app._list_right + 3
        self.assertIsNone(app.handle_mouse(Event(MOUSE, PRESS, right, app._row_top + 2)))
        self.assertEqual(app.selected, 0)
        below = app._row_top + app._list_height + 2
        self.assertIsNone(app.handle_mouse(Event(MOUSE, PRESS, 4, below)))
        self.assertEqual(app.selected, 0)

    def test_it_shows_the_space_and_the_agents_in_the_folder(self):
        folder = self.tree.path("api")
        snap = snapshot([pane("w1:p1", folder)])
        snap["agents"] = [{"pane_id": "w1:p1", "agent": "claude", "agent_status": "blocked"}]
        app = picker(self.items, snap=snap)
        body = "\n".join(self.frame(app))
        self.assertIn("space   Atlas", body)
        self.assertIn("◉ claude  needs you", body)

    def test_creating_asks_the_filesystem_afresh(self):
        app = picker(self.items)
        self.frame(app)
        os.rmdir(self.tree.path("web"))
        app.handle_key("down")
        self.frame(app)
        self.assertIsNone(app.choice())


class ConfigTest(unittest.TestCase):
    def test_auto_true_and_false(self):
        self.assertEqual(Config().preview, "auto")
        self.assertIs(Config({"preview": True}).preview, True)
        self.assertIs(Config({"preview": False}).preview, False)
        self.assertEqual(Config({"preview": "sometimes"}).preview, "auto")


if __name__ == "__main__":
    unittest.main()
