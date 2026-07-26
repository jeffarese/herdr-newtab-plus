"""Compose the picker's rows.

Everything is drawn as full rows into the popup's inner area: Herdr already
draws the popup border, so the picker stays borderless and full-bleed. Rows are
composed as (role, text) segments, measured in display columns, and only then
turned into escape sequences -- which keeps truncation honest for CJK and emoji,
and makes the layout testable by stripping the colors back out.

Composing a whole row every time is the simple thing to do, and cheap; getting
those rows onto the screen without resending the ones that did not move is
``paint``'s job, not this module's.
"""

from __future__ import annotations

from typing import Dict, List, NamedTuple, Optional, Sequence, Tuple

from .agents import Agent
from .paths import SOURCE_PATH, SOURCE_RECENT, SOURCE_SPACE, SOURCE_ZOXIDE, Scored, parent_display
from .session import SPACE, TAB, Target
from .textutil import display_width, pad, truncate, truncate_middle
from .theme import BOLD, RESET, UNDERLINE, Theme

PROMPT = "❯"
SELECTED_BAR = "▌"
SCROLL_TRACK = "│"
SCROLL_THUMB = "┃"
ARROW = "→"

PLACEHOLDER = "folder — type a name, or a path to complete it"

# The badge names where a row came from. A folder that already has a space says
# so with the space's own label, because that is the thing you recognise.
BADGE_WORDS = {
    SOURCE_RECENT: "recent",
    SOURCE_ZOXIDE: "frecent",
    SOURCE_PATH: "",
}

MIN_NAME = 12
MAX_NAME = 34


class Layout(NamedTuple):
    inner_width: int
    list_top: int
    list_height: int
    rules: bool
    chips: bool
    target: bool
    footer: bool


def compute_layout(width: int, height: int) -> Layout:
    """Give the list every row the chrome does not need.

    The chrome sheds from the bottom up as the popup shrinks: the key hints go
    first, then the line that says what Enter will do, then the agent row. The
    input and at least one suggestion always survive.
    """
    inner = max(1, width - 2)  # one column of padding on each side

    footer = height >= 9
    target = height >= 7
    chips = height >= 5
    rules = height >= 8

    list_top = 2 if rules else 1
    chrome = list_top
    chrome += 1 if rules else 0  # the rule below the list
    chrome += 1 if chips else 0
    chrome += 1 if target else 0
    chrome += 1 if footer else 0
    return Layout(inner, list_top, max(1, height - chrome), rules, chips, target, footer)


class Segment(NamedTuple):
    role: str
    text: str
    bold: bool = False
    underline: bool = False


def _text(theme: Theme, segments: Sequence[Segment], background: str = "") -> str:
    out: List[str] = []
    for segment in segments:
        if not segment.text:
            continue
        color = theme.fg(segment.role) if segment.role else ""
        if not color and not segment.bold and not segment.underline:
            out.append(segment.text)
            continue
        prefix = (BOLD if segment.bold else "") + (UNDERLINE if segment.underline else "") + color
        out.append(prefix + segment.text + RESET + background)
    return "".join(out)


def _width(segments: Sequence[Segment]) -> int:
    return sum(display_width(segment.text) for segment in segments)


def _highlight(
    text: str,
    positions: Sequence[int],
    base_role: str,
    bold: bool,
) -> List[Segment]:
    """Split text so matched characters can be lit differently from the rest."""
    marks = {p for p in positions if 0 <= p < len(text)}
    if not marks:
        return [Segment(base_role, text, bold)]
    segments: List[Segment] = []
    run: List[str] = []
    run_marked = False

    def flush() -> None:
        if not run:
            return
        # Matched characters are usually one or two letters adrift in a longer
        # name, so color alone reads as noise: underline them too and the
        # subsequence is legible at a glance.
        segments.append(
            Segment(
                "match" if run_marked else base_role, "".join(run), bold or run_marked, run_marked
            )
        )

    for index, char in enumerate(text):
        marked = index in marks
        if marked != run_marked and run:
            flush()
            run = []
        run_marked = marked
        run.append(char)
    flush()
    return segments


# -- input -----------------------------------------------------------------


def _window(text: str, cursor: int, width: int) -> Tuple[str, int]:
    """Slide a long path so the cursor stays on screen, keeping the tail visible."""
    if display_width(text) <= width:
        return text, cursor
    start = 0
    while display_width(text[start:cursor]) > width - 1 and start < cursor:
        start += 1
    return text[start:], cursor - start


