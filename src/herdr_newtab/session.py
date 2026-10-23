"""What the running session already looks like, and what a pick should do to it.

Herdr does not record a folder on a workspace: only panes carry a ``cwd``. So a
space's folders are exactly the folders its panes are sitting in, which is both
cheaper than guessing a single canonical root and more useful -- a space with a
worktree checked out alongside its main tree offers both.

That also gives the tab-versus-space decision an exact rule instead of a
heuristic: a folder already has a space when some pane in that space is standing
in it. Everything else is a new space.
"""

from __future__ import annotations

import os
import re
from typing import Any, Dict, List, NamedTuple, Optional, Sequence, Set, Tuple

TAB = "tab"
SPACE = "space"
SMART = "smart"

# What ``create`` can be, in the order the picker's own key cycles them: the
# automatic decision first, then each way of overriding it.
MODES = (SMART, TAB, SPACE)


def next_mode(mode: str) -> str:
    """The mode after this one. An unrecognised mode behaves as ``SMART`` does."""
    index = MODES.index(mode) if mode in MODES else 0
    return MODES[(index + 1) % len(MODES)]


class Space(NamedTuple):
    workspace_id: str
    label: str
    number: int
    focused: bool


class Target(NamedTuple):
    """What creating a tab for the picked folder will actually do."""

    kind: str  # TAB | SPACE
    folder: str
    workspace_id: Optional[str] = None  # the space a TAB lands in
    workspace_label: Optional[str] = None


def spaces(snapshot: Dict[str, Any]) -> List[Space]:
    out: List[Space] = []
    for raw in snapshot.get("workspaces") or []:
        if not isinstance(raw, dict):
            continue
        workspace_id = raw.get("workspace_id")
        if not isinstance(workspace_id, str):
            continue
        number = raw.get("number")
        out.append(
            Space(
                workspace_id=workspace_id,
                label=str(raw.get("label") or workspace_id),
                number=number if isinstance(number, int) else 0,
                focused=bool(raw.get("focused")),
            )
        )
    return out


def focused_workspace_id(snapshot: Dict[str, Any]) -> Optional[str]:
    value = snapshot.get("focused_workspace_id")
    if isinstance(value, str) and value:
        return value
    for space in spaces(snapshot):
        if space.focused:
            return space.workspace_id
    known = spaces(snapshot)
    return known[0].workspace_id if known else None


def focused_cwd(snapshot: Dict[str, Any]) -> Optional[str]:
    """Where the focused pane is standing.

    This is the folder a relative query is relative to. The picker's own process
    runs with the plugin directory as its cwd, which would make ``./`` mean
    something nobody typed it to mean.
    """
    focused = snapshot.get("focused_pane_id")
    fallback: Optional[str] = None
    for raw in snapshot.get("panes") or []:
        if not isinstance(raw, dict):
            continue
        cwd = raw.get("cwd")
        if not isinstance(cwd, str) or not cwd:
            continue
        if raw.get("pane_id") == focused or raw.get("focused"):
            return normalize(cwd)
        if fallback is None:
            fallback = normalize(cwd)
    return fallback


def space_folders(snapshot: Dict[str, Any]) -> Dict[str, Space]:
    """Every folder a pane is standing in, mapped to the space that owns it.

    A folder open in more than one space resolves to the focused one, so the
    picker sends you where you were rather than to whichever space sorted first.
    """
    by_id = {space.workspace_id: space for space in spaces(snapshot)}
    found: Dict[str, Space] = {}
    for raw in snapshot.get("panes") or []:
        if not isinstance(raw, dict):
            continue
        cwd = raw.get("cwd")
        workspace_id = raw.get("workspace_id")
        if not isinstance(cwd, str) or not cwd:
            continue
        space = by_id.get(workspace_id) if isinstance(workspace_id, str) else None
        if space is None:
            continue
        # Indexed under both spellings so a lookup finds the space whichever
        # side of a symlink the folder was picked from.
        for key in {normalize(cwd), resolved(cwd)}:
            if not key:
                continue
            current = found.get(key)
            if current is None or (space.focused and not current.focused):
                found[key] = space
    return found


