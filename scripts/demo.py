#!/usr/bin/env python3
"""Render the picker against a fixed session, with no Herdr and no pty.

Useful for looking at the layout while changing it, and for CI to prove the
frame still composes on every supported Python.

    python scripts/demo.py --size 92x20
    python scripts/demo.py --plain --query '~/workspace/atlas'
"""

from __future__ import annotations

import argparse
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from herdr_newtab.agents import Agent  # noqa: E402
from herdr_newtab.app import Picker  # noqa: E402
from herdr_newtab.config import Config  # noqa: E402
from herdr_newtab.paths import (  # noqa: E402
    SOURCE_RECENT,
    SOURCE_ROOT,
    SOURCE_SPACE,
    SOURCE_ZOXIDE,
    Suggestion,
)
from herdr_newtab.session import MODES  # noqa: E402
from herdr_newtab.state import Recents  # noqa: E402
from herdr_newtab.textutil import strip_ansi  # noqa: E402
from herdr_newtab.theme import Theme  # noqa: E402

HOME = os.path.expanduser("~")


def catalog():
    def folder(name, source, detail="", is_git=False):
        return Suggestion(
            path=os.path.join(HOME, "workspace", name),
            name=name,
            source=source,
            detail=detail,
            is_git=is_git,
        )

    return [
        folder("atlas", SOURCE_SPACE, "Atlas"),
        folder("herdr-bar", SOURCE_SPACE, "Herdr", is_git=True),
        folder("herdr-newtab-plus", SOURCE_RECENT, is_git=True),
        folder("mobile-client", SOURCE_ZOXIDE, is_git=True),
        folder("notes", SOURCE_ROOT, is_git=True),
        folder("classifier", SOURCE_ROOT),
        folder("log-sink", SOURCE_ROOT, is_git=True),
        folder("webhook-relay", SOURCE_ROOT, is_git=True),
    ]


def snapshot():
    """Two spaces, each with an agent in it: one working, one waiting on you."""
    return {
        "version": "0.7.5",
        "focused_workspace_id": "w1",
        "focused_pane_id": "w1:p5P",
        "workspaces": [
            {"workspace_id": "w1", "label": "Atlas", "number": 1, "focused": True},
            {"workspace_id": "w2", "label": "Herdr", "number": 2, "focused": False},
        ],
        "panes": [
            {
                "pane_id": "w1:p5P",
                "workspace_id": "w1",
                "cwd": os.path.join(HOME, "workspace", "atlas"),
            },
            {
                "pane_id": "w2:p1A",
                "workspace_id": "w2",
                "cwd": os.path.join(HOME, "workspace", "herdr-bar"),
            },
        ],
        "agents": [
            {"pane_id": "w1:p5P", "agent": "claude", "agent_status": "working"},
            {"pane_id": "w2:p1A", "agent": "codex", "agent_status": "blocked"},
        ],
    }


def recents():
    """What the picker remembers: which agent each folder was last opened with."""
    remembered = Recents(None)
    for name, agent in (
        ("herdr-newtab-plus", "claude"),
        ("atlas", "claude"),
        ("herdr-bar", "codex"),
    ):
        remembered.entries.append(
            {"path": os.path.join(HOME, "workspace", name), "agent": agent, "ts": 0}
        )
    return remembered


def chips():
    return [
        Agent("", "shell"),
        Agent("claude", "claude"),
        Agent("codex", "codex"),
        Agent("opencode", "opencode"),
        Agent("pi", "pi"),
    ]


def build(
    query: str, selected: int, chip: int, create: str = "smart", icons: bool = False
) -> Picker:
    theme = Theme(selection_background="237")
    theme.agent_icons = icons
    picker = Picker(
        snapshot=snapshot(),
        catalog=catalog(),
        chips=chips(),
        config=Config({"roots": []}),
        recents=recents(),
        theme=theme,
        base=os.path.join(HOME, "workspace"),
    )
    # The demo drives the catalog directly rather than the filesystem, so a
    # path-shaped query is filtered here instead of being completed for real.
    picker.query = query
    picker.cursor = len(query)
    if query:
        from herdr_newtab.paths import filter_catalog

        picker.rows = filter_catalog(picker.catalog, query, 200)
    picker.selected = min(selected, max(0, len(picker.rows) - 1))
    picker.chip = chip
    picker._chip_pinned = True
    picker.create_mode = create
    return picker


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--size", default="92x20", help="WIDTHxHEIGHT")
    parser.add_argument("--query", default="")
    parser.add_argument("--selected", type=int, default=0)
    parser.add_argument("--chip", type=int, default=1)
    parser.add_argument(
        "--create", default="smart", choices=MODES, help="where Enter lands (^T in the picker)"
    )
    parser.add_argument("--panel", action="store_true", help="the ? help panel instead")
    parser.add_argument(
        "--icons", action="store_true", help="vendor logos (needs the Herdr Agent Icons Max font)"
    )
    parser.add_argument("--plain", action="store_true", help="strip colors")
    parser.add_argument("--frame", action="store_true", help="draw a box around it")
    args = parser.parse_args()

    width, height = (int(part) for part in args.size.lower().split("x"))
    picker = build(args.query, args.selected, args.chip, args.create, args.icons)
    picker.help = args.panel
    lines = picker.frame(width, height)
    lines += [""] * (height - len(lines))

    for line in lines:
        text = strip_ansi(line) if args.plain else line
        if args.frame:
            from herdr_newtab.textutil import pad

            sys.stdout.write("│" + pad(text, width) + ("│" if args.plain else "\x1b[0m│") + "\n")
        else:
            sys.stdout.write(text + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())
