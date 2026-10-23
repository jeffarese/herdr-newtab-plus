#!/usr/bin/env python3
"""Measure what the picker costs, and score it against a budget.

The popup is modal and short-lived: it opens on a keypress, takes a handful of
keystrokes, and closes. So the numbers that matter are the ones you feel --
how long the first frame takes to appear, how long a keystroke takes to show,
and how many bytes each frame costs to get onto a terminal that may be on the
far side of ssh and a multiplexer.

Everything here runs against a synthetic catalog, a synthetic session, and a
synthetic tree of folders made in a temp directory for the preview to read and
folded to ``~`` the way a home is, so the same command produces the same
workload on any machine and no Herdr is needed. The measurements split into
two kinds, and they are reported and used differently:

* **timings** -- machine-dependent, so they are scored but never gated. A
  loaded laptop is not a regression.
* **counts** -- bytes emitted per frame, and questions asked of the filesystem
  per frame. These are properties of the code alone, identical everywhere, and
  ``--check`` fails on them.

    python scripts/bench.py                 # report and score
    python scripts/bench.py --json out.json # machine-readable
    python scripts/bench.py --compare out.json
    python scripts/bench.py --check         # CI gate on the deterministic counts
"""

from __future__ import annotations

import argparse
import builtins
import json
import math
import os
import shutil
import subprocess
import sys
import tempfile
import time
from typing import Any, Callable, Dict, List, NamedTuple, Optional, Sequence, Tuple

sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "src"))

from herdr_newtab import paths  # noqa: E402
from herdr_newtab.agents import Agent  # noqa: E402
from herdr_newtab.app import Picker  # noqa: E402
from herdr_newtab.config import Config  # noqa: E402
from herdr_newtab.paint import Painter  # noqa: E402
from herdr_newtab.paths import (  # noqa: E402
    SOURCE_RECENT,
    SOURCE_ROOT,
    SOURCE_SPACE,
    SOURCE_ZOXIDE,
    Suggestion,
)
from herdr_newtab.state import Recents  # noqa: E402
from herdr_newtab.theme import Theme  # noqa: E402

# Rebound by ``stage`` to the fixture tree; the defaults are only what a frame
# would show if a scenario ran without it.
HOME = os.path.expanduser("~")
BASE = os.path.join(HOME, "workspace")

# How much of the catalog gets a folder on disk: the default catalog whole, so
# every row the scenarios land on previews something real. Rows past it show
# "not a folder yet", which costs the same lookups and is just as repeatable.
STAGED = 400

DIR_POOL = ("src", "tests", "docs", "scripts", "assets")
FILE_POOL = ("README.md", "LICENSE", "pyproject.toml", "Makefile", "CHANGELOG.md", ".gitignore")

# What a keystroke workload looks like: a name typed one character at a time,
# which is the interaction the picker exists for. It narrows from the whole
# catalog down to a couple of rows, so the cost of filtering is measured across
# its whole range rather than at one convenient point.
TYPED = "herdr-newtab"

# Enough of the alphabet to keep the width cache honest -- a catalog of pure
# ASCII would never exercise the per-character measuring path.
WIDE_NAMES = ("日本語プロジェクト", "café-ledger", "emoji-🚀-lab")


# -- workload --------------------------------------------------------------


def catalog(count: int) -> List[Suggestion]:
    """A deterministic catalog of the shape a real one has.

    Sources are mixed in roughly the proportion a working machine produces:
    a few spaces already open, a handful of recents, some zoxide frecency, and
    the rest scanned out of the roots.
    """
    sources = [SOURCE_SPACE] * 3 + [SOURCE_RECENT] * 8 + [SOURCE_ZOXIDE] * 20
    stems = (
        "herdr-bar",
        "herdr-newtab-plus",
        "webhook-relay",
        "mobile-client",
        "log-sink",
        "classifier",
        "notes",
        "atlas",
        "sandbox",
    )
    out: List[Suggestion] = []
    for index in range(count):
        if index < len(WIDE_NAMES):
            name = WIDE_NAMES[index]
        elif index < len(stems) + len(WIDE_NAMES):
            name = stems[index - len(WIDE_NAMES)]
        else:
            name = "%s-%03d" % (stems[index % len(stems)], index)
        source = sources[index] if index < len(sources) else SOURCE_ROOT
        # Depth varies, because the parent column is truncated from the middle
        # and a catalog of uniformly short parents would never reach that code.
        parent = BASE if index % 3 else os.path.join(BASE, "archive", "%02d" % (index % 7))
        out.append(
            Suggestion(
                path=os.path.join(parent, name),
                name=name,
                source=source,
                detail="Atlas" if source == SOURCE_SPACE else "",
                is_git=bool(index % 2),
            )
        )
    return out


