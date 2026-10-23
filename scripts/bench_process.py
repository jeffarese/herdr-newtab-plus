#!/usr/bin/env python3
"""Time the picker as a user feels it: a real process, a real terminal, a fake Herdr.

    python3 scripts/bench_process.py                    # the numbers, for `python3 run.py`
    python3 scripts/bench_process.py --json             # machine-readable
    python3 scripts/bench_process.py --save NAME        # write scripts/bench-process-NAME.json
    python3 scripts/bench_process.py --compare NAME     # side by side with a saved run
    python3 scripts/bench_process.py --cmd bin/herdr-newtab-plus   # bench another build
    python3 scripts/bench_process.py --runs 20          # more samples, tighter medians

``bench.py`` drives the ``Picker`` object in-process, which is the right tool
for what a frame costs to compose and paint, but blind to everything outside
it: interpreter startup, imports, the socket round trip, scanning the roots,
the terminal handshake, and the detached child that starts the agent. Those are
where most of the felt cost lives, so this bench treats the plugin as a black
box. It starts the plugin's own entry point on a pseudo-terminal, serves it a
synthetic Herdr session over a Unix socket and a synthetic home on disk, types
at it, and times what comes back.

Because nothing here imports the plugin, any implementation that speaks the
same protocol is measured against the same baseline: ``--cmd`` names the
command that stands in for ``python3 run.py``.

What is measured, per workload (12, 400 and 4,000 folders under the roots, with
a session, recents and a zoxide list that grow alongside):

* first frame -- spawn to the moment the whole first frame is on screen. The
  number a user feels every time they press the chord.
* keystroke -- typing a query and deleting it, from writing the key to the last
  byte of the frame it produced.
* navigation -- the down arrow, which moves the selection and refills the
  preview from a folder it has not read yet.
* path keystroke -- typing a path into the roots, where one keystroke lists a
  directory of every folder in the workload.
* exit -- esc to process exit.
* create -- Enter to Herdr being asked for the tab or space, and to exit.
* agent hand-off -- Enter to Herdr being asked to start the agent, which a
  detached child of the plugin does after the popup has gone.
* cpu, memory, bytes -- user+system CPU and peak RSS of the popup process, and
  every byte it wrote to the terminal.
* herdr calls -- requests the popup made of the server.

And two command-line runs: ``--version``, the floor of starting the program at
all, and ``--list``, which builds the catalog and prints it.

POSIX only: pseudo-terminals and Unix sockets are what is being timed.
"""

from __future__ import annotations

import errno
import fcntl
import json
import os
import pty
import select
import shlex
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
from typing import Any, Dict, List, Optional, Sequence, Tuple

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_CMD = [sys.executable, os.path.join(ROOT, "run.py")]

# What the manifest's 110x30 popup gives the pty.
SIZE = (107, 28)
WORKLOADS = (
    # name, folders under the root, panes in the session, recents, zoxide entries
    ("small", 12, 2, 6, 0),
    ("medium", 400, 32, 40, 120),
    ("large", 4000, 200, 80, 400),
)
QUERY = b"proj"
# A frame is over once the plugin has been quiet this long.
QUIET = 0.02
TIMEOUT = 15.0
# The footer is the last row of every frame, and this is the end of it.
FRAME_DONE = b"help"
APPEARANCE_QUERY = b"\x1b]11;?"
APPEARANCE_REPLY = b"\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\"

DOWN = b"\x1b[B"
ESC = b"\x1b"
ENTER = b"\r"
TAB = b"\t"
CTRL_U = b"\x15"


# -- a synthetic machine -------------------------------------------------------