def render_input(theme: Theme, width: int, query: str, cursor: int, ghost: str) -> str:
    """The prompt line: what is typed, the block cursor, and the inline completion.

    The completion is drawn after the cursor in the ghost role rather than
    inserted into the query, so it is always obvious which characters are yours
    and which are the picker's guess.
    """
    field = max(4, width - 2)
    if not query:
        hint = truncate(PLACEHOLDER, max(0, field - 2))
        return (
            _text(theme, [Segment("accent", PROMPT + " ", True)])
            + "\x1b[7m \x1b[27m"
            + _text(theme, [Segment("dim", hint)])
        )

    visible, position = _window(query, min(cursor, len(query)), field - display_width(ghost))
    position = max(0, min(position, len(visible)))
    before = visible[:position]
    under = visible[position] if position < len(visible) else (ghost[:1] if ghost else " ")
    after = visible[position + 1 :] if position < len(visible) else ""
    tail = ghost if position < len(visible) else ghost[1:]

    return (
        _text(theme, [Segment("accent", PROMPT + " ", True)])
        + _text(theme, [Segment("text", before)])
        + "\x1b[7m"
        + under
        + "\x1b[27m"
        + _text(theme, [Segment("text", after)])
        + _text(theme, [Segment("ghost", truncate(tail, max(0, field - display_width(visible))))])
    )


# -- suggestion rows -------------------------------------------------------


def _badge(row: Scored) -> str:
    suggestion = row.suggestion
    if suggestion.source == SOURCE_SPACE:
        return suggestion.detail or "space"
    word = BADGE_WORDS.get(suggestion.source, "")
    if word:
        return word
    return "git" if suggestion.is_git else ""


