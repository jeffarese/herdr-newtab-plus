"""Which agents this machine can actually start.

Herdr's ``agent start --kind`` accepts a fixed set of kinds and launches its own
canonical executable for each. There is no API that reports which of them are
installed, so the picker looks for the executable itself and offers only the
ones it finds -- a list of twenty kinds where four are runnable is a worse row
than a list of four.

Order is by how commonly the agent is reached for, not alphabetically, because
the row is scanned rather than searched.
"""

from __future__ import annotations

import os
from shutil import which
from typing import Dict, List, NamedTuple, Optional, Sequence

NONE = ""  # the always-present first chip: open a plain shell


class Agent(NamedTuple):
    kind: str  # what `agent start --kind` wants; "" for a plain shell
    label: str  # what the chip row shows

    @property
    def is_none(self) -> bool:
        return self.kind == NONE


# kind -> the executables that mean it is installed. Herdr picks the binary
# itself at start time; these are only used to decide whether to offer the chip.
KNOWN: Dict[str, Sequence[str]] = {
    "claude": ("claude",),
    "codex": ("codex",),
    "gemini": ("gemini",),
    "cursor": ("cursor-agent", "cursor"),
    "opencode": ("opencode",),
    "copilot": ("copilot",),
    "amp": ("amp",),
    "droid": ("droid",),
    "grok": ("grok",),
    "kimi": ("kimi",),
    "pi": ("pi",),
    "cline": ("cline",),
    "kilo": ("kilo",),
    "kiro": ("kiro",),
    "devin": ("devin",),
    "agy": ("agy",),
    "omp": ("omp",),
    "mastracode": ("mastracode",),
    "hermes": ("hermes",),
    "qodercli": ("qodercli", "qoder"),
    "maki": ("maki",),
}

# The order chips appear in. Anything in KNOWN but missing here follows, sorted.
PREFERRED = (
    "claude",
    "codex",
    "gemini",
    "cursor",
    "opencode",
    "copilot",
    "amp",
    "droid",
    "grok",
    "kimi",
    "pi",
)

NONE_LABEL = "shell"

# kind -> the arguments that start it with its permission prompts turned off.
#
# Every CLI spells this differently, and most do not spell it at all, so this is
# a table of what was checked rather than a pattern. An agent missing from it is
# not assumed to have an undiscovered spelling: the toggle refuses on that chip
# and says so, which is the only answer that cannot silently start an agent with
# the wrong flag. `skip_permission_flags` in config.json adds or overrides an
# entry when a CLI grows one, or when yours takes a different form.
SKIP_PERMISSION_FLAGS: Dict[str, Sequence[str]] = {
    "claude": ("--dangerously-skip-permissions",),
    "codex": ("--dangerously-bypass-approvals-and-sandbox",),
}

SKIP_MARK = "!"  # what the chip row appends to say the flag is on


def skip_permission_flags(
    kind: str,
    overrides: Optional[Dict[str, Sequence[str]]] = None,
) -> List[str]:
    """The arguments that disable permission prompts for one agent kind.

    Empty means this kind has no known way to ask for it, which callers must
    treat as a refusal rather than as "pass nothing and hope".
    """
    if not kind:
        return []
    if overrides and kind in overrides:
        return [str(flag) for flag in overrides[kind]]
    return list(SKIP_PERMISSION_FLAGS.get(kind, ()))


def can_skip_permissions(
    kind: str,
    overrides: Optional[Dict[str, Sequence[str]]] = None,
) -> bool:
    return bool(skip_permission_flags(kind, overrides))


def installed(kind: str) -> bool:
    for binary in KNOWN.get(kind, (kind,)):
        if which(binary):
            return True
    return False


def _ordered(kinds: Sequence[str]) -> List[str]:
    rank = {kind: index for index, kind in enumerate(PREFERRED)}
    return sorted(kinds, key=lambda kind: (rank.get(kind, len(PREFERRED)), kind))


def available(configured: object = "auto") -> List[Agent]:
    """The chip row: a plain shell first, then every agent that can be started.

    A configured list is taken as written -- names Herdr does not know are
    dropped, but an installed check is not applied, so an agent behind a shim or
    a shell function still gets a chip.
    """
    chips = [Agent(NONE, NONE_LABEL)]
    if isinstance(configured, (list, tuple)):
        kinds = [kind for kind in configured if kind in KNOWN]
    else:
        kinds = _ordered([kind for kind in KNOWN if installed(kind)])
    for kind in kinds:
        chips.append(Agent(kind, kind))
    return chips


def index_of(chips: Sequence[Agent], kind: Optional[str]) -> int:
    if not kind:
        return 0
    for index, chip in enumerate(chips):
        if chip.kind == kind:
            return index
    return 0


def initial_index(
    chips: Sequence[Agent],
    default_agent: str,
    remembered: Optional[str],
) -> int:
    """Which chip starts selected.

    ``last`` reuses whatever was started in that folder before, which is the
    setting that earns its keep: the same project almost always wants the same
    agent, and the first keystroke is then never about the agent row.
    """
    if default_agent == "last":
        return index_of(chips, remembered)
    if default_agent in ("none", "", "shell"):
        return 0
    return index_of(chips, default_agent)


def agent_env() -> Dict[str, str]:
    """Environment for a detached `agent start`.

    Herdr injects its own plugin variables into this process; passing them on to
    a background child would make that child look like a plugin invocation it is
    not. Everything else is inherited untouched.
    """
    env = dict(os.environ)
    for name in (
        "HERDR_PLUGIN_ID",
        "HERDR_PLUGIN_ROOT",
        "HERDR_PLUGIN_CONFIG_DIR",
        "HERDR_PLUGIN_STATE_DIR",
        "HERDR_PLUGIN_CONTEXT_JSON",
        "HERDR_PLUGIN_ACTION_ID",
        "HERDR_PLUGIN_ENTRYPOINT_ID",
        "HERDR_PLUGIN_EVENT",
        "HERDR_PLUGIN_EVENT_JSON",
        "HERDR_PANE_ID",
        "HERDR_TAB_ID",
        "HERDR_WORKSPACE_ID",
    ):
        env.pop(name, None)
    return env
