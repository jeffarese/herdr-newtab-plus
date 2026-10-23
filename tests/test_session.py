from __future__ import annotations

import os
import unittest

from herdr_newtab import paths
from herdr_newtab.session import (
    SMART,
    SPACE,
    TAB,
    agent_name,
    clear_resolved,
    created_ids,
    focused_cwd,
    folder_statuses,
    live_agent_names,
    next_mode,
    normalize,
    resolve_target,
    resolved,
    space_folders,
    spaces,
)

from .fixtures import Tree, pane, snapshot


class NormalizeTest(unittest.TestCase):
    def test_expands_and_strips(self):
        home = os.path.expanduser("~")
        self.assertEqual(normalize("~/x/"), os.path.join(home, "x"))
        self.assertEqual(normalize("  ~  "), home)
        self.assertEqual(normalize("/"), "/")

    def test_empty_stays_empty(self):
        self.assertEqual(normalize(""), "")
        self.assertEqual(normalize("   "), "")


class SpacesTest(unittest.TestCase):
    def test_folders_come_from_panes(self):
        snap = snapshot([pane("w1:p1", "/a"), pane("w1:p2", "/b")])
        self.assertEqual(sorted(space_folders(snap)), ["/a", "/b"])
        self.assertEqual(space_folders(snap)["/a"].label, "Atlas")

    def test_focused_space_wins_a_shared_folder(self):
        snap = snapshot(
            [pane("w1:p1", "/a", "w1"), pane("w2:p1", "/a", "w2")],
            [
                {"workspace_id": "w1", "label": "one", "number": 1, "focused": False},
                {"workspace_id": "w2", "label": "two", "number": 2, "focused": True},
            ],
        )
        self.assertEqual(space_folders(snap)["/a"].label, "two")

    def test_trailing_separator_does_not_split_a_folder(self):
        snap = snapshot([pane("w1:p1", "/a/"), pane("w1:p2", "/a")])
        self.assertEqual(list(space_folders(snap)), ["/a"])

    def test_panes_without_a_known_space_are_skipped(self):
        snap = snapshot([pane("w9:p1", "/a", "w9")])
        self.assertEqual(space_folders(snap), {})

    def test_focused_cwd_prefers_the_focused_pane(self):
        snap = snapshot([pane("w1:p1", "/a"), pane("w1:p2", "/b")])
        snap["focused_pane_id"] = "w1:p2"
        self.assertEqual(focused_cwd(snap), "/b")

    def test_spaces_survive_a_missing_label(self):
        snap = snapshot([], [{"workspace_id": "w1", "number": 1, "focused": True}])
        self.assertEqual(spaces(snap)[0].label, "w1")


