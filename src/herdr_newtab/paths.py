"""Where the folder suggestions come from, and how a query narrows them.

Two modes, chosen by what the query looks like:

* **path mode** -- the query names a path (it starts with ``/``, ``~`` or
  ``.``, or contains a separator). Suggestions are the real subdirectories of
  the parent it names, so completion is always against the filesystem as it
  actually is rather than against a remembered list.
* **catalog mode** -- the query is a bare word, or empty. Suggestions come from
  a catalog assembled when the picker opens: folders that already have a space,
  folders picked recently, the children of the configured roots, and zoxide's
  frecency list when it is installed.

Both produce the same row type, so the list does not change shape as you type
your way from one into the other.
"""

from __future__ import annotations

import os
import subprocess
from typing import Any, Dict, List, NamedTuple, Optional, Sequence, Tuple

from .fuzzy import match, split_query
from .session import Space, clear_resolved, normalize, space_folders

# Rank breaks ties between sources when two folders score the same. Lower wins.
SOURCE_SPACE = "space"
SOURCE_RECENT = "recent"
SOURCE_ROOT = "root"
SOURCE_ZOXIDE = "zoxide"
SOURCE_PATH = "path"

_RANK = {
    SOURCE_PATH: 0,
    SOURCE_RECENT: 1,
    SOURCE_SPACE: 2,
    SOURCE_ZOXIDE: 3,
    SOURCE_ROOT: 4,
}

# A folder's own name is what you are almost always aiming at, so a hit there
# outweighs the same hit buried in the middle of its path.
NAME_BONUS = 24


class Suggestion(NamedTuple):
    path: str  # absolute, normalized
    name: str  # the folder's own name, which is what the row leads with
    source: str
    detail: str = ""  # the space's label, for folders that already have one
    is_git: bool = False


class Scored(NamedTuple):
    suggestion: Suggestion
    score: int
    name_positions: Tuple[int, ...]
    path_positions: Tuple[int, ...]


# Every row's path is written for display against home, so a keystroke asks for
# it once per folder in the catalog -- hundreds of times to produce the same
# string. A process cannot move house while it runs.
_HOME = ""


def home() -> str:
    global _HOME
    if not _HOME:
        _HOME = os.path.expanduser("~")
    return _HOME


def display(path: str) -> str:
    """The path as a person would write it: ``~`` for home."""
    root = home()
    if path == root:
        return "~"
    if path.startswith(root + os.sep):
        return "~" + path[len(root) :]
    return path


def folder_name(path: str) -> str:
    """What a row leads with. Home is written as ``~``, not as your username."""
    if path == home():
        return "~"
    return os.path.basename(path) or path


def parent_display(path: str) -> str:
    """The part of a row's path that is not its name -- shown after it, dimmed."""
    return display(os.path.dirname(path))


# -- filesystem ------------------------------------------------------------

# Directory listings are re-read on every keystroke as the leaf changes but the
# parent does not, so each directory is read once per picker session. The
# picker is short-lived and modal, so a folder created while it is open is a
# non-case; a stale entry costs one failed create with a clear message.
_LISTINGS: Dict[Tuple[str, bool], Tuple[Tuple[str, bool], ...]] = {}


def subdirectories(directory: str, hidden: bool = False) -> Tuple[Tuple[str, bool], ...]:
    """The directory's subdirectories as (name, is_git) pairs, sorted by name."""
    key = (directory, hidden)
    found = _LISTINGS.get(key)
    if found is not None:
        return found

    entries: List[Tuple[str, bool]] = []
    try:
        with os.scandir(directory) as scan:
            for entry in scan:
                name = entry.name
                if not hidden and name.startswith("."):
                    continue
                try:
                    if not entry.is_dir():
                        continue
                except OSError:
                    continue
                entries.append((name, os.path.isdir(os.path.join(directory, name, ".git"))))
    except OSError:
        entries = []
    entries.sort(key=lambda pair: pair[0].lower())
    result = tuple(entries)
    _LISTINGS[key] = result
    return result


def clear_cache() -> None:
    """Forget everything read off the filesystem. What the rescan key does."""
    _LISTINGS.clear()
    clear_resolved()


def is_directory(path: str) -> bool:
    try:
        return os.path.isdir(path)
    except OSError:
        return False


# -- query shape -----------------------------------------------------------


def looks_like_path(query: str) -> bool:
    text = query.strip()
    if not text:
        return False
    return text[0] in ("/", "~", ".") or os.sep in text or "$" in text