class Machine(object):
    """A home on disk, a fake Herdr on a socket, and fake agents on PATH."""

    def __init__(self, folders: int, panes: int, recents: int, zoxide: int) -> None:
        self.tmp = tempfile.mkdtemp(prefix="ntb-")
        base = os.path.realpath(self.tmp)
        self.home = os.path.join(base, "home")
        self.root = os.path.join(self.home, "work")
        self.bin = os.path.join(base, "bin")
        self.config_dir = os.path.join(base, "config")
        self.state_dir = os.path.join(base, "state")
        self.socket_path = os.path.join(base, "herdr.sock")
        for path in (self.root, self.bin, self.config_dir, self.state_dir):
            os.makedirs(path)

        self.names = ["project-%04d" % index for index in range(folders)]
        for index, name in enumerate(self.names):
            folder = os.path.join(self.root, name)
            os.mkdir(folder)
            for child in ("src", "tests", "docs", "scripts")[: index % 4 + 1]:
                os.mkdir(os.path.join(folder, child))
            for child in ("README.md", "LICENSE", "Makefile", "pyproject.toml", "notes.txt")[
                : index % 5 + 1
            ]:
                open(os.path.join(folder, child), "w").close()
            if index % 2:
                os.mkdir(os.path.join(folder, ".git"))
                with open(os.path.join(folder, ".git", "HEAD"), "w") as handle:
                    handle.write("ref: refs/heads/main\n")

        # Fake agents, so "agents": "auto" finds the same two on every machine,
        # and a fake zoxide that prints a fixed frecency list.
        for agent in ("claude", "codex"):
            self._script(agent, "exit 0\n")
        listing = os.path.join(base, "zoxide.txt")
        with open(listing, "w") as handle:
            for index in range(zoxide):
                handle.write("%d.0 %s\n" % (1000 - index, os.path.join(self.root, self.names[
                    (index * 7) % len(self.names)])))
        if zoxide:
            self._script("zoxide", 'cat "%s"\n' % listing)

        with open(os.path.join(self.config_dir, "config.json"), "w") as handle:
            json.dump({"roots": [self.root], "zoxide": "auto"}, handle)
        entries = [
            {
                "path": os.path.join(self.root, self.names[(index * 3) % len(self.names)]),
                "agent": "claude" if index % 2 == 0 else "",
                "ts": 1700000000 - index,
            }
            for index in range(recents)
        ]
        with open(os.path.join(self.state_dir, "recent.json"), "w") as handle:
            json.dump({"version": 1, "entries": entries}, handle)

        self.server = FakeHerdr(self.socket_path, self.snapshot(panes)).start()

    def _script(self, name: str, body: str) -> None:
        path = os.path.join(self.bin, name)
        with open(path, "w") as handle:
            handle.write("#!/bin/sh\n" + body)
        os.chmod(path, 0o755)

    def snapshot(self, panes: int) -> Dict[str, Any]:
        spaces = [
            {"workspace_id": "w%d" % index, "label": "space-%d" % index, "number": index,
             "focused": index == 1}
            for index in range(1, 4)
        ]
        pane_list = []
        agents = []
        for index in range(panes):
            workspace = "w%d" % (index % 3 + 1)
            pane_id = "%s:p%d" % (workspace, index)
            folder = os.path.join(self.root, self.names[(index * 5) % len(self.names)])
            pane_list.append({"pane_id": pane_id, "workspace_id": workspace, "cwd": folder,
                              "focused": index == 0})
            if index % 2 == 0:
                agents.append({"pane_id": pane_id, "agent": "claude" if index % 4 else "codex",
                               "agent_status": ("working", "blocked", "idle", "done")[index % 4]})
        return {
            "version": "0.9.4",
            "focused_workspace_id": "w1",
            "focused_pane_id": "w1:p0",
            "workspaces": spaces,
            "panes": pane_list,
            "agents": agents,
        }

    def env(self) -> Dict[str, str]:
        return {
            "PATH": self.bin + os.pathsep + "/usr/bin:/bin",
            "HOME": self.home,
            "TERM": "xterm-256color",
            "LANG": "C.UTF-8",
            "LC_ALL": "C.UTF-8",
            "HERDR_SOCKET_PATH": self.socket_path,
            "HERDR_PLUGIN_CONFIG_DIR": self.config_dir,
            "HERDR_PLUGIN_STATE_DIR": self.state_dir,
            # Never reach a real Herdr through the CLI fallback.
            "HERDR_BIN_PATH": "/nonexistent/herdr",
        }

    def close(self) -> None:
        self.server.close()
        shutil.rmtree(self.tmp, ignore_errors=True)