def stage(items: Sequence[Suggestion]) -> str:
    """Make the folders the catalog names, each with contents of its own.

    The preview lists the folder under the cursor, so the bytes a navigation
    costs depend on what is in the folders it lands on. Real folders on the
    machine running this would make that a fact about the machine; these are
    the same everywhere, and vary from folder to folder the way real ones do,
    so moving between two of them has something to redraw.
    """
    global HOME, BASE
    root = tempfile.mkdtemp(prefix="newtab-bench-")
    old_base = BASE
    HOME, BASE = root, os.path.join(root, "workspace")
    paths._HOME = root  # folds the fixture to ~, as a real home would be
    for index, item in enumerate(items[:STAGED]):
        folder = BASE + item.path[len(old_base) :]
        os.makedirs(folder)
        for name in DIR_POOL[: index % len(DIR_POOL) + 1]:
            os.mkdir(os.path.join(folder, name))
        for name in FILE_POOL[: (index * 3) % len(FILE_POOL) + 1]:
            open(os.path.join(folder, name), "w").close()
        if item.is_git:
            os.mkdir(os.path.join(folder, ".git"))
            branch = "main" if index % 3 else "feature-%d" % index
            with open(os.path.join(folder, ".git", "HEAD"), "w") as handle:
                handle.write("ref: refs/heads/%s\n" % branch)
    return root


def snapshot(panes: int) -> Dict[str, Any]:
    """A session with several spaces open, each with panes standing somewhere.

    Pane count is a parameter because the tab-versus-space decision walks every
    pane on every frame, so it is one of the two things that make a frame cost
    more than the frame before it.
    """
    spaces = [
        {"workspace_id": "w%d" % index, "label": "space-%d" % index, "number": index,
         "focused": index == 1}
        for index in range(1, 4)
    ]
    return {
        "version": "0.7.5",
        "focused_workspace_id": "w1",
        "focused_pane_id": "w1:p1",
        "workspaces": spaces,
        "panes": [
            {
                "pane_id": "w%d:p%d" % ((index % 3) + 1, index),
                "workspace_id": "w%d" % ((index % 3) + 1),
                "cwd": os.path.join(BASE, "project-%02d" % index),
            }
            for index in range(panes)
        ],
        "agents": [],
    }


def chips() -> List[Agent]:
    return [
        Agent("", "shell"),
        Agent("claude", "claude"),
        Agent("codex", "codex"),
        Agent("opencode", "opencode"),
        Agent("pi", "pi"),
    ]


def picker(items: Sequence[Suggestion], session: Dict[str, Any]) -> Picker:
    return Picker(
        snapshot=session,
        catalog=items,
        chips=chips(),
        config=Config({"roots": []}),
        recents=Recents(None),
        theme=Theme(),
        base=BASE,
    )


# -- instruments -----------------------------------------------------------