def split_path_query(query: str, base: Optional[str] = None) -> Tuple[str, str, bool]:
    """Split a path query into (directory to list, leaf to match, list itself).

    The third value says whether the directory should also be offered as a row
    of its own: true when the query names it outright, which is what makes
    ``~/workspace/tri/`` mean "this folder, and here is what is inside it".
    """
    text = query.strip()
    expanded = os.path.expanduser(os.path.expandvars(text))
    root = base or os.getcwd()

    if text in ("~", "~/") or expanded.endswith(os.sep) or expanded in ("", "."):
        directory = expanded.rstrip(os.sep) or (os.sep if expanded.startswith(os.sep) else root)
        if expanded in ("", "."):
            directory = root
        return _absolute(directory, root), "", True

    directory = os.path.dirname(expanded)
    leaf = os.path.basename(expanded)
    if not directory:
        directory = root
    return _absolute(directory, root), leaf, False


def _absolute(path: str, base: str) -> str:
    if os.path.isabs(path):
        return normalize(path)
    return normalize(os.path.join(base, path))


# -- catalog ---------------------------------------------------------------


def zoxide_folders(limit: int = 400) -> List[str]:
    """Frecent directories from zoxide, best first. Empty when it is not installed."""
    try:
        proc = subprocess.run(
            ["zoxide", "query", "--list", "--score"],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=1.5,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return []
    if proc.returncode != 0:
        return []
    scored: List[Tuple[float, str]] = []
    for line in proc.stdout.decode("utf-8", "replace").splitlines():
        parts = line.strip().split(None, 1)
        if len(parts) != 2:
            continue
        try:
            score = float(parts[0])
        except ValueError:
            continue
        scored.append((score, parts[1]))
    scored.sort(key=lambda pair: pair[0], reverse=True)
    return [path for _, path in scored[:limit]]


def zoxide_installed() -> bool:
    from shutil import which

    return which("zoxide") is not None


def root_folders(roots: Sequence[str], hidden: bool = False) -> List[Tuple[str, bool]]:
    """Every immediate child of every configured root that exists."""
    out: List[Tuple[str, bool]] = []
    seen = set()
    for raw in roots:
        directory = normalize(raw)
        if not directory or directory in seen or not is_directory(directory):
            continue
        seen.add(directory)
        for name, is_git in subdirectories(directory, hidden):
            out.append((os.path.join(directory, name), is_git))
    return out


def build_catalog(
    snapshot: Dict[str, Any],
    roots: Sequence[str],
    recents: Sequence[str] = (),
    hidden: bool = False,
    use_zoxide: bool = True,
) -> List[Suggestion]:
    """Assemble the resting list, best source first, one row per folder.

    Order here is the order shown before anything is typed: what you picked
    last, then what is already open, then the frecent and scanned folders. Once
    a query exists the score leads and this is only a tie-break.
    """
    owned: Dict[str, Space] = space_folders(snapshot)
    out: List[Suggestion] = []
    seen: Dict[str, int] = {}

    def add(path: str, source: str, detail: str = "", is_git: bool = False) -> None:
        folder = normalize(path)
        if not folder:
            return
        space = owned.get(folder)
        if space is not None:
            # A folder that already has a space is always shown as one, whichever
            # list it also happens to appear in.
            source, detail = SOURCE_SPACE, space.label
        index = seen.get(folder)
        if index is not None:
            existing = out[index]
            if _RANK[source] < _RANK[existing.source]:
                out[index] = existing._replace(source=source, detail=detail or existing.detail)
            elif is_git and not existing.is_git:
                out[index] = existing._replace(is_git=True)
            return
        seen[folder] = len(out)
        out.append(
            Suggestion(
                path=folder,
                name=folder_name(folder),
                source=source,
                detail=detail,
                is_git=is_git or is_directory(os.path.join(folder, ".git")),
            )
        )

    for path in recents:
        if is_directory(normalize(path)):
            add(path, SOURCE_RECENT)
    for folder, space in owned.items():
        add(folder, SOURCE_SPACE, space.label)
    if use_zoxide:
        for path in zoxide_folders():
            if is_directory(normalize(path)):
                add(path, SOURCE_ZOXIDE)
    for path, is_git in root_folders(roots, hidden):
        add(path, SOURCE_ROOT, is_git=is_git)
    return out


# -- matching --------------------------------------------------------------


def _score(suggestion: Suggestion, terms: Sequence[str]) -> Optional[Scored]:
    """Score one folder against every AND-ed term, over its name and its path.

    A term may land on either field; the row is only a match when every term
    lands somewhere. Highlights are kept per field so the list can show exactly
    which characters were matched where.
    """
    if not terms:
        return Scored(suggestion, 0, (), ())

    shown = display(suggestion.path)
    total = 0
    name_hits: List[int] = []
    path_hits: List[int] = []
    for term in terms:
        on_name = match(term, suggestion.name)
        on_path = match(term, shown)
        name_score = on_name.score + NAME_BONUS if on_name else None
        path_score = on_path.score if on_path else None
        if name_score is None and path_score is None:
            return None
        if path_score is None or (name_score is not None and name_score >= path_score):
            total += name_score  # type: ignore[arg-type]
            name_hits.extend(on_name.positions)  # type: ignore[union-attr]
        else:
            total += path_score
            path_hits.extend(on_path.positions)  # type: ignore[union-attr]
    return Scored(suggestion, total, tuple(sorted(set(name_hits))), tuple(sorted(set(path_hits))))


def filter_catalog(catalog: Sequence[Suggestion], query: str, limit: int) -> List[Scored]:
    terms = split_query(query)
    if not terms:
        return [Scored(item, 0, (), ()) for item in catalog[:limit]]
    scored: List[Scored] = []
    for item in catalog:
        found = _score(item, terms)
        if found is not None:
            scored.append(found)
    scored.sort(key=lambda row: (-row.score, _RANK[row.suggestion.source], row.suggestion.path))
    return scored[:limit]


def complete_path(query: str, base: str, hidden: bool, limit: int) -> List[Scored]:
    """Real subdirectories matching a path query, shell-order first.

    Prefix matches lead, because that is what typing a path means; a fuzzy hit
    further in still shows up behind them rather than not at all.
    """
    directory, leaf, list_itself = split_path_query(query, base)
    rows: List[Scored] = []

    if list_itself and is_directory(directory):
        rows.append(
            Scored(
                Suggestion(
                    path=directory,
                    name=folder_name(directory),
                    source=SOURCE_PATH,
                    is_git=is_directory(os.path.join(directory, ".git")),
                ),
                0,
                (),
                (),
            )
        )

    folded = leaf.lower()
    prefixed: List[Scored] = []
    fuzzy: List[Scored] = []
    for name, is_git in subdirectories(directory, hidden):
        suggestion = Suggestion(
            path=os.path.join(directory, name),
            name=name,
            source=SOURCE_PATH,
            is_git=is_git,
        )
        if not leaf:
            prefixed.append(Scored(suggestion, 0, (), ()))
            continue
        if name.lower().startswith(folded):
            prefixed.append(Scored(suggestion, len(leaf), tuple(range(len(leaf))), ()))
            continue
        found = match(leaf, name)
        if found is not None:
            fuzzy.append(Scored(suggestion, found.score, found.positions, ()))
    fuzzy.sort(key=lambda row: (-row.score, row.suggestion.name.lower()))

    rows.extend(prefixed)
    rows.extend(fuzzy)
    return rows[:limit]


def suggest(
    query: str,
    catalog: Sequence[Suggestion],
    base: str,
    hidden: bool = False,
    limit: int = 200,
) -> List[Scored]:
    if looks_like_path(query):
        return complete_path(query, base, hidden, limit)
    return filter_catalog(catalog, query, limit)


# -- inline completion -----------------------------------------------------


def completion_for(query: str, suggestion: Optional[Suggestion]) -> str:
    """The tail to show after the cursor, or "" when there is nothing to add.

    Only ever a literal extension of what is already typed, in the same form it
    was typed in -- a tilde query completes to a tilde path. Anything that is
    not a straight extension is left to the accept key, which replaces the whole
    input instead of pretending to grow it.
    """
    if suggestion is None or not query:
        return ""
    target = accept_text(query, suggestion)
    if len(target) <= len(query):
        return ""
    if target[: len(query)].lower() != query.lower():
        return ""
    return target[len(query) :]


def accept_text(query: str, suggestion: Suggestion) -> str:
    """What the input becomes when a row is accepted.

    Written in the same form as the query -- tilde in, tilde out -- and always
    ending in a separator, so accepting a folder immediately lists what is
    inside it and another accept walks one level deeper.
    """
    text = display(suggestion.path) if _prefers_tilde(query) else suggestion.path
    return text.rstrip(os.sep) + os.sep


def _prefers_tilde(query: str) -> bool:
    stripped = query.strip()
    if stripped.startswith("~"):
        return True
    if stripped.startswith("/"):
        return False
    return True


def resolve_input(query: str, base: str) -> str:
    """The folder a typed input names, whether or not it exists."""
    text = query.strip()
    if not text:
        return ""
    expanded = os.path.expanduser(os.path.expandvars(text))
    return _absolute(expanded, base)

