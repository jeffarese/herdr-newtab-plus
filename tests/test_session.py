from __future__ import annotations

import os
import unittest

from herdr_newtab.session import (
    SPACE,
    TAB,
    agent_name,
    created_ids,
    focused_cwd,
    live_agent_names,
    normalize,
    resolve_target,
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
        self.assertEqual(space_folders(snap)["/a"].label, "WeekGen")

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
        self.assertEqual(target.workspace_label, "WeekGen")

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


if __name__ == "__main__":
    unittest.main()