# Which status a folder shows when several agents stand in it: the one that
# wants you first, in herdr-bar's order.
STATUS_ORDER = ("blocked", "done", "working", "idle")


class FolderAgent(NamedTuple):
    """An agent standing in a folder, as the preview lists it."""

    kind: str  # "claude", "codex", ... or "" when Herdr did not say
    name: str  # the agent's own name when it has one, otherwise its kind
    status: str  # one of STATUS_ORDER, or "" when Herdr reports none


def _agent_records(snapshot: Dict[str, Any]) -> List[Tuple[Dict[str, Any], str, str]]:
    """Every agent the snapshot knows of, with the folder it is in and its status.

    Agents carry a pane rather than a folder, so the folder is the agent's own
    ``cwd`` when the snapshot has one and its pane's otherwise. A pane that
    reports an agent status but has no agent record of its own is counted too,
    once. A status Herdr does not report, or reports as ``none``, is ``""``.
    """
    panes: Dict[str, Dict[str, Any]] = {}
    for raw in snapshot.get("panes") or []:
        if isinstance(raw, dict) and isinstance(raw.get("pane_id"), str):
            panes[raw["pane_id"]] = raw

    agents = [raw for raw in snapshot.get("agents") or [] if isinstance(raw, dict)]
    claimed = {raw.get("pane_id") for raw in agents}
    records = agents + [
        pane
        for pane_id, pane in panes.items()
        if pane_id not in claimed and (pane.get("agent_status") or pane.get("agent"))
    ]

    out: List[Tuple[Dict[str, Any], str, str]] = []
    for raw in records:
        status = raw.get("agent_status") or raw.get("status")
        pane = panes.get(raw.get("pane_id")) or {}
        cwd = next(
            (
                value
                for value in (
                    raw.get("foreground_cwd"),
                    raw.get("cwd"),
                    pane.get("foreground_cwd"),
                    pane.get("cwd"),
                )
                if isinstance(value, str) and value
            ),
            None,
        )
        if cwd is not None:
            out.append((raw, cwd, status if status in STATUS_ORDER else ""))
    return out


def _folder_keys(cwd: str) -> Set[str]:
    # Indexed under both spellings, as space_folders is, so a folder picked from
    # either side of a symlink finds what is standing in it.
    return {key for key in (normalize(cwd), resolved(cwd)) if key}


def folder_statuses(snapshot: Dict[str, Any]) -> Dict[str, str]:
    """Every folder an agent is standing in, mapped to its most urgent status."""
    rank = {status: index for index, status in enumerate(STATUS_ORDER)}
    found: Dict[str, str] = {}
    for _, cwd, status in _agent_records(snapshot):
        if not status:
            continue
        for key in _folder_keys(cwd):
            current = found.get(key)
            if current is None or rank[status] < rank[current]:
                found[key] = status
    return found


def folder_agents(snapshot: Dict[str, Any]) -> Dict[str, List[FolderAgent]]:
    """Every folder an agent is standing in, mapped to all of them, most urgent first."""
    rank = {status: index for index, status in enumerate(STATUS_ORDER)}
    found: Dict[str, List[FolderAgent]] = {}
    for raw, cwd, status in _agent_records(snapshot):
        kind = raw.get("agent")
        kind = kind if isinstance(kind, str) else ""
        name = raw.get("name")
        agent = FolderAgent(kind, name if isinstance(name, str) and name else kind, status)
        for key in _folder_keys(cwd):
            found.setdefault(key, []).append(agent)
    for agents in found.values():
        agents.sort(key=lambda agent: rank.get(agent.status, len(rank)))
    return found


def live_agent_names(snapshot: Dict[str, Any]) -> Set[str]:
    """Names already taken by live agents, so a new one can avoid them."""
    names: Set[str] = set()
    for raw in snapshot.get("agents") or []:
        if not isinstance(raw, dict):
            continue
        name = raw.get("name")
        if isinstance(name, str) and name:
            names.add(name)
    return names


def normalize(path: str) -> str:
    """A comparable form of a path: expanded, absolute, no trailing slash.

    Symlinks are left alone, because this is also the path handed to Herdr when
    a space is created, and the folder the user picked is the one they should
    see. Matching an existing space additionally compares ``resolved`` forms.
    """
    expanded = os.path.expanduser(os.path.expandvars(path.strip()))
    if not expanded:
        return ""
    absolute = os.path.abspath(expanded)
    return absolute.rstrip(os.sep) or os.sep