class ResolveTargetTest(unittest.TestCase):
    def setUp(self):
        self.snap = snapshot([pane("w1:p1", "/open")])

    def test_smart_reuses_a_space_standing_in_the_folder(self):
        target = resolve_target("/open", self.snap, "smart")
        self.assertEqual(target.kind, TAB)
        self.assertEqual(target.workspace_id, "w1")
        self.assertEqual(target.workspace_label, "Atlas")

    def test_smart_creates_a_space_for_anything_else(self):
        target = resolve_target("/elsewhere", self.snap, "smart")
        self.assertEqual(target.kind, SPACE)
        self.assertIsNone(target.workspace_id)
        self.assertEqual(target.workspace_label, "elsewhere")

    def test_tab_mode_falls_back_to_the_focused_space(self):
        target = resolve_target("/elsewhere", self.snap, "tab")
        self.assertEqual(target.kind, TAB)
        self.assertEqual(target.workspace_id, "w1")

    def test_tab_mode_creates_a_space_when_there_are_none(self):
        empty = snapshot([], [])
        empty["focused_workspace_id"] = None
        self.assertEqual(resolve_target("/x", empty, "tab").kind, SPACE)

    def test_space_mode_never_reuses(self):
        self.assertEqual(resolve_target("/open", self.snap, "space").kind, SPACE)

    def test_space_mode_still_names_the_space_it_would_make(self):
        # The line above the hints reads off this label, so an insisted-on space
        # has to carry one just as an inferred one does.
        self.assertEqual(resolve_target("/open", self.snap, "space").workspace_label, "open")

    def test_an_unknown_mode_behaves_as_smart_does(self):
        self.assertEqual(resolve_target("/open", self.snap, "sideways").kind, TAB)

    def test_folder_is_normalized_on_the_way_in(self):
        self.assertEqual(resolve_target("/open/", self.snap, "smart").kind, TAB)

    def test_a_symlinked_folder_finds_the_space_it_already_has(self):
        # A shell resolves its own cwd, so the pane reports the real path while
        # the picker offers the link. Both have to land on the same space, or
        # picking the folder twice silently opens two.
        tree = Tree("real")
        try:
            link = os.path.join(tree.root, "link")
            os.symlink(tree.path("real"), link)
            snap = snapshot([pane("w1:p1", tree.path("real"))])
            self.assertEqual(resolve_target(link, snap, "smart").kind, TAB)

            # ...and the other way round, when the pane is the one on the link.
            snap = snapshot([pane("w1:p1", link)])
            self.assertEqual(resolve_target(tree.path("real"), snap, "smart").kind, TAB)
        finally:
            tree.close()

    def test_the_picked_spelling_is_what_gets_created(self):
        tree = Tree("real")
        try:
            link = os.path.join(tree.root, "link")
            os.symlink(tree.path("real"), link)
            target = resolve_target(link, snapshot([]), "smart")
            self.assertEqual(target.folder, link)
        finally:
            tree.close()


class NextModeTest(unittest.TestCase):
    def test_it_cycles_decide_for_me_then_each_override(self):
        self.assertEqual(next_mode(SMART), TAB)
        self.assertEqual(next_mode(TAB), SPACE)
        self.assertEqual(next_mode(SPACE), SMART)

    def test_an_unrecognised_mode_lands_where_smart_would(self):
        self.assertEqual(next_mode("sideways"), TAB)


class AgentNameTest(unittest.TestCase):
    def test_derives_from_the_folder(self):
        self.assertEqual(agent_name("/a/my-project", "claude"), "my-project")

    def test_illegal_characters_become_hyphens(self):
        self.assertEqual(agent_name("/a/My Project!", "claude"), "my-project")

    def test_a_leading_non_letter_is_repaired(self):
        name = agent_name("/a/2024-notes", "claude")
        self.assertTrue(name[0].isalpha(), name)

    def test_avoids_taken_names(self):
        self.assertEqual(agent_name("/a/api", "claude", ["api"]), "api-2")
        self.assertEqual(agent_name("/a/api", "claude", ["api", "api-2"]), "api-3")

    def test_stays_within_herdrs_limit(self):
        name = agent_name("/a/" + "x" * 80, "claude", ["x" * 32])
        self.assertLessEqual(len(name), 32)

    def test_a_nameless_folder_still_produces_something_legal(self):
        self.assertTrue(agent_name("/", "codex"))


class SnapshotFieldsTest(unittest.TestCase):
    def test_live_agent_names_reads_only_named_agents(self):
        snap = snapshot([])
        snap["agents"] = [{"agent": "claude"}, {"agent": "codex", "name": "api"}]
        self.assertEqual(live_agent_names(snap), {"api"})

    def test_created_ids_reads_both_shapes(self):
        result = {"tab": {"tab_id": "w1:t1"}, "root_pane": {"pane_id": "w1:p1"}}
        self.assertEqual(created_ids(result), ("w1:t1", "w1:p1"))
        self.assertEqual(created_ids({}), (None, None))