class Calls(object):
    """Count the questions a block asks the filesystem.

    A frame that stats the filesystem is a frame whose cost is not its own: on
    a network mount or a cold cache the lookup dominates everything else this
    module measures.

    What is counted is the question, not the syscalls it takes to answer it.
    ``realpath`` costs one lstat per path component, so counting primitives
    would make the number depend on how deep the home directory happens to be
    -- and a measurement that changes with the machine is no longer a fact
    about the code. Nested calls are suppressed rather than added, since
    ``isdir`` is built on ``stat`` and asking once should be priced once.
    Opening a file is a question too: it is how the preview reads a branch.
    """

    WATCHED_PATH = ("isdir", "isfile", "exists", "lexists", "islink", "realpath")
    WATCHED_OS = ("scandir", "listdir", "stat", "lstat", "readlink", "access", "getcwd")

    def __init__(self) -> None:
        self.total = 0
        self._depth = 0
        self._saved: List[Tuple[Any, str, Any]] = []

    def _wrap(self, module: Any, name: str) -> None:
        original = getattr(module, name)

        def counted(*args: Any, **kwargs: Any) -> Any:
            if not self._depth:
                self.total += 1
            self._depth += 1
            try:
                return original(*args, **kwargs)
            finally:
                self._depth -= 1

        self._saved.append((module, name, original))
        setattr(module, name, counted)

    def __enter__(self) -> "Calls":
        for name in self.WATCHED_PATH:
            self._wrap(os.path, name)
        for name in self.WATCHED_OS:
            self._wrap(os, name)
        self._wrap(builtins, "open")
        self.total = 0
        self._depth = 0
        return self

    def __exit__(self, *exc: Any) -> None:
        for module, name, original in reversed(self._saved):
            setattr(module, name, original)
        self._saved = []


class Samples(NamedTuple):
    values: Tuple[float, ...]

    def quantile(self, fraction: float) -> float:
        ordered = sorted(self.values)
        if not ordered:
            return 0.0
        index = min(len(ordered) - 1, max(0, int(round(fraction * (len(ordered) - 1)))))
        return ordered[index]

    @property
    def best(self) -> float:
        return min(self.values) if self.values else 0.0

    @property
    def median(self) -> float:
        return self.quantile(0.5)

    @property
    def p95(self) -> float:
        return self.quantile(0.95)


def measure(action: Callable[[], None], iterations: int, warmup: int = 20) -> Samples:
    for _ in range(warmup):
        action()
    values: List[float] = []
    for _ in range(iterations):
        start = time.perf_counter()
        action()
        values.append((time.perf_counter() - start) * 1000.0)
    return Samples(tuple(values))


# -- scenarios -------------------------------------------------------------


