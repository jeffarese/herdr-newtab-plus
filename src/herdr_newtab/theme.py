"""Colors.

Defaults are plain ANSI colors, so the picker inherits whatever theme the
terminal (and therefore Herdr) already uses. Anything can be overridden with a
hex value from the plugin config.

Three tiers of foreground, so a row reads in order: the folder name (text), the
path and badge you scan once the name has landed (muted), and the rules and
separators that only exist to divide them (dim).
"""

from __future__ import annotations

from typing import Dict, Optional

from .icons import agent_kind, logo_for

RESET = "\x1b[0m"
BOLD = "\x1b[1m"
UNDERLINE = "\x1b[4m"

_NAMED = {
    "black": 0,
    "red": 1,
    "green": 2,
    "yellow": 3,
    "blue": 4,
    "magenta": 5,
    "cyan": 6,
    "white": 7,
    "bright_black": 8,
    "gray": 8,
    "grey": 8,
    "bright_red": 9,
    "bright_green": 10,
    "bright_yellow": 11,
    "bright_blue": 12,
    "bright_magenta": 13,
    "bright_cyan": 14,
    "bright_white": 15,
}

DEFAULTS = {
    "accent": "bright_blue",
    "text": "default",
    "muted": "244",
    # Grays rather than `bright_black`, which is the one ANSI slot themes treat
    # as decoration: Tokyo Night and its relatives set it to a near-background
    # navy, and everything drawn in it -- hint labels, the counter, the line
    # spelling the create mode out -- came out unreadable. A gray from the cube
    # is the same gray whatever the terminal's palette says.
    "dim": "245",
    "match": "bright_blue",
    # The inline completion sitting after the cursor: present enough to read,
    # faint enough that it never looks like something you typed.
    "ghost": "244",
    "agent": "bright_magenta",
    "space": "bright_cyan",
    "error": "bright_red",
    # An agent about to start with its permission prompts off. Deliberately not
    # the agent color: the point is that the chip stops looking ordinary.
    "warn": "bright_yellow",
    # The rules between the input, the list and the chrome. They only divide,
    # so they sit below every tier of text rather than in the dimmest of them.
    "rule": "240",
    # Agent status, in herdr's own sidebar colours, for folders an agent is
    # already standing in.
    "blocked": "bright_red",
    "working": "#c78a1f",
    "done": "bright_cyan",
    "idle": "bright_green",
    # Each agent in its vendor's colour, the same table herdr-bar uses, so an
    # agent looks the same in both popups. Pi and Grok have monochrome marks and
    # take the text colour. An agent missing here falls back to `agent`.
    "agent_claude": "#D97757",
    "agent_codex": "#10A37F",
    "agent_kimi": "#9B8AFB",
    "agent_gemini": "#4285F4",
    "agent_cursor": "#A78BFA",
    "agent_opencode": "#38BDF8",
    "agent_pi": "default",
    "agent_grok": "default",
}

STATUS_GLYPHS = {
    "blocked": "◉",
    "working": "◐",
    "done": "●",
    "idle": "✓",
}


def _parse(value: Optional[str]) -> Optional[str]:
    """Return the SGR parameters for a color spec, or None for 'default'."""
    if value is None:
        return None
    spec = str(value).strip().lower()
    if not spec or spec in ("default", "none", "reset", "inherit"):
        return None
    if spec.startswith("#") and len(spec) == 7:
        try:
            red = int(spec[1:3], 16)
            green = int(spec[3:5], 16)
            blue = int(spec[5:7], 16)
        except ValueError:
            return None
        return "2;%d;%d;%d" % (red, green, blue)
    if spec in _NAMED:
        return "5;%d" % _NAMED[spec]
    if spec.isdigit() and 0 <= int(spec) <= 255:
        return "5;%d" % int(spec)
    return None


class Theme(object):
    def __init__(
        self,
        overrides: Optional[Dict[str, str]] = None,
        selection_background: Optional[str] = None,
        appearance: Optional[str] = None,
    ) -> None:
        merged = dict(DEFAULTS)
        for role, spec in (
            ("muted", default_muted(appearance)),
            ("dim", default_dim(appearance)),
            ("ghost", default_ghost(appearance)),
            ("rule", default_rule(appearance)),
        ):
            if spec:
                merged[role] = spec
        merged.update({k: v for k, v in (overrides or {}).items() if isinstance(v, str)})
        self._fg: Dict[str, str] = {}
        for role, spec in merged.items():
            params = _parse(spec)
            self._fg[role] = "" if params is None else "\x1b[38;%sm" % params
        selection = _parse(selection_background)
        self.selection_bg = "" if selection is None else "\x1b[48;%sm" % selection
        # Vendor logos are a font question, not a colour one: the caller decides
        # whether the font is there and says so here, once.
        self.agent_icons = False

    def fg(self, role: str) -> str:
        return self._fg.get(role, "")

    def agent_role(self, kind: Optional[str]) -> str:
        """The role an agent is drawn in: its vendor's colour, or `agent`."""
        role = "agent_" + agent_kind(kind)
        return role if role in self._fg else "agent"

    def logo(self, kind: Optional[str]) -> str:
        """The agent's logo when logos are on and the font has one, else empty."""
        if not self.agent_icons:
            return ""
        return logo_for(kind) or ""

    def paint(self, role: str, text: str, bold: bool = False) -> str:
        color = self.fg(role)
        if not color and not bold:
            return text
        return "%s%s%s%s" % (BOLD if bold else "", color, text, RESET)


def default_muted(appearance: Optional[str]) -> Optional[str]:
    """Pick the secondary foreground: legible, still clearly below the title.

    The neutral 244 survives either background, so an unknown appearance keeps
    the default rather than guessing.
    """
    if appearance == "dark":
        return "249"
    if appearance == "light":
        return "240"
    return None


def default_dim(appearance: Optional[str]) -> Optional[str]:
    """The third tier: below the secondary text, still a thing you can read.

    Separators and labels live here, and a label nobody can read is a label that
    is not there -- so it moves with the background rather than sitting at one
    gray that only suits half of them.
    """
    if appearance == "dark":
        return "246"
    if appearance == "light":
        return "243"
    return None


def default_ghost(appearance: Optional[str]) -> Optional[str]:
    """The inline completion: one step below `dim`, on whichever side is fainter.

    It sits in the middle of the input rather than beside it, so it is the one
    place where being read as typed text is worse than being hard to read.
    """
    if appearance == "dark":
        return "244"
    if appearance == "light":
        return "245"
    return None


def default_rule(appearance: Optional[str]) -> Optional[str]:
    """The rules: visible as a line, quiet enough to never be read as text."""
    if appearance == "dark":
        return "239"
    if appearance == "light":
        return "250"
    return None


def default_selection_background(appearance: Optional[str]) -> Optional[str]:
    """Pick a selection background that suits the terminal's own background."""
    if appearance == "dark":
        return "237"
    if appearance == "light":
        return "254"
    return None