class FolderStatusTest(unittest.TestCase):
    def snap(self, agents, panes=None):
        snap = snapshot(panes or [pane("w1:p1", "/work/api"), pane("w1:p2", "/work/web")])
        snap["agents"] = agents
        return snap

    def test_an_agent_marks_the_folder_its_pane_is_standing_in(self):
        found = folder_statuses(self.snap([{"pane_id": "w1:p1", "agent_status": "working"}]))
        self.assertEqual(found.get("/work/api"), "working")
        self.assertNotIn("/work/web", found)

    def test_the_most_urgent_status_wins(self):
        found = folder_statuses(
            self.snap(
                [
                    {"pane_id": "w1:p1", "agent_status": "idle"},
                    {"pane_id": "w1:p2", "agent_status": "working", "cwd": "/work/api"},
                    {"pane_id": "w1:p1", "agent_status": "blocked"},
                ]
            )
        )
        self.assertEqual(found.get("/work/api"), "blocked")

    def test_an_agents_own_cwd_beats_its_panes(self):
        found = folder_statuses(
            self.snap([{"pane_id": "w1:p1", "agent_status": "done", "cwd": "/work/web/"}])
        )
        self.assertEqual(found.get("/work/web"), "done")
        self.assertNotIn("/work/api", found)

    def test_a_status_herdr_does_not_report_is_left_out(self):
        found = folder_statuses(
            self.snap(
                [
                    {"pane_id": "w1:p1", "agent_status": "none"},
                    {"pane_id": "w1:p2"},
                    {"pane_id": "w1:p9", "agent_status": "working"},  # no such pane
                    "junk",
                ]
            )
        )
        self.assertEqual(found, {})

    def test_a_pane_that_reports_an_agent_status_counts_too(self):
        panes = [dict(pane("w1:p1", "/work/api"), agent_status="blocked")]
        self.assertEqual(folder_statuses(self.snap([], panes)).get("/work/api"), "blocked")


class ResolvedTest(unittest.TestCase):
    """Following a symlink is remembered, and the rescan key is how you forget."""

    def setUp(self):
        self.tree = Tree("real")
        self.link = self.tree.path("link")
        os.symlink(self.tree.path("real"), self.link)
        paths.clear_cache()

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def _realpath_calls(self, action):
        seen = []
        original = os.path.realpath

        def counted(path):
            seen.append(path)
            return original(path)

        os.path.realpath = counted
        try:
            action()
        finally:
            os.path.realpath = original
        return seen

    def test_follows_a_symlink(self):
        self.assertEqual(resolved(self.link), self.tree.path("real"))

    def test_the_same_folder_is_only_followed_once(self):
        seen = self._realpath_calls(lambda: [resolved(self.link) for _ in range(5)])
        self.assertEqual(len(seen), 1)

    def test_a_remembered_answer_is_the_same_answer(self):
        first = resolved(self.link)
        self.assertEqual(resolved(self.link), first)

    def test_different_folders_are_followed_separately(self):
        seen = self._realpath_calls(
            lambda: [resolved(self.link), resolved(self.tree.path("real"))]
        )
        self.assertEqual(len(seen), 2)

    def test_the_rescan_key_makes_it_ask_again(self):
        # ctrl+r goes through paths.clear_cache, so that has to reach this too
        # or a repointed symlink would stay wrong for the life of the popup.
        resolved(self.link)
        paths.clear_cache()
        seen = self._realpath_calls(lambda: resolved(self.link))
        self.assertEqual(len(seen), 1)

    def test_clearing_picks_up_a_symlink_that_moved(self):
        os.makedirs(self.tree.path("other"))
        self.assertEqual(resolved(self.link), self.tree.path("real"))
        os.unlink(self.link)
        os.symlink(self.tree.path("other"), self.link)
        clear_resolved()
        self.assertEqual(resolved(self.link), self.tree.path("other"))

    def test_an_empty_path_is_never_cached_as_an_answer(self):
        self.assertEqual(resolved(""), "")
        self.assertEqual(resolved("   "), "")


if __name__ == "__main__":
    unittest.main()
