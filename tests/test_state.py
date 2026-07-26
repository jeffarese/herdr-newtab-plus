from __future__ import annotations

import json
import os
import tempfile
import unittest

from herdr_newtab.state import MAX_ENTRIES, Recents


class RecentsTest(unittest.TestCase):
    def setUp(self):
        handle, self.path = tempfile.mkstemp(prefix="newtab-recent-", suffix=".json")
        os.close(handle)
        os.unlink(self.path)

    def tearDown(self):
        if os.path.exists(self.path):
            os.unlink(self.path)

    def test_most_recent_leads(self):
        recents = Recents(None)
        recents.touch("/a", "claude")
        recents.touch("/b", "codex")
        self.assertEqual(recents.folders(), ["/b", "/a"])

    def test_touching_again_moves_it_up_without_duplicating(self):
        recents = Recents(None)
        recents.touch("/a")
        recents.touch("/b")
        recents.touch("/a")
        self.assertEqual(recents.folders(), ["/a", "/b"])

    def test_the_agent_is_remembered_per_folder(self):
        recents = Recents(None)
        recents.touch("/a", "claude")
        recents.touch("/b", "codex")
        self.assertEqual(recents.agent_for("/a"), "claude")
        self.assertEqual(recents.agent_for("/b"), "codex")
        self.assertIsNone(recents.agent_for("/c"))

    def test_a_shell_pick_is_remembered_as_no_agent(self):
        recents = Recents(None)
        recents.touch("/a", "")
        self.assertIsNone(recents.agent_for("/a"))

    def test_last_agent_skips_folders_opened_as_a_shell(self):
        recents = Recents(None)
        recents.touch("/a", "claude")
        recents.touch("/b", "")
        self.assertEqual(recents.last_agent(), "claude")

    def test_paths_are_normalized_on_the_way_in(self):
        recents = Recents(None)
        recents.touch("/a/b/")
        self.assertEqual(recents.agent_for("/a/b"), None)
        self.assertEqual(recents.folders(), ["/a/b"])

    def test_the_list_is_capped(self):
        recents = Recents(None)
        for index in range(MAX_ENTRIES + 20):
            recents.touch("/f%d" % index)
        self.assertEqual(len(recents.entries), MAX_ENTRIES)

    def test_round_trips_through_a_file(self):
        recents = Recents(self.path)
        recents.touch("/a", "claude")
        recents.save()
        self.assertEqual(Recents(self.path).agent_for("/a"), "claude")

    def test_a_broken_file_is_ignored_rather_than_fatal(self):
        with open(self.path, "w", encoding="utf-8") as handle:
            handle.write("{not json")
        self.assertEqual(Recents(self.path).folders(), [])

    def test_junk_entries_are_dropped(self):
        with open(self.path, "w", encoding="utf-8") as handle:
            json.dump({"entries": [{"path": "/a"}, {"nope": 1}, "string"]}, handle)
        self.assertEqual(Recents(self.path).folders(), ["/a"])

    def test_saving_without_a_path_is_a_no_op(self):
        Recents(None).save()


if __name__ == "__main__":
    unittest.main()
