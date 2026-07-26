"""User configuration.

Read from ``$HERDR_PLUGIN_CONFIG_DIR/config.json`` -- JSON rather than TOML
because ``tomllib`` only arrived in 3.11, and the picker targets 3.9 without
taking on a parser dependency. Unknown keys are ignored, and a broken file
falls back to defaults instead of taking the picker down.
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, List, Optional

from .session import MODES, SMART

# Scanned one level deep for folders to offer before anything is typed. Only
# the ones that exist are used, so the default list can be generous.
DEFAULT_ROOTS = ["~/workspace", "~/projects", "~/src", "~/code", "~/dev", "~/repos"]

DEFAULTS: Dict[str, Any] = {
    "roots": DEFAULT_ROOTS,
    # The starting point only: the picker's own toggle overrides it, and what it
    # was left on is remembered in the state file.
    "create": SMART,  # "smart" | "tab" | "space"
    "agents": "auto",  # "auto" (detect on PATH) | ["claude", "codex", ...]
    "default_agent": "last",  # "last" | "none" | an agent kind
    # kind -> argv that starts that agent with its permission prompts off, e.g.
    # {"gemini": ["--yolo"]}. Merged over the built-in table, so an entry here
    # both teaches the toggle a new agent and corrects one it already knows.
    "skip_permission_flags": {},
    "zoxide": "auto",  # true | false | "auto" (on when zoxide is installed)
    "hidden": False,  # offer dot-directories during path completion
    "mouse": True,
    "max_suggestions": 200,
    "colors": {},  # role -> "#rrggbb" | ansi name | 0-255
    "selection_background": "auto",  # "auto" | "none" | "#rrggbb" | 0-255
}


class Config(object):
    def __init__(self, values: Optional[Dict[str, Any]] = None) -> None:
        merged = dict(DEFAULTS)
        merged.update(values or {})

        roots = merged.get("roots")
        self.roots: List[str] = (
            [r for r in roots if isinstance(r, str)] if isinstance(roots, list) else DEFAULT_ROOTS
        )

        create = merged.get("create")
        self.create = create if create in MODES else SMART

        agents = merged.get("agents")
        self.agents = (
            [a for a in agents if isinstance(a, str)] if isinstance(agents, list) else "auto"
        )

        default_agent = merged.get("default_agent")
        self.default_agent = default_agent if isinstance(default_agent, str) else "last"

        # A malformed entry is dropped rather than repaired: the values become
        # argv for a real agent, so a half-understood one is worse than none.
        flags = merged.get("skip_permission_flags")
        self.skip_permission_flags: Dict[str, List[str]] = {}
        if isinstance(flags, dict):
            for kind, argv in flags.items():
                if not isinstance(kind, str) or not isinstance(argv, (list, tuple)):
                    continue
                if all(isinstance(flag, str) for flag in argv):
                    self.skip_permission_flags[kind] = [flag for flag in argv if flag]

        self.zoxide = merged.get("zoxide", "auto")
        self.hidden = bool(merged.get("hidden", False))
        self.mouse = bool(merged.get("mouse", True))

        try:
            self.max_suggestions = max(20, int(merged.get("max_suggestions", 200)))
        except (TypeError, ValueError):
            self.max_suggestions = 200

        colors = merged.get("colors")
        self.colors = colors if isinstance(colors, dict) else {}
        self.selection_background = merged.get("selection_background", "auto")

    @staticmethod
    def path(config_dir: Optional[str] = None) -> Optional[str]:
        directory = config_dir
        if directory is None:
            directory = os.environ.get("HERDR_PLUGIN_CONFIG_DIR")
        if not directory:
            return None
        return os.path.join(directory, "config.json")

    @classmethod
    def load(cls, config_dir: Optional[str] = None) -> "Config":
        path = cls.path(config_dir)
        if not path or not os.path.exists(path):
            return cls()
        try:
            with open(path, "r", encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            return cls()
        if not isinstance(data, dict):
            return cls()
        return cls(data)

    def wants_zoxide(self, installed: bool) -> bool:
        if self.zoxide is True:
            return True
        if self.zoxide is False:
            return False
        return installed