class FakeHerdr(object):
    """Serves the slice of the Herdr socket protocol the plugin uses, instantly."""

    def __init__(self, path: str, data: Dict[str, Any]) -> None:
        self.path = path
        self._snapshot = self._encode({"snapshot": data})
        self.calls: List[Tuple[float, str]] = []
        self._lock = threading.Lock()
        self._closing = False
        self._server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self._server.bind(path)
        self._server.listen(64)

    @staticmethod
    def _encode(result: Dict[str, Any]) -> bytes:
        return json.dumps({"id": "newtab", "result": result}).encode() + b"\n"

    def start(self) -> "FakeHerdr":
        threading.Thread(target=self._accept, daemon=True).start()
        return self

    def reset(self) -> None:
        with self._lock:
            self.calls = []

    def first(self, method: str) -> Optional[float]:
        with self._lock:
            return next((at for at, name in self.calls if name == method), None)

    def wait_for(self, method: str, timeout: float = TIMEOUT) -> Optional[float]:
        deadline = time.perf_counter() + timeout
        while time.perf_counter() < deadline:
            found = self.first(method)
            if found is not None:
                return found
            time.sleep(0.001)
        return None

    def count(self) -> int:
        with self._lock:
            return len(self.calls)

    def close(self) -> None:
        self._closing = True
        try:
            self._server.close()
        except OSError:
            pass

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
            with self._lock:
                self.calls.append((time.perf_counter(), method))
            conn.sendall(self._answer(method))
        except (OSError, ValueError):
            pass
        finally:
            try:
                conn.close()
            except OSError:
                pass

    def _answer(self, method: str) -> bytes:
        if method == "session.snapshot":
            return self._snapshot
        if method in ("tab.create", "workspace.create"):
            return self._encode({
                "tab": {"tab_id": "w9:t1"},
                "workspace": {"workspace_id": "w9"},
                "root_pane": {"pane_id": "w9:p1"},
            })
        return self._encode({})


# -- a pseudo-terminal session -------------------------------------------------


class Session(object):
    """One plugin process on a pty, with every output byte timestamped.

    Spawned with posix_spawn rather than fork, so the child never holds this
    process's pages: on macOS the peak RSS wait4 reports would otherwise be the
    bench's, not the plugin's.
    """

    def __init__(self, argv: Sequence[str], env: Dict[str, str]) -> None:
        master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", SIZE[1], SIZE[0], 0, 0))
        self.master = master
        self.output = bytearray()
        self.chunks: List[Tuple[float, int]] = []
        program = shutil.which(argv[0]) or argv[0]
        self.started = time.perf_counter()
        self.pid = os.posix_spawn(
            program,
            list(argv),
            env,
            file_actions=[
                (os.POSIX_SPAWN_DUP2, slave, 0),
                (os.POSIX_SPAWN_DUP2, slave, 1),
                (os.POSIX_SPAWN_DUP2, slave, 2),
                (os.POSIX_SPAWN_CLOSE, master),
            ],
            setsid=True,
        )
        os.close(slave)
        self.rusage: Any = None
        self.status: Optional[int] = None
        self._answered = 0

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
        self.chunks.append((time.perf_counter(), len(self.output)))
        # Be the terminal: answer the background colour query the way most do.
        while True:
            found = self.output.find(APPEARANCE_QUERY, self._answered)
            if found < 0:
                break
            self._answered = found + len(APPEARANCE_QUERY)
            os.write(self.master, APPEARANCE_REPLY)
        return True

    def wait_for(self, marker: bytes, since: int = 0) -> float:
        deadline = time.perf_counter() + TIMEOUT
        while time.perf_counter() < deadline:
            found = self.output.find(marker, since)
            if found >= 0:
                end = found + len(marker)
                return next(at for at, length in self.chunks if length >= end)
            if not self.pump(0.05):
                break
        tail = bytes(self.output[-400:])
        raise RuntimeError("timed out waiting for %r; got:\n%r" % (marker, tail))

    def key(self, data: bytes) -> Optional[float]:
        """Send a key; the time from writing it to the last byte of its frame."""
        before = len(self.output)
        sent = time.perf_counter()
        os.write(self.master, data)
        deadline = sent + 2.0
        while len(self.output) == before and time.perf_counter() < deadline:
            if not self.pump(deadline - time.perf_counter()):
                return None
        if len(self.output) == before:
            return None
        last = self.settle()
        return ((last or self.chunks[-1][0]) - sent) * 1000.0

    def settle(self, quiet: float = QUIET) -> Optional[float]:
        last: Optional[float] = None
        while True:
            before = len(self.output)
            if not self.pump(quiet) or len(self.output) == before:
                return last
            last = self.chunks[-1][0]

    def send(self, data: bytes) -> float:
        sent = time.perf_counter()
        os.write(self.master, data)
        return sent

    def wait_exit(self) -> float:
        deadline = time.perf_counter() + TIMEOUT
        while time.perf_counter() < deadline:
            pid, status, usage = os.wait4(self.pid, os.WNOHANG)
            if pid:
                self.status = status
                self.rusage = usage
                return time.perf_counter()
            self.pump(0.001)
        raise RuntimeError("plugin did not exit")

    def cpu_ms(self) -> float:
        usage = self.rusage
        return (usage.ru_utime + usage.ru_stime) * 1000.0 if usage else 0.0

    def peak_kb(self) -> int:
        usage = self.rusage
        if not usage:
            return 0
        return int(usage.ru_maxrss / 1024) if sys.platform == "darwin" else int(usage.ru_maxrss)

    def close(self) -> None:
        if self.status is None:
            try:
                os.killpg(self.pid, signal.SIGKILL)
            except OSError:
                pass
            try:
                os.waitpid(self.pid, 0)
            except ChildProcessError:
                pass
        try:
            os.close(self.master)
        except OSError:
            pass


