#!/usr/bin/env python3
"""Hold two builds of the picker side by side and show where they differ.

    python3 scripts/parity.py --rust target/release/herdr-newtab-plus   # Python vs Rust
    python3 scripts/parity.py --rust ... --only pick-path                 # some scenarios
    python3 scripts/parity.py --save tests/parity/golden.json            # record Python
    python3 scripts/parity.py --check tests/parity/golden.json --rust bin/herdr-newtab-plus

The Rust port has to be a drop-in replacement: same screens, same Herdr
calls, same files on disk, same output. This harness drives each build as a
black box, the way Herdr does -- a real process on a pseudo-terminal, a fake
Herdr on a Unix socket, fake agents and a fake zoxide on PATH so every run sees
the same machine -- and records a transcript per scenario:

* the screen after every step, cell by cell with its rendition,
* every Herdr request that changes something, with its parameters,
* the files the plugin left in its state directory,
* exit status and whatever it printed outside the alt screen.

Two transcripts that are equal mean the user could not tell the builds apart.
POSIX only.
"""

from __future__ import annotations

import copy
import difflib
import errno
import fcntl
import json
import os
import pty
import re
import select
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
import unicodedata
from typing import Any, Callable, Dict, List, Optional, Sequence, Tuple

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PYTHON_CMD = [sys.executable, os.path.join(ROOT, "run.py")]
SIZE = (107, 28)  # what the manifest's 110x30 popup gives the pty
SETTLE = 0.3
EXIT_TIMEOUT = 10.0
AGENT_TIMEOUT = 6.0

DARK = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"
LIGHT = b"\x1b]11;rgb:fafa/fafa/f0f0\x07"
QUERY = b"\x1b]11;?"

ESC = b"\x1b"
ENTER = b"\r"
TAB = b"\t"
SHIFT_TAB = b"\x1b[Z"
UP = b"\x1b[A"
DOWN = b"\x1b[B"
RIGHT = b"\x1b[C"
LEFT = b"\x1b[D"
HOME = b"\x1b[H"
END = b"\x1b[F"
PGUP = b"\x1b[5~"
PGDN = b"\x1b[6~"
DELETE = b"\x1b[3~"
BS = b"\x7f"
F1 = b"\x1bOP"
F2 = b"\x1bOQ"


def ctrl(letter: str) -> bytes:
    return bytes([ord(letter.lower()) - 96])


def click(x: int, y: int, button: int = 0) -> bytes:
    return b"\x1b[<%d;%d;%dM\x1b[<%d;%d;%dm" % (button, x, y, button, x, y)


def wheel(x: int, y: int, down: bool) -> bytes:
    return b"\x1b[<%d;%d;%dM" % (65 if down else 64, x, y)


def paste(text: str) -> bytes:
    return b"\x1b[200~" + text.encode() + b"\x1b[201~"


def keys(*items: Any) -> List[Tuple[str, Any]]:
    """Steps: bytes are sent as one write, str is typed one key at a time."""
    steps: List[Tuple[str, Any]] = []
    for item in items:
        if isinstance(item, tuple):
            steps.append(item)
        elif isinstance(item, str):
            for char in item:
                steps.append(("keys", char.encode()))
        else:
            steps.append(("keys", item))
    return steps


def resize(width: int, height: int) -> Tuple[str, Any]:
    return ("resize", (width, height))


def wait(seconds: float) -> Tuple[str, Any]:
    return ("wait", seconds)


# -- the machine both builds see ---------------------------------------------

FOLDERS = [
    "atlas", "atlas-web", "beacon", "cargo-lab", "dotfiles", "harbor", "herdr-bar",
    "herdr-newtab-plus", "kestrel", "lighthouse", "meridian", "nimbus", "orchard",
    "pier", "quartz", "relay", "sable", "tern", "umbra", "vesper", "willow", "zephyr",
]
GIT = {"atlas", "atlas-web", "beacon", "harbor", "herdr-bar", "herdr-newtab-plus", "kestrel",
       "meridian", "orchard", "relay", "zephyr"}
FILES = ["README.md", "Cargo.toml", "LICENSE", "Makefile", ".gitignore", ".env.example",
         "notes.txt", "pyproject.toml", "setup.cfg", "main.rs"]
CHILDREN = ["src", "tests", "docs", "scripts", ".github", "target", "assets", "examples"]


