"""What the picker remembers between opens.

Persisted under the plugin state directory. Three jobs: order the resting list
so the folder you keep coming back to is the first Enter, remember which agent
that folder wants so the chip row is already right when the picker opens, and
hold the create mode the toggle was last left on.

The mode is one value for the whole picker rather than one per folder, because
it is a preference about how you like new tabs to land -- something you should be
able to read off the screen the moment the popup opens, not discover by
selecting the folder that happens to carry it.
"""

from __future__ import annotations

import json
import os
import tempfile
import time
from typing import Dict, List, Optional

from .session import MODES, normalize

MAX_ENTRIES = 80


class Recents(object):
    def __init__(self, path: Optional[str]) -> None:
        self.path = path
        self.entries: List[Dict[str, object]] = []
        self._create: Optional[str] = None
        # Writing the file is skipped unless something actually changed, so the
        # picker can save on the way out of either exit without a cancel
        # rewriting the same bytes.
        self._dirty = False
        self._load()

    def _load(self) -> None:
        if not self.path or not os.path.exists(self.path):
            return
        try:
            with open(self.path, "r", encoding="utf-8") as handle:
                data = json.load(handle)
        except (OSError, ValueError):
            return
        if not isinstance(data, dict):
            return
        create = data.get("create")
        if create in MODES:
            self._create = str(create)
        entries = data.get("entries")
        if not isinstance(entries, list):
            return
        cleaned: List[Dict[str, object]] = []
        for entry in entries:
            if not isinstance(entry, dict):
                continue
            path = entry.get("path")
            if not isinstance(path, str) or not path:
                continue
            agent = entry.get("agent")
            cleaned.append(
                {
                    "path": normalize(path),
                    "agent": agent if isinstance(agent, str) else "",
                    "ts": entry.get("ts") if isinstance(entry.get("ts"), (int, float)) else 0,
                }
            )
        self.entries = cleaned[:MAX_ENTRIES]

    def folders(self) -> List[str]:
        """Recent folders, most recent first."""
        return [str(entry["path"]) for entry in self.entries]

    def agent_for(self, folder: str) -> Optional[str]:
        """The agent last started in this folder, or None."""
        target = normalize(folder)
        for entry in self.entries:
            if entry["path"] == target:
                agent = entry.get("agent")
                return str(agent) if agent else None
        return None

    def last_agent(self) -> Optional[str]:
        """The most recently started agent anywhere -- the default for a new folder."""
        for entry in self.entries:
            agent = entry.get("agent")
            if agent:
                return str(agent)
        return None

    def create_mode(self) -> Optional[str]:
        """The mode the toggle was last left on, or None if it never was."""
        return self._create

    def remember_create_mode(self, mode: str) -> None:
        if mode not in MODES or mode == self._create:
            return
        self._create = mode
        self._dirty = True

    def touch(self, folder: str, agent: str = "") -> None:
        target = normalize(folder)
        if not target:
            return
        self.entries = [entry for entry in self.entries if entry["path"] != target]
        self.entries.insert(0, {"path": target, "agent": agent, "ts": int(time.time())})
        del self.entries[MAX_ENTRIES:]
        self._dirty = True

    def save(self) -> None:
        if not self.path or not self._dirty:
            return
        directory = os.path.dirname(self.path)
        try:
            if directory:
                os.makedirs(directory, exist_ok=True)
            handle = tempfile.NamedTemporaryFile(
                "w",
                encoding="utf-8",
                dir=directory or ".",
                delete=False,
                prefix=".recent-",
                suffix=".json",
            )
            try:
                payload: Dict[str, object] = {"version": 1, "entries": self.entries}
                if self._create:
                    payload["create"] = self._create
                json.dump(payload, handle)
                handle.flush()
                os.fsync(handle.fileno())
            finally:
                handle.close()
            os.replace(handle.name, self.path)
            self._dirty = False
        except OSError:
            # Losing the recent list is not worth failing a create over.
            pass