# Following symlinks costs one lstat per path component, and the paths asked
# about are the same on every frame: the panes in the snapshot, which cannot
# change while the picker is up, and the folder under the cursor, which comes
# back around as soon as the selection does. So each answer is worked out once.
# Same bargain as the directory listings in ``paths``: the picker is modal and
# short-lived, and the rescan key clears both.
_RESOLVED: Dict[str, str] = {}


def resolved(path: str) -> str:
    """The same folder with every symlink followed.

    A shell resolves its own cwd, so a pane started in ``/tmp/x`` reports
    ``/private/tmp/x`` on macOS. Without this, picking that folder a second time
    would not recognise the space it already has and would open a duplicate.
    """
    folder = normalize(path)
    if not folder:
        return ""
    found = _RESOLVED.get(folder)
    if found is not None:
        return found
    try:
        found = os.path.realpath(folder).rstrip(os.sep) or os.sep
    except OSError:
        found = folder
    _RESOLVED[folder] = found
    return found


def clear_resolved() -> None:
    """Forget every followed symlink, so the next question is asked afresh."""
    _RESOLVED.clear()


def resolve_target(
    folder: str,
    snapshot: Dict[str, Any],
    mode: str = "smart",
) -> Target:
    """Decide whether the picked folder becomes a tab or a whole new space."""
    normalized = normalize(folder)
    label = os.path.basename(normalized) or normalized

    if mode == SPACE:
        # Labelled like any other new space: the line above the hints names the
        # space Enter is about to make, whether it was inferred or insisted on.
        return Target(SPACE, normalized, workspace_label=label)

    known = space_folders(snapshot)
    match = known.get(normalized) or known.get(resolved(folder))

    if mode == TAB:
        # Always a tab: in the space that already owns the folder when there is
        # one, otherwise in whichever space is focused.
        workspace_id = match.workspace_id if match else focused_workspace_id(snapshot)
        if workspace_id is None:
            return Target(SPACE, normalized)  # nothing to put a tab in yet
        return Target(
            TAB,
            normalized,
            workspace_id,
            match.label if match else _label_of(snapshot, workspace_id),
        )

    if match is not None:
        return Target(TAB, normalized, match.workspace_id, match.label)
    return Target(SPACE, normalized, workspace_label=label)


def _label_of(snapshot: Dict[str, Any], workspace_id: str) -> Optional[str]:
    for space in spaces(snapshot):
        if space.workspace_id == workspace_id:
            return space.label
    return None


# Herdr requires `[a-z][a-z0-9_-]{0,31}` for an agent name.
_NAME_STRIP = re.compile(r"[^a-z0-9_-]+")


def agent_name(folder: str, kind: str, taken: Sequence[str] = ()) -> str:
    """A legal, unused agent name derived from the folder being opened."""
    base = _NAME_STRIP.sub("-", os.path.basename(normalize(folder)).lower()).strip("-_")
    if not base or not base[0].isalpha():
        base = "%s-%s" % (kind, base) if base else kind
    base = _NAME_STRIP.sub("-", base).strip("-_")[:32] or "agent"
    if not base[0].isalpha():
        base = "a" + base[:31]

    used = set(taken)
    if base not in used:
        return base
    for suffix in range(2, 100):
        tail = "-%d" % suffix
        candidate = base[: 32 - len(tail)] + tail
        if candidate not in used:
            return candidate
    return base[:32]


def created_ids(result: Dict[str, Any]) -> Tuple[Optional[str], Optional[str]]:
    """Pull (tab_id, root pane id) out of a tab.create or workspace.create reply."""
    tab = result.get("tab")
    tab_id = tab.get("tab_id") if isinstance(tab, dict) else None
    pane = result.get("root_pane")
    pane_id = pane.get("pane_id") if isinstance(pane, dict) else None
    return (
        tab_id if isinstance(tab_id, str) else None,
        pane_id if isinstance(pane_id, str) else None,
    )
