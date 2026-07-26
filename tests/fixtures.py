"""Shared fixtures: a fake session, and a real directory tree to complete against."""

from __future__ import annotations

import os
import shutil
import tempfile
from typing import Any, Dict, List

from herdr_newtab.agents import Agent
from herdr_newtab.app import Picker
from herdr_newtab.config import Config
from herdr_newtab.state import Recents
from herdr_newtab.theme import Theme


def snapshot(
    panes: List[Dict[str, Any]], workspaces: List[Dict[str, Any]] = None
) -> Dict[str, Any]:
    return {
        "version": "0.7.5",
        "focused_workspace_id": "w1",
        "focused_pane_id": panes[0]["pane_id"] if panes else None,
        "workspaces": workspaces
        if workspaces is not None
        else [{"workspace_id": "w1", "label": "WeekGen", "number": 1, "focused": True}],
        "panes": panes,
        "agents": [],
    }


def pane(pane_id: str, cwd: str, workspace_id: str = "w1") -> Dict[str, Any]:
    return {"pane_id": pane_id, "workspace_id": workspace_id, "cwd": cwd}


CHIPS = [
    Agent("", "shell"),
    Agent("claude", "claude"),
    Agent("codex", "codex"),
    Agent("pi", "pi"),
]


class Tree(object):
    """A throwaway directory tree, so path completion is tested against a real one."""

    def __init__(self, *names: str) -> None:
        self.root = os.path.realpath(tempfile.mkdtemp(prefix="newtab-test-"))
        for name in names:
            os.makedirs(os.path.join(self.root, name), exist_ok=True)

    def path(self, *parts: str) -> str:
        return os.path.join(self.root, *parts)

    def close(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


def picker(
    catalog=(),
    base: str = "/tmp",
    snap: Dict[str, Any] = None,
    config: Config = None,
    recents: Recents = None,
) -> Picker:
    return Picker(
        snapshot=snap if snap is not None else snapshot([pane("w1:p1", "/nowhere")]),
        catalog=list(catalog),
        chips=CHIPS,
        config=config or Config({"roots": []}),
        recents=recents or Recents(None),
        theme=Theme(),
        base=base,
    )