# -- scenarios -----------------------------------------------------------------


def browse(cmd: Sequence[str], machine: Machine) -> Dict[str, Any]:
    """Open, type and delete a query, walk the list, complete a path, then esc."""
    machine.server.reset()
    session = Session(cmd, machine.env())
    try:
        first = (session.wait_for(FRAME_DONE) - session.started) * 1000.0
        session.settle(0.15)
        typing = [session.key(bytes([char])) for char in QUERY]
        typing += [session.key(b"\x7f") for _ in QUERY]
        walking = [session.key(DOWN) for _ in range(12)]
        # Into the root: the "/" after "work" is the keystroke that lists it.
        pathing = [session.key(bytes([char])) for char in b"~/work/"]
        pathing += [session.key(bytes([char])) for char in b"pro"]
        session.key(CTRL_U)
        calls = machine.server.count()
        sent = session.send(ESC)
        exited = session.wait_exit()
        return {
            "first_frame_ms": first,
            "keystroke_ms": [value for value in typing if value is not None],
            "nav_ms": [value for value in walking if value is not None],
            "path_ms": [value for value in pathing if value is not None],
            "exit_ms": (exited - sent) * 1000.0,
            "cpu_ms": session.cpu_ms(),
            "peak_kb": session.peak_kb(),
            "bytes": len(session.output),
            "calls": calls,
        }
    finally:
        session.close()


def create(cmd: Sequence[str], machine: Machine, agent: bool) -> Dict[str, Any]:
    """Open, pick the agent chip or the shell, press Enter."""
    machine.server.reset()
    session = Session(cmd, machine.env())
    try:
        session.wait_for(FRAME_DONE)
        session.settle(0.15)
        # The first row is a recent opened with claude, so the chip row is on
        # claude already; shift+tab back to the shell for a create without one.
        if not agent:
            session.key(b"\x1b[Z")
        sent = session.send(ENTER)
        exited = session.wait_exit()
        created = machine.server.first("workspace.create") or machine.server.first("tab.create")
        started = machine.server.wait_for("agent.start") if agent else None
        result = {
            "create_ms": ((created or exited) - sent) * 1000.0,
            "exit_ms": (exited - sent) * 1000.0,
        }
        if agent:
            result["agent_ms"] = ((started - sent) * 1000.0) if started else None
        return result
    finally:
        session.close()


