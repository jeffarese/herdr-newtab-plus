"""Vendor logos from the Herdr Agent Icons Max font, the ones herdr-bar uses.

The font is herdr-radar's (MIT; the marks belong to their owners). It puts one
logo per agent in a private-use block starting at U+E1A0, in the order of
``VENDORS`` -- the same table herdr-bar and herdr-radar read, so a terminal set
up for either already draws these.

Nothing here installs a font or touches the terminal's configuration. ``auto``
looks for the font file where the platform keeps per-user and system fonts, and
falls back to plain labels when it is not there; ``font`` forces the logos, for
a picker reached over ssh from a terminal that has the font locally.

Codepoints: https://github.com/hhdebb/herdr-radar/blob/main/tools/codepoints.toml
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
from typing import List, Optional

FAMILY = "Herdr Agent Icons Max"
FILE_PREFIX = "HerdrAgentIconsMax"
FIRST = 0xE1A0

VENDORS = (
    "claude",
    "codex",
    "opencode",
    "omp",
    "cline",
    "mastracode",
    "kimi",
    "kilo",
    "maki",
    "pi",
    "hermes",
    "cursor",
    "copilot",
    "deepseek",
    "gemini",
    "gpt",
    "qwen",
    "grok",
    "agy",
    "kiro",
    "amp",
    "devin",
    "qodercli",
    "glm",
)

# What Herdr or a config might call an agent, against what the font calls it.
_ALIASES = {
    "cursor-agent": "cursor",
    "open-code": "opencode",
    "qoder": "qodercli",
}

# A logo is drawn one cell wide, but terminals let an icon glyph spill into the
# blank cell after it -- which is what makes it legible at text size. Two spaces
# after a logo keep the spill off whatever follows.
GAP = "  "


def agent_kind(agent: Optional[str]) -> str:
    """The agent as the logo and colour tables know it: ``Claude Code`` is ``claude``."""
    kind = (agent or "").strip().lower().split(":", 1)[0]
    if kind.endswith(" code"):
        kind = kind[: -len(" code")]
    return _ALIASES.get(kind, kind)


def logo_for(agent: Optional[str]) -> Optional[str]:
    kind = agent_kind(agent)
    if kind in VENDORS:
        return chr(FIRST + VENDORS.index(kind))
    return None


def is_logo(char: str) -> bool:
    return len(char) == 1 and FIRST <= ord(char) < FIRST + len(VENDORS)


def _font_dirs() -> List[str]:
    home = os.path.expanduser("~")
    if sys.platform == "darwin":
        return [os.path.join(home, "Library", "Fonts"), "/Library/Fonts"]
    # Only the top of the per-user folders: anything deeper, or system-wide, is
    # fontconfig's to answer.
    data = os.environ.get("XDG_DATA_HOME") or os.path.join(home, ".local", "share")
    return [os.path.join(data, "fonts"), os.path.join(home, ".fonts")]


def _has_font_file(directory: str) -> bool:
    try:
        names = os.listdir(directory)
    except OSError:
        return False
    return any(name.startswith(FILE_PREFIX) and name.endswith(".ttf") for name in names)


def detect() -> bool:
    """Whether this machine has the font, by file first and fontconfig second."""
    if any(_has_font_file(directory) for directory in _font_dirs()):
        return True
    if sys.platform == "darwin":
        return False
    try:
        proc = subprocess.run(
            ["fc-match", "--format", "%{family}", FAMILY],
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=1.0,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return False
    families = [part.strip() for part in proc.stdout.decode("utf-8", "replace").split(",")]
    return proc.returncode == 0 and FAMILY in families


_FOUND: Optional[bool] = None
_PREFETCH: Optional[threading.Thread] = None
_PREFETCHED: List[bool] = []


def prefetch() -> None:
    """Start looking now, so the answer is ready by the time the frame needs it.

    ``fc-match`` is a few milliseconds on Linux and nothing it reads depends on
    the session, so it overlaps the snapshot request instead of following it.
    """
    global _PREFETCH
    if _FOUND is not None or _PREFETCH is not None:
        return
    _PREFETCH = threading.Thread(target=lambda: _PREFETCHED.append(detect()), daemon=True)
    _PREFETCH.start()


def font_available() -> bool:
    global _FOUND, _PREFETCH
    if _FOUND is None:
        if _PREFETCH is not None:
            _PREFETCH.join(2.0)
            _PREFETCH = None
        _FOUND = _PREFETCHED[0] if _PREFETCHED else detect()
    return _FOUND


def clear_cache() -> None:
    global _FOUND, _PREFETCH
    _FOUND = None
    _PREFETCH = None
    del _PREFETCHED[:]


def enabled(mode: object) -> bool:
    """``auto`` follows the font, ``font`` forces logos, anything else is text only."""
    if mode == "font" or mode is True:
        return True
    if mode == "auto":
        return font_available()
    return False