def name_column(rows: Sequence[Scored], width: int) -> int:
    """How wide the name column has to be for the rows currently on screen.

    Sized to what is actually visible rather than to a fixed share of the popup,
    so a list of short names does not leave a corridor of empty space before the
    paths, and scrolling into longer ones widens it instead of truncating them.
    """
    longest = max((display_width(row.suggestion.name) for row in rows), default=0)
    return max(MIN_NAME, min(MAX_NAME, longest, max(MIN_NAME, width // 2)))


def render_row(
    theme: Theme,
    row: Scored,
    width: int,
    selected: bool,
    name_room: Optional[int] = None,
) -> str:
    """One folder: its name, where it lives, and what already knows about it."""
    suggestion = row.suggestion
    background = theme.selection_bg if selected else ""

    marker = Segment("accent", SELECTED_BAR + " ", True) if selected else Segment("", "  ")
    badge = _badge(row)
    badge_role = "space" if suggestion.source == SOURCE_SPACE else "dim"
    badge_width = display_width(badge) + 2 if badge and width >= 44 else 0

    if name_room is None:
        name_room = max(MIN_NAME, min(MAX_NAME, (width - 2 - badge_width) // 2))
    name = truncate(suggestion.name, name_room)
    segments: List[Segment] = [marker]
    segments.extend(_highlight(name, row.name_positions, "text", selected))
    segments.append(Segment("", " " * max(1, name_room - display_width(name) + 1)))

    parent_room = max(0, width - _width(segments) - badge_width)
    if parent_room >= 4:
        parent = truncate_middle(parent_display(suggestion.path), parent_room)
        segments.extend(_highlight(parent, row.path_positions, "muted", False))

    if badge_width:
        used = _width(segments)
        segments.append(Segment("", " " * max(1, width - used - display_width(badge))))
        segments.append(Segment(badge_role, badge))

    rendered = _text(theme, segments, background)
    if not background:
        return rendered
    # A selected row is a band across the list, so it has to be filled to the
    # edge rather than only behind its own text.
    return background + rendered + " " * max(0, width - _width(segments)) + RESET


def render_empty(theme: Theme, width: int, query: str) -> str:
    if query.strip():
        message = "no folder matches — ⏎ still creates it if the path exists"
    else:
        message = "no folders to suggest — type a path"
    return _text(theme, [Segment("", "  "), Segment("dim", truncate(message, max(0, width - 2)))])


# -- agent chips -----------------------------------------------------------


def render_chips(theme: Theme, width: int, chips: Sequence[Agent], selected: int) -> str:
    """The agent row, always visible so the choice never has to be discovered."""
    segments: List[Segment] = [Segment("dim", "  agent  ")]
    for index, chip in enumerate(chips):
        if index:
            segments.append(Segment("dim", " · "))
        if index == selected:
            role = "dim" if chip.is_none else "agent"
            segments.append(Segment(role, chip.label, True, True))
        else:
            segments.append(Segment("muted" if chip.is_none else "text", chip.label))
    if _width(segments) <= width:
        return _text(theme, segments)

    # Too many agents to show at once: scroll the row until the selection fits,
    # never past it, so the chip you are on is always the one you can see.
    labels = [chip.label for chip in chips]
    head = max(0, selected - 2)
    while head < selected and display_width("  agent  " + " · ".join(labels[head:])) > width:
        head += 1
    return _text(theme, [segments[0]] + _chips_from(theme, chips, selected, head, width))


def _chips_from(
    theme: Theme, chips: Sequence[Agent], selected: int, head: int, width: int
) -> List[Segment]:
    segments: List[Segment] = []
    used = display_width("  agent  ")
    if head:
        segments.append(Segment("dim", "‹ "))
        used += 2
    for index in range(head, len(chips)):
        chip = chips[index]
        piece = (" · " if index > head else "") + chip.label
        if used + display_width(piece) > width - 2:
            segments.append(Segment("dim", " ›"))
            break
        if index > head:
            segments.append(Segment("dim", " · "))
        if index == selected:
            role = "dim" if chip.is_none else "agent"
            segments.append(Segment(role, chip.label, True, True))
        else:
            segments.append(Segment("muted" if chip.is_none else "text", chip.label))
        used += display_width(piece)
    return segments


# -- target and footer -----------------------------------------------------


def render_target(
    theme: Theme,
    width: int,
    target: Optional[Target],
    agent: Optional[Agent],
    error: str = "",
) -> str:
    """What Enter is about to do, spelled out before it happens.

    The tab-versus-space decision is made from what is already open, so it can
    change under you as you type. Saying it plainly here is what keeps that
    from being a surprise.
    """
    if error:
        return _text(
            theme,
            [Segment("", "  "), Segment("error", truncate(error, max(0, width - 2)))],
        )
    if target is None:
        return ""

    segments: List[Segment] = [Segment("accent", "  " + ARROW + " ", True)]
    if target.kind == TAB:
        segments.append(Segment("text", "new tab in "))
        segments.append(Segment("space", target.workspace_label or "this space", True))
    else:
        segments.append(Segment("text", "new space "))
        segments.append(Segment("space", target.workspace_label or "", True))
    if agent is not None and not agent.is_none:
        segments.append(Segment("dim", "  ·  "))
        segments.append(Segment("agent", agent.label))
    return _text(theme, _fit(segments, width))


def _fit(segments: Sequence[Segment], width: int) -> List[Segment]:
    out: List[Segment] = []
    used = 0
    for segment in segments:
        room = width - used
        if room <= 0:
            break
        text = truncate(segment.text, room)
        out.append(segment._replace(text=text))
        used += display_width(text)
    return out


def render_footer(theme: Theme, width: int, hints: Sequence[Tuple[str, str]], counter: str) -> str:
    segments: List[Segment] = [Segment("", "  ")]
    for index, (key, label) in enumerate(hints):
        if index:
            segments.append(Segment("dim", "   "))
        segments.append(Segment("muted", key))
        if label:
            segments.append(Segment("dim", " " + label))
    used = _width(segments)
    counter_width = display_width(counter)
    while len(segments) > 1 and used + counter_width + 2 > width:
        segments.pop()
        while len(segments) > 1 and segments[-1].role == "dim" and not segments[-1].text.strip():
            segments.pop()
        used = _width(segments)
    segments.append(Segment("", " " * max(1, width - used - counter_width)))
    segments.append(Segment("dim", counter))
    return _text(theme, segments)


def render_rule(theme: Theme, width: int) -> str:
    return _text(theme, [Segment("dim", "─" * max(0, width))])


def scrollbar_column(total: int, visible: int, offset: int, height: int) -> Dict[int, str]:
    """Track and thumb for a list taller than its window, keyed by row index."""
    if total <= visible or height <= 0:
        return {}
    thumb = max(1, int(round(height * visible / float(total))))
    span = height - thumb
    reach = max(1, total - visible)
    start = int(round(span * min(offset, reach) / float(reach)))
    return {
        index: (SCROLL_THUMB if start <= index < start + thumb else SCROLL_TRACK)
        for index in range(height)
    }


def pad_row(text: str, width: int) -> str:
    return pad(text, width)


__all__ = [
    "Layout",
    "compute_layout",
    "render_chips",
    "render_empty",
    "render_footer",
    "render_input",
    "render_row",
    "render_rule",
    "render_target",
    "scrollbar_column",
    "SPACE",
    "pad_row",
]