def command(cmd: Sequence[str], machine: Machine, flag: str) -> float:
    start = time.perf_counter()
    subprocess.run(list(cmd) + [flag], env=machine.env(), stdout=subprocess.DEVNULL,
                   stderr=subprocess.DEVNULL, check=False)
    return (time.perf_counter() - start) * 1000.0


# -- statistics and reporting --------------------------------------------------


def percentile(values: Sequence[float], fraction: float) -> float:
    values = [value for value in values if value is not None]
    if not values:
        return 0.0
    ordered = sorted(values)
    index = min(len(ordered) - 1, int(round(fraction * (len(ordered) - 1))))
    return ordered[index]


def run_workload(cmd: Sequence[str], spec: Tuple[str, int, int, int, int], runs: int):
    name, folders, panes, recents, zoxide = spec
    machine = Machine(folders, panes, recents, zoxide)
    try:
        browse(cmd, machine)  # warm the page cache and any bytecode caches
        browses = [browse(cmd, machine) for _ in range(runs)]
        creates = [create(cmd, machine, agent=False) for _ in range(max(3, runs // 2))]
        agents = [create(cmd, machine, agent=True) for _ in range(max(3, runs // 2))]
        lists = [command(cmd, machine, "--list") for _ in range(max(3, runs // 2))]

        def every(key: str) -> List[float]:
            return [value for run in browses for value in run[key]]

        def each(rows: List[Dict[str, Any]], key: str) -> List[float]:
            return [row[key] for row in rows if row.get(key) is not None]

        path = every("path_ms")
        return {
            "folders": folders,
            "panes": panes,
            "first_frame_p50_ms": percentile(each(browses, "first_frame_ms"), 0.5),
            "first_frame_p90_ms": percentile(each(browses, "first_frame_ms"), 0.9),
            "keystroke_p50_ms": percentile(every("keystroke_ms"), 0.5),
            "keystroke_p99_ms": percentile(every("keystroke_ms"), 0.99),
            "nav_p50_ms": percentile(every("nav_ms"), 0.5),
            "nav_p99_ms": percentile(every("nav_ms"), 0.99),
            "path_p50_ms": percentile(path, 0.5),
            "path_max_p50_ms": percentile([max(run["path_ms"]) for run in browses], 0.5),
            "exit_p50_ms": percentile(each(browses, "exit_ms"), 0.5),
            "create_p50_ms": percentile(each(creates, "create_ms"), 0.5),
            "create_exit_p50_ms": percentile(each(creates, "exit_ms"), 0.5),
            "agent_p50_ms": percentile(each(agents, "agent_ms"), 0.5),
            "cpu_p50_ms": percentile(each(browses, "cpu_ms"), 0.5),
            "peak_p50_kb": int(percentile(each(browses, "peak_kb"), 0.5)),
            "bytes_p50": int(percentile(each(browses, "bytes"), 0.5)),
            "herdr_calls_p50": int(percentile(each(browses, "calls"), 0.5)),
            "list_p50_ms": percentile(lists, 0.5),
        }
    finally:
        machine.close()


def run_all(cmd: Sequence[str], runs: int) -> Dict[str, Any]:
    results: Dict[str, Any] = {}
    for spec in WORKLOADS:
        results[spec[0]] = run_workload(cmd, spec, runs)
    machine = Machine(1, 1, 0, 0)
    try:
        command(cmd, machine, "--version")
        results["version_p50_ms"] = percentile(
            [command(cmd, machine, "--version") for _ in range(max(5, runs))], 0.5)
    finally:
        machine.close()
    return results


def machine_info() -> Dict[str, Any]:
    uname = os.uname()
    return {
        "platform": sys.platform,
        "machine": uname.machine,
        "cpus": os.cpu_count(),
        "bench_python": sys.version.split()[0],
    }


ROWS = (
    ("first_frame_p50_ms", "first frame p50", "ms"),
    ("first_frame_p90_ms", "first frame p90", "ms"),
    ("keystroke_p50_ms", "keystroke p50", "ms"),
    ("keystroke_p99_ms", "keystroke p99", "ms"),
    ("nav_p50_ms", "navigation p50", "ms"),
    ("nav_p99_ms", "navigation p99", "ms"),
    ("path_p50_ms", "path keystroke p50", "ms"),
    ("path_max_p50_ms", "listing keystroke", "ms"),
    ("exit_p50_ms", "esc to exit", "ms"),
    ("create_p50_ms", "enter to create", "ms"),
    ("create_exit_p50_ms", "enter to exit", "ms"),
    ("agent_p50_ms", "enter to agent start", "ms"),
    ("cpu_p50_ms", "cpu per session", "ms"),
    ("peak_p50_kb", "peak memory", "KB"),
    ("bytes_p50", "bytes per session", "B"),
    ("herdr_calls_p50", "herdr calls", ""),
    ("list_p50_ms", "--list", "ms"),
)


def _fmt(value: Any, unit: str) -> str:
    if value is None:
        return "-"
    if isinstance(value, float):
        text = "%.2f" % value if value < 100 else "%.0f" % value
    else:
        text = str(value)
    return (text + " " + unit).strip()


def _delta(now: float, was: float) -> str:
    if not was or now is None:
        return ""
    if now <= was:
        return "(%.1fx)" % (was / now) if now else "(inf)"
    return "(+%.0f%%)" % (100.0 * (now - was) / was)


def report(data: Dict[str, Any], baseline: Optional[Dict[str, Any]] = None) -> str:
    results = data["results"]
    names = [spec[0] for spec in WORKLOADS]
    width = 26 if baseline else 14
    lines = ["", "  herdr-newtab-plus process bench   %s" % " ".join(data["cmd"]), ""]
    header = "  %-22s" % "" + "".join(
        ("%" + str(width) + "s") % ("%s (%d)" % (name, results[name]["folders"]))
        for name in names
    )
    lines.append(header)
    lines.append("  " + "-" * (len(header) - 2))
    for key, label, unit in ROWS:
        cells = []
        for name in names:
            value = results[name].get(key)
            cell = _fmt(value, unit)
            if baseline:
                was = baseline["results"].get(name, {}).get(key)
                if was and value is not None:
                    cell += " " + _delta(value, was)
            cells.append(("%" + str(width) + "s") % cell)
        lines.append("  %-22s%s" % (label, "".join(cells)))
    lines.append("")
    version = results.get("version_p50_ms")
    cell = _fmt(version, "ms")
    if baseline and baseline["results"].get("version_p50_ms"):
        cell += " " + _delta(version, baseline["results"]["version_p50_ms"])
    lines.append("  %-22s%s" % ("--version", cell))
    lines.append("")
    return "\n".join(lines) + "\n"


def saved_path(name: str) -> str:
    return os.path.join(ROOT, "scripts", "bench-process-%s.json" % name)


def main(argv: Sequence[str]) -> int:
    args = list(argv)
    cmd = DEFAULT_CMD
    runs = 12
    if "--cmd" in args:
        cmd = shlex.split(args[args.index("--cmd") + 1])
        if not os.path.isabs(cmd[0]) and os.path.exists(os.path.join(ROOT, cmd[0])):
            cmd[0] = os.path.join(ROOT, cmd[0])
    if "--runs" in args:
        runs = int(args[args.index("--runs") + 1])

    data = {
        "cmd": [os.path.relpath(part, ROOT) if os.path.isabs(part) and part.startswith(ROOT)
                else part for part in cmd],
        "runs": runs,
        "machine": machine_info(),
        "results": run_all(cmd, runs),
    }
    if data["cmd"] and data["cmd"][0] == sys.executable:
        data["cmd"][0] = "python3"
        data["machine"]["plugin_python"] = sys.version.split()[0]

    if "--json" in args:
        sys.stdout.write(json.dumps(data, indent=2, sort_keys=True) + "\n")
        return 0
    baseline = None
    if "--compare" in args:
        with open(saved_path(args[args.index("--compare") + 1]), encoding="utf-8") as handle:
            baseline = json.load(handle)
    sys.stdout.write(report(data, baseline))
    if "--save" in args:
        target = saved_path(args[args.index("--save") + 1])
        with open(target, "w", encoding="utf-8") as handle:
            json.dump(data, handle, indent=2, sort_keys=True)
            handle.write("\n")
        sys.stdout.write("  saved %s\n\n" % os.path.relpath(target, ROOT))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