def bench_keystroke(
    items: Sequence[Suggestion], session: Dict[str, Any], size: Tuple[int, int], iterations: int
) -> Dict[str, Any]:
    """One character typed: filter, compose, diff, and the bytes that result.

    Timed per character across the whole word, because the first keystroke
    filters the entire catalog and the last one filters almost nothing -- an
    average over one of those alone would flatter or libel the picker.
    """
    width, height = size
    filter_ms: List[float] = []
    compose_ms: List[float] = []
    diff_ms: List[float] = []
    total_ms: List[float] = []
    byte_counts: List[int] = []

    for length in range(1, len(TYPED) + 1):
        prefix, char = TYPED[: length - 1], TYPED[length - 1]
        app = picker(items, session)
        painter = Painter()

        def restore() -> None:
            app.set_query(prefix)
            painter.frame(app.frame(width, height), width, height)

        def keystroke() -> None:
            start = time.perf_counter()
            app.insert(char)
            after_filter = time.perf_counter()
            rows = app.frame(width, height)
            after_compose = time.perf_counter()
            patch = painter.frame(rows, width, height)
            done = time.perf_counter()
            filter_ms.append((after_filter - start) * 1000.0)
            compose_ms.append((after_compose - after_filter) * 1000.0)
            diff_ms.append((done - after_compose) * 1000.0)
            total_ms.append((done - start) * 1000.0)
            byte_counts.append(len(patch.encode("utf-8")))

        for _ in range(max(3, iterations // len(TYPED))):
            restore()
            keystroke()

    return {
        "total_ms": Samples(tuple(total_ms)),
        "filter_ms": Samples(tuple(filter_ms)),
        "compose_ms": Samples(tuple(compose_ms)),
        "diff_ms": Samples(tuple(diff_ms)),
        "bytes": sorted(byte_counts)[len(byte_counts) // 2],
        "bytes_max": max(byte_counts),
    }


def bench_navigation(
    items: Sequence[Suggestion], session: Dict[str, Any], size: Tuple[int, int], iterations: int
) -> Dict[str, Any]:
    """Down-arrow: nothing is refiltered, one band moves, the list may scroll.

    The cheapest thing the picker does and the thing it does most, so it is
    where a diffing painter has to earn its keep.
    """
    width, height = size
    app = picker(items, session)
    painter = Painter()
    painter.frame(app.frame(width, height), width, height)
    byte_counts: List[int] = []

    def step() -> None:
        app.move(1)
        patch = painter.frame(app.frame(width, height), width, height)
        byte_counts.append(len(patch.encode("utf-8")))

    samples = measure(step, iterations)
    return {
        "ms": samples,
        "bytes": sorted(byte_counts)[len(byte_counts) // 2],
        "bytes_max": max(byte_counts),
    }


def bench_agent(
    items: Sequence[Suggestion], session: Dict[str, Any], size: Tuple[int, int], iterations: int
) -> Dict[str, Any]:
    """Shift-Tab: one row changes and nothing else does. The floor for a frame."""
    width, height = size
    app = picker(items, session)
    painter = Painter()
    painter.frame(app.frame(width, height), width, height)
    byte_counts: List[int] = []

    def step() -> None:
        app.cycle_agent(1)
        patch = painter.frame(app.frame(width, height), width, height)
        byte_counts.append(len(patch.encode("utf-8")))

    samples = measure(step, iterations)
    return {
        "ms": samples,
        "bytes": sorted(byte_counts)[len(byte_counts) // 2],
    }


def bench_repaint(
    items: Sequence[Suggestion], session: Dict[str, Any], size: Tuple[int, int], iterations: int
) -> Dict[str, Any]:
    """A full repaint -- the first frame, and every frame after a resize.

    Also the ceiling every other frame is priced against: the diffing painter
    is only worth its complexity by the distance between this and the rest.
    """
    width, height = size
    app = picker(items, session)
    painter = Painter()
    sizes: List[int] = []

    def step() -> None:
        painter.reset()
        patch = painter.frame(app.frame(width, height), width, height)
        sizes.append(len(patch.encode("utf-8")))

    samples = measure(step, iterations)
    return {"ms": samples, "bytes": sorted(sizes)[len(sizes) // 2]}


def bench_calls(
    items: Sequence[Suggestion], session: Dict[str, Any], size: Tuple[int, int]
) -> Dict[str, int]:
    """Filesystem calls per frame, split by what provokes them.

    Every scenario starts from a cleared cache and a fresh picker, because the
    picker remembers what it has looked up and the popup it models is opened
    fresh each time. Without that, whichever scenario ran first would pay for
    the ones after it and the numbers would depend on their order.

    A row is counted twice over: landing on it for the first time, which is
    what moving down a list does, and coming back to it, which is what holding
    a key or retyping does. The first is the honest cost of navigating; the
    second is what the picker owes for standing still.
    """
    width, height = size

    def fresh() -> Tuple[Picker, Painter]:
        paths.clear_cache()
        app = picker(items, session)
        painter = Painter()
        painter.frame(app.frame(width, height), width, height)
        return app, painter

    app, _ = fresh()
    with Calls() as counter:
        app.move(1)
        app.frame(width, height)
        nav = counter.total

    app, _ = fresh()
    app.move(1)
    app.frame(width, height)
    app.move(-1)
    app.frame(width, height)
    with Calls() as counter:
        app.move(1)  # back to a row already looked up
        app.frame(width, height)
        nav_repeat = counter.total

    paths.clear_cache()
    app_typed = picker(items, session)
    app_typed.set_query(TYPED[:-1])
    painter_typed = Painter()
    painter_typed.frame(app_typed.frame(width, height), width, height)
    with Calls() as counter:
        app_typed.insert(TYPED[-1])
        app_typed.frame(width, height)
        keystroke = counter.total

    paths.clear_cache()
    return {"nav": nav, "nav_repeat": nav_repeat, "keystroke": keystroke}


COLD_START = """
import os, sys, time
sys.path.insert(0, %r)
start = time.perf_counter()
import herdr_newtab.app, herdr_newtab.cli, herdr_newtab.paths, herdr_newtab.render
imported = time.perf_counter()
sys.stderr.write("%%.4f %%.4f\\n" %% ((imported - start) * 1000.0, 0.0))
"""


def bench_startup(root: str, iterations: int) -> Dict[str, Any]:
    """Cold start, in a subprocess, because import cost is only paid once.

    The interpreter's own floor is measured separately and reported beside it:
    a plugin cannot be faster than ``python3 -c pass``, and knowing how much of
    the wait is not ours is the difference between optimising and flailing.
    """
    source = os.path.join(root, "src")
    floor: List[float] = []
    total: List[float] = []
    imports: List[float] = []

    for _ in range(iterations):
        start = time.perf_counter()
        subprocess.run([sys.executable, "-c", "pass"], check=True)
        floor.append((time.perf_counter() - start) * 1000.0)

        start = time.perf_counter()
        done = subprocess.run(
            [sys.executable, "-c", COLD_START % source],
            check=True,
            stderr=subprocess.PIPE,
        )
        total.append((time.perf_counter() - start) * 1000.0)
        imports.append(float(done.stderr.decode().split()[0]))

    return {
        "total_ms": Samples(tuple(total)),
        "floor_ms": Samples(tuple(floor)),
        "import_ms": Samples(tuple(imports)),
    }


def probe_roots(config: Config) -> Dict[str, Any]:
    """What scanning this machine's real roots costs.

    Reported but never scored: it measures a filesystem, not this code, and a
    cold NFS mount would make the score say something it does not mean.
    """
    # Configured roots are stored as written, tildes and all, and expanded
    # where they are used -- so a probe that skipped the expansion would find
    # nothing on every machine and quietly report a zero.
    found = [paths.normalize(root) for root in config.roots]
    found = [root for root in found if root and paths.is_directory(root)]
    paths.clear_cache()
    start = time.perf_counter()
    entries = sum(len(paths.subdirectories(root, False)) for root in found)
    elapsed = (time.perf_counter() - start) * 1000.0
    paths.clear_cache()
    return {"roots": len(found), "folders": entries, "ms": elapsed}


# -- score -----------------------------------------------------------------


class Metric(NamedTuple):
    key: str
    label: str
    unit: str
    budget: float  # at or under this, the metric is as good as it needs to be
    limit: float  # at or over this, it is bad enough to be worth nothing
    weight: int
    fixed: bool  # true when the value is a property of the code, not the machine


# Budgets are anchored to what the interaction requires, not to what the code
# currently happens to do -- a budget read off the current numbers only ever
# says "no change", which is not a measurement.
#
# * A keystroke under ~3ms is invisible beside the terminal's own frame time,
#   and a first frame under ~120ms reads as "it was already there".
# * A keystroke changes the input by one character and re-sorts the rows under
#   it, so a few hundred bytes is what it needs; a navigation moves a selection
#   band, and a band is full-bleed by design, so two whole rows is its honest
#   floor -- about a kilobyte at this width. Both budgets are set to what the
#   drawing genuinely requires, so what they catch is a diff that has stopped
#   working rather than a layout somebody meant to change.
# * Landing on a folder asks the filesystem three things at most: what is in
#   it, for the preview, which also says whether it exists at all; which branch
#   it is on, when it is a checkout; and what it is behind its symlinks, which
#   is how the picker knows whether that folder already has a space. Three is
#   therefore the budget, and standing still should cost nothing. The session
#   it reasons from is a snapshot taken when the popup opened and cannot change
#   while it is up, so anything spent re-deriving that is spent again on every
#   keystroke.
# * The preview redraws on every navigation, so a navigation also pays for the
#   lines of the folder it lands on that differ from the one it left. The
#   benchmark's folders differ the way real ones do, and the budget stays where
#   it was: the preview has to fit inside it.
METRICS = (
    Metric("startup_ms", "cold start", "ms", 120.0, 700.0, 20, False),
    Metric("keystroke_ms", "keystroke p95", "ms", 3.0, 25.0, 25, False),
    Metric("nav_ms", "navigation p95", "ms", 1.5, 12.0, 15, False),
    Metric("keystroke_bytes", "bytes per keystroke", "B", 400.0, 8000.0, 15, True),
    Metric("nav_bytes", "bytes per navigation", "B", 1000.0, 8000.0, 15, True),
    Metric("frame_calls", "lookups per frame", "", 3.0, 32.0, 10, True),
)

# ``--check`` gates the fixed metrics, generously: it is there to catch a frame
# that stopped diffing or started walking the filesystem, not to freeze the
# layout. Lookups are counted as questions rather than syscalls, so unlike the
# byte counts they do not shift with the terminal or the depth of a home
# directory, and the ceiling can sit close to the budget.
CEILINGS = {
    "keystroke_bytes": 800,
    "nav_bytes": 1400,
    "frame_calls": 4,
}

GRADES = ((95.0, "A+"), (90.0, "A"), (80.0, "B"), (70.0, "C"), (60.0, "D"), (0.0, "F"))


def score_of(value: float, metric: Metric) -> float:
    """100 at the budget, 0 at the limit, log-scaled between.

    Log rather than linear because these quantities are perceived in ratios:
    the gap between 3ms and 6ms is the same amount of worse as 6ms to 12ms.
    """
    if value <= metric.budget:
        return 100.0
    if value >= metric.limit:
        return 0.0
    return 100.0 * math.log(metric.limit / value) / math.log(metric.limit / metric.budget)


def grade_of(score: float) -> str:
    for floor, letter in GRADES:
        if score >= floor:
            return letter
    return "F"


def collect(args: argparse.Namespace, startup_runs: int) -> Dict[str, Any]:
    """Run every scenario. ``startup_runs`` of zero skips the subprocess work.

    Only the score needs cold start, and it is by far the slowest thing here --
    ``--check`` looks at counts alone, so it does not pay for it.
    """
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    size = tuple(int(part) for part in args.size.lower().split("x"))
    items = catalog(args.catalog)
    session = snapshot(args.panes)

    keystroke = bench_keystroke(items, session, size, args.iterations)
    navigation = bench_navigation(items, session, size, args.iterations)
    agent = bench_agent(items, session, size, args.iterations)
    repaint = bench_repaint(items, session, size, max(20, args.iterations // 4))
    calls = bench_calls(items, session, size)
    startup = bench_startup(root, startup_runs) if startup_runs else None

    values = {
        "startup_ms": startup["total_ms"].median if startup else 0.0,
        "keystroke_ms": keystroke["total_ms"].p95,
        "nav_ms": navigation["ms"].p95,
        "keystroke_bytes": float(keystroke["bytes"]),
        "nav_bytes": float(navigation["bytes"]),
        "frame_calls": float(max(calls["nav"], calls["keystroke"])),
    }

    scores = {metric.key: score_of(values[metric.key], metric) for metric in METRICS}
    total = sum(scores[metric.key] * metric.weight for metric in METRICS) / sum(
        metric.weight for metric in METRICS
    )

    return {
        "environment": {
            "python": sys.version.split()[0],
            "platform": sys.platform,
            "size": "%dx%d" % size,
            "catalog": args.catalog,
            "panes": args.panes,
        },
        "values": values,
        "scores": scores,
        "score": total,
        "grade": grade_of(total),
        "detail": {
            "keystroke": {
                "median_ms": keystroke["total_ms"].median,
                "p95_ms": keystroke["total_ms"].p95,
                "filter_ms": keystroke["filter_ms"].median,
                "compose_ms": keystroke["compose_ms"].median,
                "diff_ms": keystroke["diff_ms"].median,
                "bytes": keystroke["bytes"],
                "bytes_max": keystroke["bytes_max"],
            },
            "navigation": {
                "median_ms": navigation["ms"].median,
                "p95_ms": navigation["ms"].p95,
                "bytes": navigation["bytes"],
                "bytes_max": navigation["bytes_max"],
            },
            "agent": {"median_ms": agent["ms"].median, "bytes": agent["bytes"]},
            "repaint": {"median_ms": repaint["ms"].median, "bytes": repaint["bytes"]},
            "startup": {
                "total_ms": startup["total_ms"].median if startup else 0.0,
                "floor_ms": startup["floor_ms"].median if startup else 0.0,
                "import_ms": startup["import_ms"].median if startup else 0.0,
            },
            "calls": calls,
            "roots": probe_roots(Config({})),
        },
    }


# -- report ----------------------------------------------------------------


def _num(value: float, unit: str) -> str:
    if unit == "ms":
        return "%.2f ms" % value
    if unit == "B":
        return "%d B" % int(round(value))
    return "%d" % int(round(value))


def report(result: Dict[str, Any], previous: Optional[Dict[str, Any]]) -> None:
    environment = result["environment"]
    detail = result["detail"]
    write = sys.stdout.write

    write("\nherdr-newtab-plus performance\n")
    write(
        "  python %s on %s · %s popup · %d folders · %d panes\n\n"
        % (
            environment["python"],
            environment["platform"],
            environment["size"],
            environment["catalog"],
            environment["panes"],
        )
    )

    write("  %-22s %12s %10s %6s\n" % ("", "measured", "budget", "score"))
    for metric in METRICS:
        value = result["values"][metric.key]
        score = result["scores"][metric.key]
        delta = ""
        if previous and metric.key in previous.get("values", {}):
            before = previous["values"][metric.key]
            if before:
                change = (value - before) / before * 100.0
                if abs(change) >= 2.0:
                    delta = "  %+.0f%%" % change
        write(
            "  %-22s %12s %10s %6.0f%s\n"
            % (
                metric.label + ("" if metric.fixed else " *"),
                _num(value, metric.unit),
                _num(metric.budget, metric.unit),
                score,
                delta,
            )
        )

    write("\n  score %.0f / 100   %s\n" % (result["score"], result["grade"]))
    if previous:
        write("  was   %.0f / 100   %s\n" % (previous["score"], previous["grade"]))
    write("  * machine-dependent; the rest are the same on every machine\n")

    write("\n  where a keystroke goes\n")
    keystroke = detail["keystroke"]
    for label, key in (("filter", "filter_ms"), ("compose", "compose_ms"), ("diff", "diff_ms")):
        share = keystroke[key] / keystroke["median_ms"] * 100.0 if keystroke["median_ms"] else 0.0
        write("    %-10s %8.3f ms  %3.0f%%\n" % (label, keystroke[key], share))

    write("\n  what each frame costs to send\n")
    repaint_bytes = detail["repaint"]["bytes"]
    for label, item in (
        ("first / resize", detail["repaint"]),
        ("keystroke", keystroke),
        ("navigation", detail["navigation"]),
        ("agent", detail["agent"]),
    ):
        saved = 100.0 - (item["bytes"] / float(repaint_bytes) * 100.0) if repaint_bytes else 0.0
        write(
            "    %-16s %6d B%s\n"
            % (
                label,
                item["bytes"],
                "   the whole screen" if label == "first / resize"
                else "   %.0f%% of a repaint saved" % saved,
            )
        )

    startup = detail["startup"]
    write("\n  cold start\n")
    write("    interpreter    %8.1f ms\n" % startup["floor_ms"])
    write("    imports        %8.1f ms\n" % startup["import_ms"])
    write("    total          %8.1f ms\n" % startup["total_ms"])

    calls = detail["calls"]
    write("\n  what a frame asks the filesystem\n")
    write("    arriving at a folder   %d\n" % calls["nav"])
    write("    returning to one       %d\n" % calls["nav_repeat"])
    write("    typing                 %d\n" % calls["keystroke"])

    roots = detail["roots"]
    write("\n  this machine (not scored)\n")
    write(
        "    scanning %d root%s found %d folders in %.1f ms\n"
        % (roots["roots"], "" if roots["roots"] == 1 else "s", roots["folders"], roots["ms"])
    )
    write("\n")


def invariants(size: Tuple[int, int], panes: int) -> List[Tuple[str, bool, str, bool]]:
    """Facts that hold on every machine, whatever the numbers on it are.

    An absolute ceiling has to be loose enough to survive a different home
    directory or a different terminal, which makes it a blunt instrument. These
    are the sharp one: a frame's filesystem cost must not depend on how many
    folders are in the catalog, because a frame only ever draws the handful of
    rows that fit. The day something starts statting per row, this says so
    however fast the machine it ran on happened to be.
    """
    small = bench_calls(catalog(50), snapshot(panes), size)
    large = bench_calls(catalog(1600), snapshot(panes), size)
    checks = [
        (
            "lookups per %s frame do not scale with the catalog" % kind,
            small[kind] == large[kind],
            "%d at 50 folders, %d at 1600" % (small[kind], large[kind]),
            True,
        )
        for kind in ("nav", "keystroke")
    ]

    # A frame's filesystem cost must track the snapshot it reasons from once,
    # not once per frame; growing with panes is the symptom that it is being
    # re-derived behind every keystroke, which is exactly what a busy session
    # would make expensive.
    few = bench_calls(catalog(400), snapshot(2), size)
    many = bench_calls(catalog(400), snapshot(32), size)
    checks.append(
        (
            "lookups per frame do not scale with open panes",
            few["nav"] == many["nav"],
            "%d at 2 panes, %d at 32" % (few["nav"], many["nav"]),
            True,
        )
    )

    # Standing on a folder already looked at should cost nothing new: the
    # snapshot cannot change while the popup is up, so a frame that keeps
    # asking is a frame paying twice for one answer.
    steady = bench_calls(catalog(400), snapshot(8), size)
    checks.append(
        (
            "returning to a folder costs less than arriving at it",
            steady["nav_repeat"] < steady["nav"],
            "%d on arrival, %d on return" % (steady["nav"], steady["nav_repeat"]),
            True,
        )
    )
    return checks


def check(result: Dict[str, Any], size: Tuple[int, int], panes: int) -> int:
    """Fail on the measurements that do not depend on the machine running them."""
    failures: List[str] = []

    sys.stdout.write("\n  ceilings\n")
    for metric in METRICS:
        if not metric.fixed:
            continue
        ceiling = CEILINGS.get(metric.key)
        if ceiling is None:
            continue
        value = result["values"][metric.key]
        ok = value <= ceiling
        sys.stdout.write(
            "    %s  %-24s %8s   ceiling %s\n"
            % ("ok  " if ok else "FAIL", metric.label, _num(value, metric.unit),
               _num(ceiling, metric.unit))
        )
        if not ok:
            failures.append(metric.label)

    sys.stdout.write("\n  invariants\n")
    for label, ok, detail, fatal in invariants(size, panes):
        mark = "ok  " if ok else ("FAIL" if fatal else "note")
        sys.stdout.write("    %s  %-58s %s\n" % (mark, label, detail))
        if not ok and fatal:
            failures.append(label)

    if failures:
        sys.stdout.write("\n  regressed: %s\n" % "; ".join(failures))
        return 1
    sys.stdout.write("\n  every machine-independent measurement is within budget\n")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        # What the shipped popup gives the picker: `height - 2` rows and
        # `width - 3` columns of the manifest's 110x30.
        "--size", default="107x28", help="popup size, WIDTHxHEIGHT"
    )
    parser.add_argument("--catalog", type=int, default=400, help="folders in the catalog")
    parser.add_argument("--panes", type=int, default=8, help="panes in the session snapshot")
    parser.add_argument("--iterations", type=int, default=300)
    parser.add_argument("--startup-runs", type=int, default=5)
    parser.add_argument("--json", metavar="FILE", help="write the full result as JSON")
    parser.add_argument("--compare", metavar="FILE", help="show the change against a saved run")
    parser.add_argument(
        "--check", action="store_true", help="exit nonzero on a fixed-cost regression"
    )
    args = parser.parse_args()

    fixture = stage(catalog(max(args.catalog, STAGED)))
    try:
        return run(args)
    finally:
        shutil.rmtree(fixture, ignore_errors=True)


def run(args: argparse.Namespace) -> int:
    result = collect(args, 0 if args.check else args.startup_runs)

    if args.json:
        with open(args.json, "w") as handle:
            json.dump(result, handle, indent=2, sort_keys=True)
            handle.write("\n")

    previous: Optional[Dict[str, Any]] = None
    if args.compare:
        with open(args.compare) as handle:
            previous = json.load(handle)

    if args.check:
        size = tuple(int(part) for part in args.size.lower().split("x"))
        sys.stdout.write("\nmachine-independent measurements")
        return check(result, size, args.panes)

    report(result, previous)
    return 0


if __name__ == "__main__":
    sys.exit(main())
