from __future__ import annotations

import unittest

from herdr_newtab.agents import Agent
from herdr_newtab.app import Choice
from herdr_newtab.cli import _transient, create
from herdr_newtab.client import HerdrError
from herdr_newtab.session import SPACE, TAB, Target


class FakeClient(object):
    def __init__(self, pane="w1:p9"):
        self.pane = pane
        self.calls = []

    def create_tab(self, workspace_id, cwd, label=None):
        self.calls.append(("tab", workspace_id, cwd, label))
        return {"tab": {"tab_id": "w1:t9"}, "root_pane": {"pane_id": self.pane}}

    def create_workspace(self, cwd, label=None):
        self.calls.append(("space", cwd, label))
        return {"tab": {"tab_id": "w9:t1"}, "root_pane": {"pane_id": self.pane}}


SHELL = Agent("", "shell")


class CreateTest(unittest.TestCase):
    def test_a_tab_target_creates_a_tab_in_that_space(self):
        client = FakeClient()
        pane = create(client, Choice("/a/x", SHELL, Target(TAB, "/a/x", "w1", "One")))
        self.assertEqual(pane, "w1:p9")
        self.assertEqual(client.calls, [("tab", "w1", "/a/x", None)])

    def test_a_space_target_creates_a_space_labelled_by_the_folder(self):
        client = FakeClient()
        create(client, Choice("/a/my-thing", SHELL, Target(SPACE, "/a/my-thing")))
        self.assertEqual(client.calls, [("space", "/a/my-thing", "my-thing")])

    def test_a_tab_target_without_a_space_falls_back_to_creating_one(self):
        client = FakeClient()
        create(client, Choice("/a/x", SHELL, Target(TAB, "/a/x", None, None)))
        self.assertEqual(client.calls[0][0], "space")

    def test_a_reply_with_no_pane_is_an_error_rather_than_a_silent_pass(self):
        class NoPane(FakeClient):
            def create_workspace(self, cwd, label=None):
                return {"tab": {"tab_id": "w9:t1"}}

        with self.assertRaises(HerdrError):
            create(NoPane(), Choice("/a/x", SHELL, Target(SPACE, "/a/x")))


class TransientTest(unittest.TestCase):
    def test_a_shell_that_is_still_starting_is_worth_waiting_for(self):
        self.assertTrue(_transient("agent target pane w3:p2 is not an available shell"))

    def test_a_real_refusal_is_not_retried(self):
        self.assertFalse(_transient("unknown agent kind: nope"))
        self.assertFalse(_transient("agent name already in use"))


if __name__ == "__main__":
    unittest.main()