class RecordingHerdr(object):
    """Serves the slice of the Herdr socket protocol the plugin uses, and logs it."""

    def __init__(self, path: str, data: Dict[str, Any],
                 answers: Optional[Dict[str, Any]] = None) -> None:
        self.path = path
        self.data = data
        self.answers = answers or {}
        self.log: List[Tuple[str, Any]] = []
        self._lock = threading.Lock()
        self._closing = False
        self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._server.bind(path)
        self._server.listen(64)
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self) -> None:
        while not self._closing:
            try:
                conn, _ = self._server.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(conn,), daemon=True).start()

    def _serve(self, conn: socket.socket) -> None:
        buffered = b""
        try:
            while b"\n" not in buffered:
                chunk = conn.recv(65536)
                if not chunk:
                    return
                buffered += chunk
            request = json.loads(buffered.split(b"\n", 1)[0])
            method = str(request.get("method", ""))
            params = request.get("params") or {}
            with self._lock:
                self.log.append((method, params))
                count = sum(1 for m, _ in self.log if m == method)
            conn.sendall(self._answer(method, count))
        except (OSError, ValueError):
            pass
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def _answer(self, method: str, count: int) -> bytes:
        answer = self.answers.get(method)
        if callable(answer):
            answer = answer(count)
        if isinstance(answer, bytes):
            return answer
        if answer is not None:
            return json.dumps(dict({"id": "newtab"}, **answer)).encode() + b"\n"
        if method == "session.snapshot":
            result: Dict[str, Any] = {"snapshot": self.data}
        elif method in ("tab.create", "workspace.create"):
            result = {"tab": {"tab_id": "w9:t1"}, "workspace": {"workspace_id": "w9"},
                      "root_pane": {"pane_id": "w9:p1"}}
        else:
            result = {}
        return json.dumps({"id": "newtab", "result": result}).encode() + b"\n"

    def changes(self) -> List[Any]:
        with self._lock:
            return [[method, params] for method, params in self.log
                    if method != "session.snapshot"]

    def reads(self) -> int:
        with self._lock:
            return sum(1 for method, _ in self.log if method == "session.snapshot")

    def wait_for(self, method: str, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._lock:
                if any(m == method for m, _ in self.log):
                    return True
            time.sleep(0.02)
        return False

    def close(self) -> None:
        self._closing = True
        try:
            self._server.close()
        except OSError:
            pass


class Machine(object):
    """A home with a work root, fake tools, a config and a recording Herdr."""

    def __init__(self, scenario: Dict[str, Any]) -> None:
        self.tmp = os.path.realpath(tempfile.mkdtemp(prefix="ntp-"))
        base = self.tmp
        self.home = os.path.join(base, "home")
        self.root = os.path.join(self.home, "work")
        self.bin = os.path.join(base, "bin")
        self.config_dir = os.path.join(base, "config")
        self.state_dir = os.path.join(base, "state")
        self.socket_path = os.path.join(base, "herdr.sock")
        for path in (self.root, self.bin, self.config_dir, self.state_dir):
            os.makedirs(path)

        for index, name in enumerate(scenario.get("folders", FOLDERS)):
            folder = os.path.join(self.root, name)
            os.makedirs(folder, exist_ok=True)
            for child in CHILDREN[: index % len(CHILDREN) + 1]:
                os.mkdir(os.path.join(folder, child))
            for child in FILES[: (index * 3) % len(FILES) + 1]:
                with open(os.path.join(folder, child), "w") as handle:
                    handle.write("x")
            if name in GIT:
                os.mkdir(os.path.join(folder, ".git"))
                with open(os.path.join(folder, ".git", "HEAD"), "w") as handle:
                    handle.write("ref: refs/heads/%s\n" % ("main" if index % 3 else "feature/" + name))
        # A worktree, a detached head, an unreadable folder and an empty one.
        if scenario.get("extras", True):
            self._extras()

        for agent in scenario.get("agents_on_path", ("claude", "codex")):
            self._script(agent, "exit 0\n")
        self._script("fc-match", "exit 1\n")
        zoxide = scenario.get("zoxide")
        if zoxide is not None:
            listing = os.path.join(base, "zoxide.txt")
            with open(listing, "w") as handle:
                handle.write(zoxide.replace("{root}", self.root).replace("{home}", self.home))
            self._script("zoxide", '/bin/cat "%s"\n' % listing)

        config = scenario.get("config", {"roots": ["~/work"]})
        if config is not None:
            with open(os.path.join(self.config_dir, "config.json"), "w") as handle:
                handle.write(config if isinstance(config, str) else json.dumps(config))
        for relative, content in (scenario.get("files") or {}).items():
            path = os.path.join(self.state_dir, relative)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "w") as handle:
                text = content if isinstance(content, str) else json.dumps(content)
                handle.write(text.replace("{root}", self.root).replace("{home}", self.home))

        # macOS assesses a new executable on its first launch, which can take
        # longer than the plugin waits for zoxide. Pay that here, not in a run.
        for name in os.listdir(self.bin):
            subprocess.run([os.path.join(self.bin, name)], stdin=subprocess.DEVNULL,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

        data = scenario.get("data", default_snapshot)
        data = data(self) if callable(data) else copy.deepcopy(data)
        self.server: Optional[RecordingHerdr] = None
        if scenario.get("socket_up", True):
            self.server = RecordingHerdr(self.socket_path, data, scenario.get("answers"))

    def _extras(self) -> None:
        tree = os.path.join(self.root, "harbor-review")
        os.makedirs(os.path.join(tree, "src"))
        with open(os.path.join(tree, ".git"), "w") as handle:
            handle.write("gitdir: ../harbor/.git/worktrees/review\n")
        gitdir = os.path.join(self.root, "harbor", ".git", "worktrees", "review")
        os.makedirs(gitdir)
        with open(os.path.join(gitdir, "HEAD"), "w") as handle:
            handle.write("ref: refs/heads/review\n")
        detached = os.path.join(self.root, "detached")
        os.makedirs(os.path.join(detached, ".git"))
        with open(os.path.join(detached, ".git", "HEAD"), "w") as handle:
            handle.write("3f2a9c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39\n")
        os.makedirs(os.path.join(self.root, "empty"))
        locked = os.path.join(self.root, "locked")
        os.makedirs(locked)
        os.chmod(locked, 0)
        many = os.path.join(self.root, "many-files")
        os.makedirs(many)
        for index in range(40):
            with open(os.path.join(many, "file-%02d.txt" % index), "w") as handle:
                handle.write("x")
        os.symlink(os.path.join(self.root, "atlas"), os.path.join(self.root, "atlas-link"))
        for name in ("日本語プロジェクト", "café-naïve", "emoji-🚀-rocket", "Zebra", "apple"):
            os.makedirs(os.path.join(self.root, name))

    def _script(self, name: str, body: str) -> None:
        path = os.path.join(self.bin, name)
        with open(path, "w") as handle:
            handle.write("#!/bin/sh\n" + body)
        os.chmod(path, 0o755)

    def env(self) -> Dict[str, str]:
        return {
            "PATH": self.bin,
            "HOME": self.home,
            "TERM": "xterm-256color",
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "PYTHONHASHSEED": "0",
            "HERDR_SOCKET_PATH": self.socket_path,
            "HERDR_PLUGIN_CONFIG_DIR": self.config_dir,
            "HERDR_PLUGIN_STATE_DIR": self.state_dir,
            "HERDR_BIN_PATH": "/nonexistent/herdr",
        }

    def state_files(self) -> Dict[str, Any]:
        """Every file under the state directory, with volatile parts masked."""
        out: Dict[str, Any] = {}
        for directory, _, names in os.walk(self.state_dir):
            for name in sorted(names):
                path = os.path.join(directory, name)
                relative = os.path.relpath(path, self.state_dir)
                with open(path, "rb") as handle:
                    text = handle.read().decode("utf-8", "replace")
                if name == "recent.json":
                    try:
                        parsed = json.loads(text)
                        for entry in parsed.get("entries", []):
                            if isinstance(entry, dict) and isinstance(entry.get("ts"), int) \
                                    and entry["ts"] > 1_000_000_000:
                                entry["ts"] = "<now>"
                        text = json.dumps(parsed)
                    except ValueError:
                        pass
                out[relative] = self.mask(text)
                mode = os.stat(path).st_mode & 0o777
                out[relative + " (mode)"] = oct(mode)
        return dict(sorted(out.items()))

    def mask(self, text: str) -> str:
        return text.replace(self.tmp, "<tmp>")

    def close(self) -> None:
        if self.server is not None:
            self.server.close()
        locked = os.path.join(self.root, "locked")
        if os.path.exists(locked):
            os.chmod(locked, 0o755)
        shutil.rmtree(self.tmp, ignore_errors=True)


def default_snapshot(machine: Machine) -> Dict[str, Any]:
    root = machine.root
    spaces = [
        {"workspace_id": "w1", "label": "atlas", "number": 1, "focused": True},
        {"workspace_id": "w2", "label": "harbor", "number": 2, "focused": False},
        {"workspace_id": "w3", "label": "Kestrel Ops", "number": 3, "focused": False},
    ]
    panes = [
        {"pane_id": "w1:p1", "workspace_id": "w1", "cwd": os.path.join(root, "atlas"),
         "focused": True},
        {"pane_id": "w1:p2", "workspace_id": "w1", "cwd": os.path.join(root, "atlas-web")},
        {"pane_id": "w2:p1", "workspace_id": "w2", "cwd": os.path.join(root, "harbor")},
        {"pane_id": "w2:p2", "workspace_id": "w2", "cwd": os.path.join(root, "harbor-review")},
        {"pane_id": "w3:p1", "workspace_id": "w3", "cwd": os.path.join(root, "kestrel")},
        {"pane_id": "w3:p2", "workspace_id": "w3", "cwd": os.path.join(root, "kestrel"),
         "agent_status": "done", "agent": "gemini"},
    ]
    agents = [
        {"pane_id": "w1:p1", "agent": "claude", "name": "atlas", "agent_status": "working"},
        {"pane_id": "w1:p2", "agent": "codex", "name": "atlas-web", "agent_status": "blocked"},
        {"pane_id": "w2:p1", "agent": "claude", "name": "harbor", "status": "idle"},
        {"pane_id": "w2:p2", "agent": "Claude Code", "agent_status": "none"},
    ]
    return {
        "version": "0.9.4",
        "focused_workspace_id": "w1",
        "focused_pane_id": "w1:p1",
        "workspaces": spaces,
        "panes": panes,
        "agents": agents,
    }


def empty_snapshot(machine: Machine) -> Dict[str, Any]:
    return {"version": "0.9.4", "workspaces": [], "panes": [], "agents": []}


RECENTS = {"recent.json": {"version": 1, "entries": [
    {"path": "{root}/meridian", "agent": "codex", "ts": 1700000300},
    {"path": "{root}/beacon", "agent": "", "ts": 1700000200},
    {"path": "{root}/atlas", "agent": "claude", "ts": 1700000100},
    {"path": "{root}/gone-now", "agent": "claude", "ts": 1700000000},
]}}


# -- a screen ----------------------------------------------------------------


def char_width(char: str) -> int:
    if unicodedata.combining(char):
        return 0
    code = ord(char)
    if code < 32 or code == 127:
        return 0
    return 2 if unicodedata.east_asian_width(char) in ("W", "F") else 1


DEFAULT_STYLE = (None, None, False, False, False)


def sgr(params: str, style: Tuple) -> Tuple:
    fg, bg, bold, underline, reverse = style
    parts = params.split(";") if params else ["0"]
    index = 0
    while index < len(parts):
        part = parts[index] or "0"
        if part in ("38", "48"):
            mode = parts[index + 1] if index + 1 < len(parts) else ""
            length = 3 if mode == "2" else 1
            value = ";".join(parts[index: index + 2 + length])
            if part == "38":
                fg = value
            else:
                bg = value
            index += 2 + length
            continue
        if part == "0":
            fg = bg = None
            bold = underline = reverse = False
        elif part == "1":
            bold = True
        elif part == "4":
            underline = True
        elif part == "7":
            reverse = True
        elif part == "22":
            bold = False
        elif part == "24":
            underline = False
        elif part == "27":
            reverse = False
        elif part == "39":
            fg = None
        elif part == "49":
            bg = None
        index += 1
    return (fg, bg, bold, underline, reverse)


class Screen(object):
    """The alt screen, and what was printed on the main one."""

    def __init__(self, width: int, height: int) -> None:
        self.width = width
        self.height = height
        self.cells = [[(" ", DEFAULT_STYLE)] * width for _ in range(height)]
        self.x = self.y = 0
        self.style = DEFAULT_STYLE
        self.alt = False
        self.main_text = ""
        self.modes: List[str] = []
        self.pending = ""

    def resize(self, width: int, height: int) -> None:
        cells = [[(" ", DEFAULT_STYLE)] * width for _ in range(height)]
        for y in range(min(height, self.height)):
            for x in range(min(width, self.width)):
                cells[y][x] = self.cells[y][x]
        self.cells, self.width, self.height = cells, width, height

    def feed(self, data: str) -> None:
        data = self.pending + data
        self.pending = ""
        index, length = 0, len(data)
        while index < length:
            char = data[index]
            if char != "\x1b":
                if not self.alt:
                    self.main_text += char
                elif char == "\n":
                    self.y += 1
                elif char == "\r":
                    self.x = 0
                else:
                    self._put(char)
                index += 1
                continue
            if index + 1 >= length:
                self.pending = data[index:]
                break
            introducer = data[index + 1]
            if introducer == "[":
                end = index + 2
                while end < length and not ("\x40" <= data[end] <= "\x7e"):
                    end += 1
                if end >= length:
                    self.pending = data[index:]
                    break
                self._csi(data[index + 2: end], data[end])
                index = end + 1
                continue
            if introducer == "]":
                bel = data.find("\x07", index)
                st = data.find("\x1b\\", index)
                ends = [e for e in ((bel, 1), (st, 2)) if e[0] >= 0]
                if not ends:
                    self.pending = data[index:]
                    break
                end, width = min(ends)
                self.modes.append("osc:" + data[index + 2: end])
                index = end + width
                continue
            index += 2

    def _put(self, char: str) -> None:
        width = char_width(char)
        if width == 0 or self.y >= self.height or self.x >= self.width:
            return
        self.cells[self.y][self.x] = (char, self.style)
        if width == 2 and self.x + 1 < self.width:
            self.cells[self.y][self.x + 1] = ("", self.style)
        self.x += width

    def _clear(self, y: int, start: int, stop: int) -> None:
        if 0 <= y < self.height:
            row = self.cells[y]
            for x in range(max(0, start), min(self.width, stop)):
                row[x] = (" ", DEFAULT_STYLE)

    def _csi(self, params: str, final: str) -> None:
        if params.startswith("?") and final in ("h", "l"):
            self.modes.append(params + final)
            if params == "?1049":
                self.alt = final == "h"
                if self.alt:
                    for y in range(self.height):
                        self._clear(y, 0, self.width)
            return
        numbers = [int(part) if part.isdigit() else 0 for part in params.split(";")]
        if final == "m":
            self.style = sgr(params, self.style)
        elif final in ("H", "f"):
            self.y = max(0, (numbers[0] if numbers and numbers[0] else 1) - 1)
            self.x = max(0, (numbers[1] if len(numbers) > 1 and numbers[1] else 1) - 1)
        elif final == "K":
            mode = numbers[0] if numbers else 0
            if mode == 0:
                self._clear(self.y, self.x, self.width)
            elif mode == 1:
                self._clear(self.y, 0, self.x + 1)
            else:
                self._clear(self.y, 0, self.width)
        elif final == "J":
            mode = numbers[0] if numbers else 0
            if mode == 0:
                self._clear(self.y, self.x, self.width)
                for y in range(self.y + 1, self.height):
                    self._clear(y, 0, self.width)
            else:
                for y in range(self.height):
                    self._clear(y, 0, self.width)
        elif final == "C":
            self.x += numbers[0] if numbers and numbers[0] else 1
        else:
            self.modes.append("csi:" + params + final)

    def dump(self) -> List[str]:
        """Each row as its text, then its style runs."""
        out = []
        for row in self.cells:
            text = "".join(char for char, _ in row)
            runs = []
            start = 0
            for x in range(1, len(row) + 1):
                if x == len(row) or row[x][1] != row[start][1]:
                    style = row[start][1]
                    if style != DEFAULT_STYLE:
                        runs.append("%d-%d:%s" % (start, x, json.dumps(style)))
                    start = x
            out.append(text.rstrip() + ("  ‖ " + " ".join(runs) if runs else ""))
        return out


# -- a session on a pty -----------------------------------------------------


class Session(object):
    def __init__(self, argv: Sequence[str], env: Dict[str, str], size: Tuple[int, int],
                 appearance: Optional[bytes]) -> None:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", size[1], size[0], 0, 0))
        self.master = master
        self.output = bytearray()
        self.appearance = appearance
        self._answered = 0
        self.proc = subprocess.Popen(list(argv), stdin=slave, stdout=slave, stderr=slave,
                                     env=env, cwd=ROOT, start_new_session=True,
                                     close_fds=True)
        os.close(slave)
        self.status: Optional[int] = None

    def pump(self, timeout: float) -> bool:
        try:
            readable, _, _ = select.select([self.master], [], [], max(0.0, timeout))
        except InterruptedError:
            return True
        if not readable:
            return True
        try:
            data = os.read(self.master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return False
            raise
        if not data:
            return False
        self.output += data
        while self.appearance is not None:
            found = self.output.find(QUERY, self._answered)
            if found < 0:
                break
            self._answered = found + len(QUERY)
            os.write(self.master, self.appearance)
        return True

    def drain(self, quiet: float = SETTLE) -> None:
        while True:
            before = len(self.output)
            if not self.pump(quiet) or len(self.output) == before:
                return

    def send(self, data: bytes) -> None:
        try:
            os.write(self.master, data)
        except OSError:
            pass

    def resize(self, width: int, height: int) -> None:
        fcntl.ioctl(self.master, termios.TIOCSWINSZ, struct.pack("HHHH", height, width, 0, 0))
        try:
            os.killpg(self.proc.pid, signal.SIGWINCH)
        except OSError:
            pass

    def reaped(self) -> bool:
        if self.status is not None:
            return True
        code = self.proc.poll()
        if code is None:
            return False
        self.status = code
        return True

    def wait_reaped(self, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.reaped():
                return True
            self.pump(0.01)
        return self.reaped()

    def close(self) -> None:
        if self.status is None:
            try:
                os.killpg(self.proc.pid, signal.SIGKILL)
            except OSError:
                pass
            try:
                self.proc.wait(5)
            except subprocess.TimeoutExpired:
                pass
        try:
            os.close(self.master)
        except OSError:
            pass


# -- running a scenario ------------------------------------------------------


def run_picker(cmd: Sequence[str], scenario: Dict[str, Any]) -> Dict[str, Any]:
    machine = Machine(scenario)
    size = scenario.get("size", SIZE)
    env = machine.env()
    env.update(scenario.get("env", {}))
    session = Session(list(cmd) + list(scenario.get("args", [])), env, size,
                      scenario.get("appearance", DARK))
    frames: List[Any] = []
    screen = Screen(*size)
    fed = 0
    try:
        session.drain(0.6)
        for kind, value in [("look", None)] + list(scenario.get("steps", [])):
            label = "open"
            if kind == "keys":
                session.send(value.replace(b"{root}", machine.root.encode()))
                session.drain()
                label = repr(value)
            elif kind == "wait":
                time.sleep(value)
                session.drain()
                label = "wait %s" % value
            elif kind == "resize":
                session.resize(*value)
                screen.resize(*value)
                time.sleep(0.05)
                # Python's select() resumes after SIGWINCH and only redraws on
                # the next input; a key nothing is bound to wakes both builds.
                session.send(F2)
                session.drain()
                label = "resize %dx%d" % value
            text = session.output[fed:].decode("utf-8", "replace")
            fed = len(session.output)
            screen.feed(text)
            frames.append({"after": label, "screen": screen.dump()})
            if session.reaped():
                break
        if session.wait_reaped(EXIT_TIMEOUT):
            status: Any = session.status
        else:
            session.send(b"\x03")
            status = ("did not exit; ctrl+c -> %s" % session.status
                      if session.wait_reaped(EXIT_TIMEOUT) else "hung")
        # The agent is started by a detached child after the popup has gone,
        # and the two builds' children boot at different speeds: wait for it
        # wherever something was created, so neither is cut off early.
        if machine.server is not None and (
                scenario.get("expect_agent") or any(
                    method in ("tab.create", "workspace.create")
                    for method, _ in machine.server.changes())):
            timeout = AGENT_TIMEOUT if scenario.get("expect_agent") else 1.5
            machine.server.wait_for("agent.start", timeout)
            time.sleep(0.3)
        session.drain(0.1)
        screen.feed(session.output[fed:].decode("utf-8", "replace"))
        return {
            "frames": frames,
            "exit": status,
            "printed": machine.mask(screen.main_text),
            "modes": screen.modes,
            "changes": machine.server.changes() if machine.server else [],
            "snapshots": machine.server.reads() if machine.server else 0,
            "files": machine.state_files(),
        }
    finally:
        session.close()
        machine.close()


def run_cli(cmd: Sequence[str], scenario: Dict[str, Any]) -> Dict[str, Any]:
    machine = Machine(scenario)
    env = machine.env()
    env.update(scenario.get("env", {}))
    try:
        proc = subprocess.run(list(cmd) + list(scenario["args"]), env=env, cwd=ROOT,
                              stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, timeout=60)
        stdout = machine.mask(proc.stdout.decode("utf-8", "replace"))
        stderr = machine.mask(proc.stderr.decode("utf-8", "replace"))
        for mask in scenario.get("mask", []):
            stdout = "\n".join("<masked>" if re.match(mask, line) else line
                               for line in stdout.split("\n"))
        return {
            "exit": proc.returncode,
            "stdout": stdout,
            "stderr": stderr,
            "changes": machine.server.changes() if machine.server else [],
            "files": machine.state_files(),
        }
    finally:
        machine.close()


# -- scenarios ---------------------------------------------------------------


def worktree_snapshot(machine: Machine) -> Dict[str, Any]:
    data = default_snapshot(machine)
    data["panes"].append({"pane_id": "w2:p3", "workspace_id": "w2",
                          "cwd": os.path.join(machine.root, "atlas"), "focused": False})
    return data


def symlink_snapshot(machine: Machine) -> Dict[str, Any]:
    data = default_snapshot(machine)
    # A pane standing in the real folder, picked later through the symlink.
    return data


def odd_snapshot(machine: Machine) -> Dict[str, Any]:
    root = machine.root
    return {
        "version": 7,
        "focused_pane_id": "nope",
        "workspaces": [
            {"workspace_id": "w1", "label": "", "number": "3", "focused": False},
            "not a space",
            {"workspace_id": 5, "label": "bad"},
            {"workspace_id": "w2", "label": None, "number": True, "focused": True},
        ],
        "panes": [
            {"pane_id": "w1:p1", "workspace_id": "w1", "cwd": os.path.join(root, "beacon") + "/"},
            {"pane_id": "w2:p1", "workspace_id": "w2", "cwd": "~/work/tern"},
            {"pane_id": "w2:p2", "workspace_id": "w2", "cwd": ""},
            {"pane_id": "w9:p1", "workspace_id": "w9", "cwd": os.path.join(root, "sable")},
            42,
        ],
        "agents": [
            {"pane_id": "w1:p1", "agent": "kimi", "agent_status": "blocked"},
            {"pane_id": "w2:p1", "agent": "cursor-agent", "agent_status": "working",
             "foreground_cwd": os.path.join(root, "umbra")},
            "nope",
        ],
    }


ALL_AGENTS = ("claude", "codex", "gemini", "cursor-agent", "opencode", "copilot", "amp",
              "droid", "grok", "kimi", "pi", "cline", "kilo", "kiro", "devin", "agy", "omp",
              "mastracode", "hermes", "qoder", "maki")

ZOXIDE = (
    "  120.5 {root}/zephyr\n"
    "   88.0 {root}/willow\n"
    "  1_000 {root}/quartz\n"
    "  nan {root}/pier\n"
    "garbage\n"
    "   88.0 {root}/vesper\n"
    "   12.0 {home}/not-there\n"
    "    2.5 {root}/atlas\n"
)

PICKER: Dict[str, Dict[str, Any]] = {
    "pick-open": {"steps": keys(ESC)},
    "pick-browse": {"files": RECENTS, "steps": keys(DOWN, DOWN, UP, PGDN, PGDN, PGUP, ctrl("n"),
                                                    ctrl("p"), UP, UP, ESC)},
    "pick-type": {"files": RECENTS, "steps": keys("her", BS, "rdr nt", ctrl("w"), "bar",
                                                  ctrl("a"), RIGHT, RIGHT, DELETE, ctrl("e"),
                                                  ctrl("k"), LEFT, ctrl("u"), ESC)},
    "pick-fuzzy": {"steps": keys("mrd", ctrl("u"), "'atlas web'", ctrl("u"), "ATL", ctrl("u"),
                                 "zzzzqx", ESC)},
    "pick-path": {"steps": keys("~/work/", "ha", TAB, ctrl("w"), "her", TAB, TAB, ctrl("w"),
                                ctrl("w"), "/", ESC)},
    "pick-path-abs": {"steps": keys(("keys", b"{root}"), ESC)},
    "pick-path-dot": {"steps": keys("./", DOWN, ctrl("u"), ".", ctrl("u"), "..", ESC)},
    "pick-path-vars": {"steps": keys("$HOME/work/k", RIGHT, ctrl("u"), "${HOME}/wo", ESC)},
    "pick-path-missing": {"steps": keys("~/work/nothing-here", ENTER, wait(0.4), ESC)},
    "pick-complete-right": {"steps": keys("bea", RIGHT, RIGHT, ctrl("u"), RIGHT, ESC)},
    "pick-help": {"steps": keys("?", "x", F1, DOWN, F1, ENTER, "a", F1, ESC, ESC)},
    "pick-help-narrow": {"size": (60, 20), "steps": keys(F1, ESC, ESC)},
    "pick-modes": {"steps": keys(ctrl("t"), ctrl("t"), ctrl("t"), ctrl("t"), ESC)},
    "pick-mode-remembered": {"files": {"recent.json": {"version": 1, "create": "tab",
                                                       "entries": []}},
                             "steps": keys(ctrl("t"), ESC)},
    "pick-skip": {"files": RECENTS, "steps": keys(ctrl("y"), TAB, ctrl("y"), TAB, ctrl("y"),
                                                  SHIFT_TAB, ctrl("y"), ESC)},
    "pick-agents": {"steps": keys(TAB, TAB, TAB, SHIFT_TAB, SHIFT_TAB, SHIFT_TAB, ESC)},
    "pick-preview-toggle": {"steps": keys(ctrl("o"), ctrl("o"), DOWN, ESC)},
    "pick-preview-narrow": {"size": (85, 24), "steps": keys(ctrl("o"), DOWN, ctrl("o"),
                                                             ctrl("o"), ESC)},
    "pick-preview-cases": {"steps": keys("harbor-rev", ctrl("u"), "detached", ctrl("u"),
                                         "empty", ctrl("u"), "locked", ctrl("u"), "many-fi",
                                         ctrl("u"), "kestrel", ctrl("u"), "~/work/nope",
                                         ctrl("u"), "atlas-link", ESC)},
    "pick-preview-hidden": {"config": {"roots": ["~/work"], "hidden": True},
                            "steps": keys("atlas", ctrl("u"), "~/work/at", ESC)},
    "pick-preview-on": {"size": (85, 24), "config": {"roots": ["~/work"], "preview": True},
                        "steps": keys(DOWN, ESC)},
    "pick-preview-off": {"config": {"roots": ["~/work"], "preview": False},
                         "steps": keys(ctrl("o"), ctrl("o"), ESC)},
    "pick-preview-int": {"config": {"roots": ["~/work"], "preview": 1},
                         "steps": keys(ctrl("o"), ESC)},
    "pick-create-tab": {"steps": keys(ENTER)},
    "pick-create-space": {"files": RECENTS, "steps": keys("tern", ENTER)},
    "pick-create-agent": {"files": RECENTS, "expect_agent": True, "steps": keys("beacon", TAB,
                                                                                ENTER)},
    "pick-create-skip": {"expect_agent": True, "steps": keys("quartz", TAB, TAB, ctrl("y"),
                                                             ENTER)},
    "pick-create-forced-space": {"steps": keys("atlas", ctrl("t"), ctrl("t"), ENTER)},
    "pick-create-forced-tab": {"steps": keys("vesper", ctrl("t"), ENTER)},
    "pick-create-path": {"steps": keys("~/work/sable/", ENTER)},
    "pick-create-symlink": {"steps": keys("~/work/atlas-link", ENTER)},
    "pick-create-file": {"steps": keys("~/work/atlas/README.md", ENTER, wait(0.3), ESC)},
    "pick-create-empty": {"config": {"roots": []}, "data": empty_snapshot,
                          "steps": keys(ENTER, wait(0.2), ESC)},
    "pick-create-fails": {"answers": {"workspace.create": {"error": {"message": "nope, full"}}},
                          "steps": keys("tern", ENTER)},
    "pick-create-no-pane": {"answers": {"tab.create": {"result": {"tab": {"tab_id": "x"}}}},
                            "steps": keys(ENTER)},
    "pick-create-name-taken": {
        "expect_agent": True,
        "answers": {"agent.start": lambda count: {"error": {"message": "agent name taken"}}
                    if count == 1 else {"result": {}}},
        "steps": keys("atlas", TAB, ENTER)},
    "pick-mouse": {"files": RECENTS, "steps": keys(("keys", click(10, 5)), ("keys", click(10, 6)),
                                                   ("keys", wheel(10, 6, True)),
                                                   ("keys", wheel(10, 6, False)),
                                                   ("keys", click(16, 17)),
                                                   ("keys", click(90, 6)),
                                                   ("keys", click(10, 14)),
                                                   ("keys", click(10, 5)), ESC)},
    "pick-mouse-confirm": {"steps": keys(("keys", click(10, 6)), ("keys", click(10, 6)))},
    "pick-mouse-off": {"config": {"roots": ["~/work"], "mouse": False}, "steps": keys(ESC)},
    "pick-paste": {"steps": keys(("keys", paste("~/work/\nwil\r")), ctrl("u"),
                                 ("keys", paste("?")), ctrl("u"), ("keys", paste("a\x01b")), ESC)},
    "pick-rescan": {"steps": keys(DOWN, DOWN, ctrl("r"), wait(0.2), ESC)},
    "pick-flash": {"steps": keys(RIGHT, wait(2.8), ESC)},
    "pick-resize": {"steps": keys(resize(80, 20), resize(140, 40), resize(30, 6), resize(107, 28),
                                  ESC)},
    "pick-tiny": {"size": (24, 4), "steps": keys(DOWN, "a", ESC)},
    "pick-small": {"size": (50, 8), "steps": keys(DOWN, TAB, ESC)},
    "pick-short": {"size": (70, 12), "steps": keys(DOWN, ESC)},
    "pick-wide": {"size": (200, 50), "steps": keys(DOWN, ctrl("t"), ESC)},
    "pick-unicode": {"steps": keys("日本", BS, "café", ctrl("u"), "emoji", ctrl("u"), "~/work/日",
                                   TAB, ESC)},
    "pick-light": {"appearance": LIGHT, "steps": keys(DOWN, ESC)},
    "pick-silent-terminal": {"appearance": None, "steps": keys(DOWN, ESC)},
    "pick-icons": {"config": {"roots": ["~/work"], "agent_icons": "font"},
                   "files": RECENTS, "steps": keys(TAB, ctrl("y"), DOWN, ESC)},
    "pick-icons-bool": {"config": {"roots": ["~/work"], "agent_icons": True}, "steps": keys(ESC)},
    "pick-colors": {"config": {"roots": ["~/work"], "colors": {
        "accent": "#ff8800", "muted": "250", "text": "nope", "working": "1", "dim": "red",
        "agent_claude": "default", "rule": "#12345", "match": " BRIGHT_GREEN "},
        "selection_background": "#334455"}, "steps": keys(DOWN, "at", ESC)},
    "pick-no-selection": {"config": {"roots": ["~/work"], "selection_background": None},
                          "steps": keys(DOWN, TAB, ESC)},
    "pick-selection-none": {"config": {"roots": ["~/work"], "selection_background": "none",
                                       "colors": {"muted": "244"}},
                            "steps": keys(DOWN, ESC)},
    "pick-config-agents": {"config": {"roots": ["~/work"], "agents": ["codex", "nope", 3, "pi"],
                                      "default_agent": "pi"}, "steps": keys(TAB, ESC)},
    "pick-config-default-none": {"config": {"roots": ["~/work"], "default_agent": "none"},
                                 "files": RECENTS, "steps": keys(DOWN, ESC)},
    "pick-config-default-codex": {"config": {"roots": ["~/work"], "default_agent": "codex"},
                                  "steps": keys(DOWN, ESC)},
    "pick-config-create": {"config": {"roots": ["~/work"], "create": "space"},
                           "steps": keys(ENTER)},
    "pick-config-skip-flags": {"config": {"roots": ["~/work"], "skip_permission_flags": {
        "claude": [], "gemini": ["--yolo", ""], "codex": [1]}},
        "agents_on_path": ("claude", "codex", "gemini"), "expect_agent": True,
        "steps": keys(TAB, ctrl("y"), TAB, ctrl("y"), TAB, ctrl("y"), "kestrel", ENTER)},
    "pick-config-max": {"config": {"roots": ["~/work"], "max_suggestions": "5"},
                        "steps": keys(PGDN, ESC)},
    "pick-config-roots": {"config": {"roots": ["~/work", "~/work", "~/work/harbor", 7, "",
                                               "~/missing", "$HOME/work/herdr-bar"]},
                          "steps": keys(PGDN, PGDN, PGDN, ESC)},
    "pick-config-broken": {"config": "{not json", "steps": keys(ESC)},
    "pick-config-list": {"config": "[1, 2]", "steps": keys(ESC)},
    "pick-config-bom": {"config": "﻿{\"roots\": [\"~/work\"]}", "steps": keys(ESC)},
    "pick-config-nulls": {"config": {"roots": None, "create": None, "agents": None,
                                     "mouse": None, "hidden": None, "zoxide": None,
                                     "preview": None, "colors": None,
                                     "max_suggestions": None}, "steps": keys(ESC)},
    "pick-zoxide": {"zoxide": ZOXIDE, "config": {"roots": ["~/work"], "zoxide": True},
                    "steps": keys(PGDN, ESC)},
    "pick-zoxide-auto": {"zoxide": ZOXIDE, "steps": keys(PGDN, "wil", ESC)},
    "pick-zoxide-off": {"zoxide": ZOXIDE, "config": {"roots": ["~/work"], "zoxide": False},
                        "steps": keys(ESC)},
    "pick-zoxide-fails": {"zoxide": None, "config": {"roots": ["~/work"], "zoxide": True},
                          "steps": keys(ESC)},
    "pick-many-agents": {"agents_on_path": ALL_AGENTS, "steps": keys(
        TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, TAB, ("keys", click(60, 25)),
        SHIFT_TAB, ESC)},
    "pick-many-agents-narrow": {"agents_on_path": ALL_AGENTS, "size": (40, 12),
                                "steps": keys(TAB, TAB, TAB, ESC)},
    "pick-no-agents": {"agents_on_path": (), "steps": keys(TAB, ctrl("y"), ESC)},
    "pick-recents-broken": {"files": {"recent.json": "{oops"}, "steps": keys(ctrl("t"), ESC)},
    "pick-recents-odd": {"files": {"recent.json": {"version": 1, "create": 4, "entries": [
        {"path": "{root}/willow", "agent": 7, "ts": 1.5},
        {"path": "{root}/willow/", "agent": "claude", "ts": True},
        {"path": "", "agent": "x"}, "nope", {"agent": "codex"},
        {"path": "{root}/relay", "agent": "codex", "ts": "x"},
        {"path": "~/work/orchard", "agent": "gemini", "ts": 12345678901234567890},
    ]}}, "steps": keys(DOWN, DOWN, ESC)},
    "pick-recents-touch": {"files": RECENTS, "steps": keys("relay", ENTER)},
    "pick-odd-snapshot": {"data": odd_snapshot, "steps": keys(DOWN, DOWN, "umbra", ENTER)},
    "pick-worktree-spaces": {"data": worktree_snapshot, "steps": keys("atlas", ctrl("o"),
                                                                      ENTER)},
    "pick-symlink-space": {"data": symlink_snapshot, "steps": keys("atlas-link", ENTER)},
    "pick-empty-catalog": {"config": {"roots": []}, "data": empty_snapshot,
                           "steps": keys("x", ESC)},
    "pick-unreachable": {"socket_up": False, "steps": []},
    "pick-bad-snapshot": {"answers": {"session.snapshot": {"result": {"nope": 1}}}, "steps": []},
    "pick-garbage-snapshot": {"answers": {"session.snapshot": b"not json\n"}, "steps": []},
    "pick-herdr-error": {"answers": {"session.snapshot": {"error": "boom"}}, "steps": []},
    "pick-ctrl-c": {"steps": keys("abc", ctrl("c"))},
    "pick-ctrl-q": {"steps": keys(ctrl("q"))},
    "pick-escape-split": {"steps": keys(b"\x1bO", wait(0.2), b"\x1b", wait(0.2), ESC)},
}

CLI: Dict[str, Dict[str, Any]] = {
    "cli-version": {"args": ["--version"], "mask": [r"^herdr-newtab-plus "]},
    "cli-help": {"args": ["--help"], "mask": [r"^usage: "]},
    "cli-h": {"args": ["-h", "--list"], "mask": [r"^usage: "]},
    "cli-list": {"args": ["--list"], "files": RECENTS},
    "cli-list-zoxide": {"args": ["--list"], "zoxide": ZOXIDE},
    "cli-list-odd": {"args": ["--list"], "data": odd_snapshot},
    "cli-list-unreachable": {"args": ["--list"], "socket_up": False},
    "cli-list-error": {"args": ["--list"], "answers": {"session.snapshot": {"error": {
        "message": "no session"}}}},
    "cli-list-hidden": {"args": ["--list"], "config": {"roots": ["~/work"], "hidden": True}},
    "cli-doctor": {"args": ["--doctor"], "files": RECENTS,
                   "mask": [r"^python ", r"^runtime ", r"^herdr-newtab-plus "]},
    "cli-doctor-config": {"args": ["--doctor"], "agents_on_path": ALL_AGENTS,
                          "config": {"roots": ["~/work", "~/x"], "create": "tab",
                                     "preview": False, "agent_icons": "none",
                                     "skip_permission_flags": {"gemini": ["--yolo"]}},
                          "mask": [r"^python ", r"^runtime ", r"^herdr-newtab-plus "]},
    "cli-doctor-font": {"args": ["--doctor"], "config": {"agent_icons": "font", "preview": True},
                        "mask": [r"^python ", r"^runtime ", r"^herdr-newtab-plus "]},
    "cli-doctor-unreachable": {"args": ["--doctor"], "socket_up": False,
                               "mask": [r"^python ", r"^runtime ", r"^herdr-newtab-plus "]},
    "cli-no-tty": {"args": []},
    "cli-start-agent-missing": {"args": ["--start-agent"]},
    "cli-start-agent-ok": {"args": ["--start-agent",
                                    '{"name": "atlas", "kind": "claude", "pane": "w1:p1", '
                                    '"args": ["--x"]}']},
    "cli-start-agent-no-args": {"args": ["--start-agent",
                                         '{"name": "a", "kind": "codex", "pane": 3}']},
    "cli-start-agent-bad-args": {"args": ["--start-agent",
                                          '{"name": "a", "kind": "codex", "pane": "p", '
                                          '"args": "x"}']},
    "cli-start-agent-fails": {"args": ["--start-agent",
                                       '{"name": "a", "kind": "codex", "pane": "p"}'],
                              "answers": {"agent.start": {"error": {"message": "pane gone"}}}},
}


def run_scenarios(cmd: Sequence[str], only: Optional[Sequence[str]]) -> Dict[str, Any]:
    results: Dict[str, Any] = {}
    for scenarios, runner in ((PICKER, run_picker), (CLI, run_cli)):
        for name, scenario in scenarios.items():
            if only and not any(part in name for part in only):
                continue
            scenario = dict(scenario)
            results[name] = runner(cmd, scenario)
    return results


TMP_NAME = re.compile(r"ntp-[A-Za-z0-9_]{8}")


def dump(value: Any) -> List[str]:
    text = json.dumps(value, indent=1, ensure_ascii=False, sort_keys=True)
    return TMP_NAME.sub("ntp-XXXXXXXX", text).splitlines()


def compare(reference: Dict[str, Any], candidate: Dict[str, Any]) -> int:
    failures = 0
    for name in candidate:
        if name not in reference:
            print("? %s (no reference)" % name)
            continue
        lines = list(difflib.unified_diff(dump(reference[name]), dump(candidate[name]),
                                          "python:" + name, "rust:" + name, lineterm="", n=2))
        if lines:
            failures += 1
            print("✗ %s" % name)
            print("\n".join(lines[:150]))
            if len(lines) > 150:
                print("  … %d more diff lines" % (len(lines) - 150))
        else:
            print("✓ %s" % name)
    print("\n%d/%d scenarios differ" % (failures, len(candidate)))
    return 1 if failures else 0


def main(argv: Sequence[str]) -> int:
    args = list(argv)

    def option(flag: str) -> Optional[str]:
        return args[args.index(flag) + 1] if flag in args else None

    only_text = option("--only")
    only = only_text.split(",") if only_text else None
    rust = option("--rust")
    rust_cmd = [os.path.abspath(rust)] if rust else None
    if "--save" in args:
        results = run_scenarios(PYTHON_CMD, only)
        with open(option("--save"), "w") as handle:  # type: ignore[arg-type]
            json.dump(results, handle, indent=1, ensure_ascii=False, sort_keys=True)
            handle.write("\n")
        return 0
    if "--check" in args:
        with open(option("--check")) as handle:  # type: ignore[arg-type]
            reference = json.load(handle)
        return compare(reference, run_scenarios(rust_cmd or PYTHON_CMD, only))
    if rust_cmd is None:
        print(__doc__)
        return 2
    reference = run_scenarios(PYTHON_CMD, only)
    return compare(reference, run_scenarios(rust_cmd, only))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
