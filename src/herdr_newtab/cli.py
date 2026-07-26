"""Command line entry point.

The popup's whole job is to answer two questions and get out of the way, so the
order matters: the tab or space is created while the user is still looking at
the picker's last frame, and only the agent -- which Herdr waits on until it is
genuinely ready for input, and that can take many seconds -- is handed to a
detached child. The popup closes on the tab appearing, not on the agent booting.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from typing import List, Optional

from . import __version__, agents, paths
from .app import Choice, Picker
from .client import HerdrClient, HerdrError, resolve_bin
from .config import Config
from .session import SPACE, TAB, agent_name, created_ids, focused_cwd, live_agent_names
from .state import Recents
from .term import Terminal, detect_appearance
from .theme import Theme, default_selection_background

USAGE = """herdr-newtab-plus - new tab, with a folder and an optional agent

usage: run.py [options]

options:
  --list      print the folders the picker would offer, as JSON
  --doctor    print environment diagnostics
  --version   print the plugin version
  --help      show this message

Normally Herdr launches this itself:
  herdr plugin action invoke herdr-newtab-plus.open
"""

# Hidden: how the detached child re-enters to start the agent.
START_AGENT = "--start-agent"


def state_path() -> Optional[str]:
    state_dir = os.environ.get("HERDR_PLUGIN_STATE_DIR")
    if not state_dir:
        return None
    return os.path.join(state_dir, "recent.json")


def log_path() -> Optional[str]:
    state_dir = os.environ.get("HERDR_PLUGIN_STATE_DIR")
    if not state_dir:
        return None
    return os.path.join(state_dir, "agent-start.log")


def _fail(message: str) -> int:
    sys.stderr.write("herdr-newtab-plus: %s\n" % message)
    return 2


def _catalog(client: HerdrClient, config: Config, recents: Recents):
    snapshot = client.snapshot()
    catalog = paths.build_catalog(
        snapshot,
        config.roots,
        recents.folders(),
        config.hidden,
        config.wants_zoxide(paths.zoxide_installed()),
    )
    return snapshot, catalog


def _skip_summary(chips, config: Config) -> str:
    """Which offered agents ctrl+Y can actually arm, and with what.

    Worth printing: the answer is a table of flags per CLI, so "the toggle does
    nothing on this chip" is otherwise only discoverable by pressing it.
    """
    parts = []
    for chip in chips:
        flags = agents.skip_permission_flags(chip.kind, config.skip_permission_flags)
        if flags:
            parts.append("%s (%s)" % (chip.label, " ".join(flags)))
    return ", ".join(parts)


def _create_mode(config: Config, recents: Recents) -> str:
    """The mode in force, and whether the toggle or the config file set it."""
    remembered = recents.create_mode()
    if remembered is None:
        return "%s (config)" % config.create
    return "%s (remembered; config says %s)" % (remembered, config.create)


def _doctor(client: HerdrClient, config: Config) -> int:
    recents = Recents(state_path())
    chips = agents.available(config.agents)
    lines = [
        "herdr-newtab-plus %s" % __version__,
        "python           %s" % sys.version.split()[0],
        "herdr bin        %s" % client.bin_path,
        "socket           %s" % (client.socket_path or "(unset)"),
        "socket usable    %s" % ("yes" if client.socket_ok else "no, using the CLI"),
        "config           %s" % (Config.path() or "(no HERDR_PLUGIN_CONFIG_DIR)"),
        "state            %s" % (state_path() or "(no HERDR_PLUGIN_STATE_DIR)"),
        "create mode      %s" % _create_mode(config, recents),
        "roots            %s" % ", ".join(config.roots),
        "zoxide           %s"
        % ("yes" if config.wants_zoxide(paths.zoxide_installed()) else "no"),
        "agents           %s" % ", ".join(chip.label for chip in chips),
        "skip permissions %s" % (_skip_summary(chips, config) or "(none of the above)"),
        "stdin is a tty   %s" % ("yes" if sys.stdin.isatty() else "no"),
    ]
    try:
        snapshot, catalog = _catalog(client, config, recents)
    except HerdrError as error:
        lines.append("herdr            unreachable: %s" % error)
        sys.stdout.write("\n".join(lines) + "\n")
        return 1
    lines.append("herdr            %s" % snapshot.get("version", "?"))
    lines.append("base cwd         %s" % (focused_cwd(snapshot) or "(unknown)"))
    lines.append(
        "catalog          %d folders (%d already have a space)"
        % (len(catalog), sum(1 for item in catalog if item.source == paths.SOURCE_SPACE))
    )
    sys.stdout.write("\n".join(lines) + "\n")
    return 0


def _list(client: HerdrClient, config: Config) -> int:
    recents = Recents(state_path())
    _, catalog = _catalog(client, config, recents)
    payload = [
        {
            "path": item.path,
            "name": item.name,
            "source": item.source,
            "detail": item.detail,
            "git": item.is_git,
        }
        for item in catalog
    ]
    sys.stdout.write(json.dumps(payload, indent=2) + "\n")
    return 0


# -- creating --------------------------------------------------------------


def create(client: HerdrClient, choice: Choice) -> str:
    """Make the tab or space and focus it. Returns the new root pane's id."""
    if choice.target.kind == TAB and choice.target.workspace_id:
        result = client.create_tab(choice.target.workspace_id, choice.folder)
    else:
        label = os.path.basename(choice.folder) or None
        result = client.create_workspace(choice.folder, label)
    _, pane_id = created_ids(result)
    if not pane_id:
        raise HerdrError("herdr created the %s but reported no pane to use" % choice.target.kind)
    return pane_id


