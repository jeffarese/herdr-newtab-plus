from __future__ import annotations

import os
import unittest

from herdr_newtab import agents
from herdr_newtab.agents import KNOWN, PREFERRED, Agent, agent_env, available, initial_index


class AvailableTest(unittest.TestCase):
    def test_a_plain_shell_always_leads(self):
        chips = available([])
        self.assertEqual(len(chips), 1)
        self.assertTrue(chips[0].is_none)

    def test_a_configured_list_is_taken_as_written(self):
        chips = available(["codex", "claude"])
        self.assertEqual([chip.kind for chip in chips], ["", "codex", "claude"])

    def test_kinds_herdr_does_not_know_are_dropped(self):
        self.assertEqual([chip.kind for chip in available(["nope", "claude"])], ["", "claude"])

    def test_detection_only_offers_what_is_installed(self):
        real = agents.installed
        try:
            agents.installed = lambda kind: kind in ("codex", "claude")
            chips = available("auto")
        finally:
            agents.installed = real
        self.assertEqual([chip.kind for chip in chips], ["", "claude", "codex"])

    def test_preferred_kinds_lead_and_the_rest_sort(self):
        real = agents.installed
        try:
            agents.installed = lambda kind: kind in ("maki", "claude", "agy")
            chips = available("auto")
        finally:
            agents.installed = real
        self.assertEqual([chip.kind for chip in chips], ["", "claude", "agy", "maki"])

    def test_every_preferred_kind_is_one_herdr_knows(self):
        for kind in PREFERRED:
            self.assertIn(kind, KNOWN)


class InitialIndexTest(unittest.TestCase):
    def setUp(self):
        self.chips = [Agent("", "shell"), Agent("claude", "claude"), Agent("codex", "codex")]

    def test_last_uses_what_the_folder_remembers(self):
        self.assertEqual(initial_index(self.chips, "last", "codex"), 2)

    def test_last_with_nothing_remembered_falls_back_to_the_shell(self):
        self.assertEqual(initial_index(self.chips, "last", None), 0)

    def test_a_remembered_agent_that_is_gone_falls_back(self):
        self.assertEqual(initial_index(self.chips, "last", "gemini"), 0)

    def test_none_pins_the_shell(self):
        self.assertEqual(initial_index(self.chips, "none", "codex"), 0)

    def test_a_named_default_wins(self):
        self.assertEqual(initial_index(self.chips, "claude", "codex"), 1)


class AgentEnvTest(unittest.TestCase):
    def test_plugin_variables_do_not_leak_into_the_child(self):
        os.environ["HERDR_PLUGIN_ID"] = "herdr-newtab-plus"
        os.environ["HERDR_PANE_ID"] = "w1:p1"
        os.environ["HERDR_SOCKET_PATH"] = "/tmp/sock"
        try:
            env = agent_env()
        finally:
            for name in ("HERDR_PLUGIN_ID", "HERDR_PANE_ID", "HERDR_SOCKET_PATH"):
                os.environ.pop(name, None)
        self.assertNotIn("HERDR_PLUGIN_ID", env)
        self.assertNotIn("HERDR_PANE_ID", env)
        # The socket is how the child reaches Herdr, so it has to survive.
        self.assertEqual(env["HERDR_SOCKET_PATH"], "/tmp/sock")


if __name__ == "__main__":
    unittest.main()