def spawn_agent(
    choice: Choice, pane_id: str, name: str, args: Optional[List[str]] = None
) -> None:
    """Hand the agent start to a detached child so the popup can close now."""
    payload = json.dumps(
        {
            "name": name,
            "kind": choice.agent.kind,
            "pane": pane_id,
            "args": list(args or []),
        }
    )
    command = [sys.executable, os.path.abspath(sys.argv[0]), START_AGENT, payload]

    log = None
    target = log_path()
    if target:
        try:
            os.makedirs(os.path.dirname(target), exist_ok=True)
            log = open(target, "a", encoding="utf-8")
        except OSError:
            log = None
    sink = log or subprocess.DEVNULL
    try:
        subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=sink,
            stderr=sink,
            start_new_session=True,
            env=agents.agent_env(),
            cwd=os.path.dirname(os.path.abspath(sys.argv[0])) or None,
        )
    except OSError as error:
        sys.stderr.write("herdr-newtab-plus: could not start %s: %s\n" % (choice.agent.kind, error))
    finally:
        if log is not None:
            log.close()


# A pane is born running a shell's startup files, and Herdr refuses to hand it
# to an agent until that shell owns the foreground and is idle. A heavy zshrc
# takes a moment, so the detached child waits it out rather than giving up on
# the first no.
SHELL_WAIT_SECONDS = 25.0
SHELL_POLL_SECONDS = 0.35
NAME_ATTEMPTS = 4


def _transient(message: str) -> bool:
    """Whether the pane is merely not ready yet, as opposed to unusable."""
    text = message.lower()
    return "available shell" in text or "foreground" in text or "not ready" in text


def _start_agent(payload: str) -> int:
    """The detached child: wait for the shell, then start the agent.

    Runs after the popup is gone, so there is nobody to report to but the log.
    Both things that realistically go wrong here -- the shell still starting up,
    and a name another agent took first -- are worth retrying rather than
    leaving the user with a tab that quietly has no agent in it.
    """
    try:
        request = json.loads(payload)
        name = str(request["name"])
        kind = str(request["kind"])
        pane = str(request["pane"])
        raw = request.get("args") or []
        if not isinstance(raw, list):
            raise TypeError("args must be a list")
        extra = [str(arg) for arg in raw]
    except (KeyError, TypeError, ValueError) as error:
        return _fail("bad --start-agent payload: %s" % error)

    client = HerdrClient(timeout=90.0)
    deadline = time.monotonic() + SHELL_WAIT_SECONDS
    renames = 0
    last: Optional[HerdrError] = None

    while True:
        try:
            client.start_agent(name, kind, pane, args=extra)
            return 0
        except HerdrError as error:
            last = error
            message = str(error)
            if _transient(message) and time.monotonic() < deadline:
                time.sleep(SHELL_POLL_SECONDS)
                continue
            if "name" in message.lower() and renames < NAME_ATTEMPTS:
                renames += 1
                try:
                    taken = live_agent_names(client.snapshot())
                except HerdrError:
                    taken = set()
                taken.add(name)
                name = agent_name(name, kind, sorted(taken))
                continue
            break
    return _fail("could not start %s in %s: %s" % (kind, pane, last))


# -- entry -----------------------------------------------------------------


def main(argv: Optional[List[str]] = None) -> int:
    args = list(argv if argv is not None else sys.argv[1:])
    if START_AGENT in args:
        index = args.index(START_AGENT)
        if index + 1 >= len(args):
            return _fail("%s needs a payload" % START_AGENT)
        return _start_agent(args[index + 1])
    if "--help" in args or "-h" in args:
        sys.stdout.write(USAGE)
        return 0
    if "--version" in args:
        sys.stdout.write("herdr-newtab-plus %s\n" % __version__)
        return 0

    config = Config.load()
    client = HerdrClient()

    if "--doctor" in args:
        return _doctor(client, config)
    try:
        if "--list" in args:
            return _list(client, config)
    except HerdrError as error:
        return _fail(str(error))

    if not sys.stdin.isatty() or not sys.stdout.isatty():
        return _fail("needs a terminal; run it from Herdr with `plugin pane open`")

    recents = Recents(state_path())
    try:
        snapshot, catalog = _catalog(client, config, recents)
    except HerdrError as error:
        return _fail(str(error))

    selection_background = config.selection_background
    # One appearance query, and only when something still depends on it: it
    # costs a round trip to the terminal on every open.
    appearance = None
    if selection_background == "auto" or "muted" not in (config.colors or {}):
        appearance = detect_appearance()
    if selection_background == "auto":
        selection_background = default_selection_background(appearance)
    theme = Theme(config.colors, selection_background, appearance)

    picker = Picker(
        snapshot=snapshot,
        catalog=catalog,
        chips=agents.available(config.agents),
        config=config,
        recents=recents,
        theme=theme,
        base=focused_cwd(snapshot) or paths.home(),
    )

    with Terminal(mouse=config.mouse) as terminal:
        choice = picker.run(terminal)

    if choice is None:
        # Cancelling abandons the pick, not the preference: the create mode is a
        # setting, and a toggle that quietly reverted on escape would be one you
        # could never trust. A no-op when nothing was touched.
        recents.save()
        return 0

    try:
        pane_id = create(client, choice)
    except HerdrError as error:
        return _fail(str(error))

    if not choice.agent.is_none:
        name = agent_name(choice.folder, choice.agent.kind, sorted(live_agent_names(snapshot)))
        extra = (
            agents.skip_permission_flags(choice.agent.kind, config.skip_permission_flags)
            if choice.skip_permissions
            else []
        )
        spawn_agent(choice, pane_id, name, extra)

    recents.touch(choice.folder, choice.agent.kind)
    recents.save()
    return 0


__all__ = ["main", "create", "spawn_agent", "SPACE", "resolve_bin"]
