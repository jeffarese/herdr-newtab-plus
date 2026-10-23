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

from .agents import SKIP_MARK, Agent
from .icons import GAP
from .paths import (
    SOURCE_PATH,
    SOURCE_RECENT,
    SOURCE_SPACE,
    SOURCE_ZOXIDE,
    Contents,
    Scored,
    parent_display,
)
from .session import MODES, SMART, SPACE, TAB, FolderAgent, Target
from .textutil import display_width, pad, truncate, truncate_middle, wrap
from .theme import BOLD, RESET, STATUS_GLYPHS, UNDERLINE, Theme

PROMPT = "❯"
SELECTED_BAR = "▌"
SCROLL_TRACK = "│"
SCROLL_THUMB = "┃"
ARROW = "→"
META_SEP = " · "

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


# The popup is sized for the help panel, which is a page of prose; the picker
# wants a short list whatever it is given. Ten is more than the six that fitted
# before and still a number you take in at a glance rather than read.
MAX_LIST = 10


# The preview column, as herdr-bar sizes its own: a third of the popup, within
# bounds, and only when the list keeps enough to show a row whole beside it.
PREVIEW_MIN = 28
PREVIEW_MAX = 56
MIN_LIST = 46
SEPARATOR = " │ "
# Below this many rows there is no room for the folder's contents under its
# name and path, and a preview of only a heading says nothing the row did not.
PREVIEW_ROWS = 5
# How wide the popup has to be before the preview shows by itself. Asked for --
# with ^O, or ``"preview": true`` -- it only needs the room to fit.
PREVIEW_AUTO = 92
PREVIEW_FITS = PREVIEW_MIN + MIN_LIST + len(SEPARATOR)


class Layout(NamedTuple):
    inner_width: int
    list_top: int
    list_height: int
    rules: bool
    chips: bool
    target: bool
    footer: bool
    note: bool = False
    spare: int = 0
    list_width: int = 0  # the whole inner width when there is no preview
    preview_width: int = 0


def compute_layout(width: int, height: int, preview: bool = False) -> Layout:
    """Give the list the rows the chrome does not need, up to a cap.

    The chrome sheds from the bottom up as the popup shrinks: the key hints go
    first, then the line that says what Enter will do, then the agent row. The
    input and at least one suggestion always survive.

    Growing works the other way round. Past the cap the rows go to the note that
    spells the create mode out, and then to nothing: they sit as air between the
    list and the chrome, which stays on the floor of the popup where it was.

    The preview, when asked for and when it fits, takes a column off the right
    of the list and every row the list has, air included: the rows the list was
    not allowed are the ones a folder's contents have a use for.
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

    room = max(1, height - chrome)
    note = target and room > MAX_LIST + 1
    if note:
        room -= 1

    list_width, preview_width = inner, 0
    if preview and inner >= PREVIEW_FITS and room >= PREVIEW_ROWS:
        preview_width = min(PREVIEW_MAX, max(PREVIEW_MIN, inner // 3))
        list_width = inner - preview_width - len(SEPARATOR)
    return Layout(
        inner,
        list_top,
        min(room, MAX_LIST),
        rules,
        chips,
        target,
        footer,
        note,
        max(0, room - MAX_LIST),
        list_width,
        preview_width,
    )


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


def _meta(
    theme: Theme, row: Scored, agent: Optional[str], status: Optional[str]
) -> List[Segment]:
    """The right-hand end of a row: who is already there, and where it came from.

    An agent standing in the folder leads with its status glyph, in the colours
    herdr's sidebar uses for it, so a folder with something waiting in it is
    told apart from one that is merely open. The folder's own agent -- the one
    the chips will move to when the row is selected -- is named here only when
    there is no logo at the front of the row to say it instead.
    """
    pieces: List[List[Segment]] = []
    if agent and not theme.logo(agent):
        pieces.append([Segment(theme.agent_role(agent), agent)])
    badge = _badge(row)
    glyph = STATUS_GLYPHS.get(status or "")
    place: List[Segment] = []
    if glyph:
        place.append(Segment(status or "", glyph + (" " if badge else "")))
    if badge:
        place.append(Segment("space" if row.suggestion.source == SOURCE_SPACE else "dim", badge))
    if place:
        pieces.append(place)
    out: List[Segment] = []
    for index, piece in enumerate(pieces):
        if index:
            out.append(Segment("rule", META_SEP))
        out.extend(piece)
    return out


def render_row(
    theme: Theme,
    row: Scored,
    width: int,
    selected: bool,
    name_room: Optional[int] = None,
    agent: Optional[str] = None,
    status: Optional[str] = None,
) -> str:
    """One folder: its name, where it lives, and what already knows about it.

    ``agent`` is the agent this folder was last opened with, and ``status`` the
    most urgent status of any agent standing in it right now. With the icon font
    the agent's logo leads the row, in a gutter every row keeps whether it has a
    logo or not, so the names stay in one column as the list scrolls.
    """
    background = theme.selection_bg if selected else ""

    marker = Segment("accent", SELECTED_BAR + " ", True) if selected else Segment("", "  ")
    segments: List[Segment] = [marker]
    if theme.agent_icons:
        logo = theme.logo(agent)
        segments.append(
            Segment(theme.agent_role(agent), logo + GAP) if logo else Segment("", " " + GAP)
        )

    meta = _meta(theme, row, agent, status) if width >= 44 else []
    meta_width = _width(meta) + 2 if meta else 0

    lead = _width(segments)
    if name_room is None:
        name_room = max(MIN_NAME, min(MAX_NAME, (width - lead - meta_width) // 2))
    name = truncate(row.suggestion.name, name_room)
    segments.extend(_highlight(name, row.name_positions, "text", selected))
    segments.append(Segment("", " " * max(1, name_room - display_width(name) + 1)))

    parent_room = max(0, width - _width(segments) - meta_width)
    if parent_room >= 4:
        parent = truncate_middle(parent_display(row.suggestion.path), parent_room)
        segments.extend(_highlight(parent, row.path_positions, "muted", False))

    if meta_width:
        used = _width(segments)
        segments.append(Segment("", " " * max(1, width - used - _width(meta))))
        segments.extend(meta)

    segments = _fit(segments, width)
    rendered = _text(theme, segments, background)
    if not background:
        return rendered
    # A selected row is a band across the list, so it has to be filled to the
    # edge rather than only behind its own text.
    return background + rendered + " " * max(0, width - _width(segments)) + RESET


def render_empty(theme: Theme, width: int, height: int, query: str) -> List[str]:
    """What the list says when it has nothing to show, and what to do about it."""
    if query.strip():
        headline, hint = "no folder matches", "⏎ still creates it, if the path exists"
    else:
        headline, hint = "no folders to suggest", "start with ~ / or . to complete real folders"
    room = max(0, width - 2)
    rows = [
        _text(theme, [Segment("", "  "), Segment("text", truncate(headline, room), True)]),
        _text(theme, [Segment("", "  "), Segment("muted", truncate(hint, room))]),
    ]
    return (rows + [""] * height)[: max(1, height)]


# -- agent chips -----------------------------------------------------------

CHIP_LEAD = "  agent "
CHIP_SEP = " "
SHELL_MARK = "$"  # stands in the logo column for the plain shell, as in herdr-bar


def chip_label(chip: Agent, selected: bool, skipping: bool) -> str:
    """The text of one chip, marked when it is about to skip its prompts.

    Only the selected chip can carry the mark, because the setting only applies
    to the agent Enter would start.
    """
    if selected and skipping and not chip.is_none:
        return chip.label + SKIP_MARK
    return chip.label


def _chip(theme: Theme, chip: Agent, selected: bool, skipping: bool, pill: bool) -> List[Segment]:
    """One chip, padded by a column on each side so a selected one reads as a pill.

    The logo keeps its vendor's colour whether or not the chip is selected: it is
    what the eye finds the agent by. The label is what changes -- quiet until it
    is the one Enter would start, then bold in the agent's own colour, or in the
    warning colour once its prompts are about to be skipped.
    """
    label = chip_label(chip, selected, skipping)
    warn = selected and skipping and not chip.is_none
    role = theme.agent_role(chip.kind)
    out = [Segment("", " ")]
    if theme.agent_icons:
        logo = SHELL_MARK if chip.is_none else theme.logo(chip.kind)
        if logo:
            out.append(Segment("muted" if chip.is_none else ("warn" if warn else role), logo + GAP))
    if selected:
        label_role = "text" if chip.is_none else ("warn" if warn else role)
        # Without a selection colour to draw the pill in, the underline is what
        # is left to say which chip it is.
        out.append(Segment(label_role, label, True, not pill))
    else:
        out.append(Segment("muted", label))
    out.append(Segment("", " "))
    return out


def chips_row(
    theme: Theme,
    width: int,
    chips: Sequence[Agent],
    selected: int,
    skipping: bool = False,
) -> Tuple[str, List[Tuple[int, int]]]:
    """The agent row, and the columns each chip covers so a click can find it.

    The spans come from the same pass that draws the row, so a chip widened by
    its skip mark, or a row scrolled to keep the selection on screen, can never
    put a click on the wrong agent. A chip scrolled out of view spans nothing.
    """
    pill = bool(theme.selection_bg)
    cells = [
        _chip(theme, chip, index == selected, skipping, pill) for index, chip in enumerate(chips)
    ]
    widths = [_width(cell) for cell in cells]
    lead = display_width(CHIP_LEAD)
    count = len(cells)

    def needed(head: int, tail: int) -> int:
        inner = sum(widths[head:tail]) + max(0, tail - head - 1) * len(CHIP_SEP)
        return lead + (2 if head else 0) + inner + (2 if tail < count else 0)

    head, tail = 0, count
    if needed(0, count) > width:
        # Too many agents at once: scroll until the selection fits, never past
        # it, so the chip you are on is always one you can see.
        head = max(0, selected - 2)
        while head < selected and needed(head, selected + 1) > width:
            head += 1
        tail = selected + 1
        while tail < count and needed(head, tail + 1) <= width:
            tail += 1

    spans: List[Tuple[int, int]] = [(-1, -1)] * count
    out = [_text(theme, [Segment("muted", CHIP_LEAD)])]
    column = lead
    if head:
        out.append(_text(theme, [Segment("dim", "‹ ")]))
        column += 2
    for index in range(head, tail):
        if index > head:
            out.append(CHIP_SEP)
            column += len(CHIP_SEP)
        if index == selected and pill:
            out.append(theme.selection_bg + _text(theme, cells[index], theme.selection_bg) + RESET)
        else:
            out.append(_text(theme, cells[index]))
        spans[index] = (column, column + widths[index])
        column += widths[index]
    if tail < count:
        out.append(_text(theme, [Segment("dim", " ›")]))
        column += 2

    if column > width:
        # Not even the selected chip fits: say what can be said, unpainted by
        # the pill, rather than run off the edge.
        flat = [Segment("muted", CHIP_LEAD)] + [seg for cell in cells[head:tail] for seg in cell]
        return _text(theme, _fit(flat, width)), [(-1, -1)] * count
    return "".join(out), spans


def render_chips(
    theme: Theme,
    width: int,
    chips: Sequence[Agent],
    selected: int,
    skipping: bool = False,
) -> str:
    """The agent row, always visible so the choice never has to be discovered."""
    return chips_row(theme, width, chips, selected, skipping)[0]


# -- target and footer -----------------------------------------------------


def mode_switch(theme: Theme, mode: str) -> Tuple[str, int]:
    """All three create modes, with the one the picker is on drawn as a pill.

    The same pill as the selected agent chip, because it is the same kind of
    thing: one choice out of a row of them, moved by a key. Showing the two it
    is not on is what makes ^T something you can read off the screen rather
    than something you have to know is there.
    """
    pill = theme.selection_bg
    out: List[str] = []
    for index, name in enumerate(MODES):
        if index:
            out.append(" ")
        if name != mode:
            out.append(_text(theme, [Segment("dim", " " + name + " ")]))
        elif pill:
            label = _text(theme, [Segment("accent", name, True)], pill)
            out.append(pill + " " + label + " " + RESET)
        else:
            out.append(" " + _text(theme, [Segment("accent", name, True, True)]) + " ")
    return "".join(out), sum(len(name) + 2 for name in MODES) + len(MODES) - 1


def render_target(
    theme: Theme,
    width: int,
    target: Optional[Target],
    agent: Optional[Agent],
    error: str = "",
    skipping: bool = False,
    mode: str = SMART,
) -> str:
    """What Enter is about to do, spelled out before it happens.

    The tab-versus-space decision is made from what is already open, so it can
    change under you as you type. Saying it plainly here is what keeps that
    from being a surprise.

    The create mode sits at the right-hand end as a three-way switch, so the
    sentence and the rule that produced it are read together. A popup too narrow
    for the switch badges an overridden mode instead, because then the sentence
    is no longer the whole story.
    """
    if error:
        segments = [Segment("", "  "), Segment("error", error)]
    elif target is None:
        return ""
    else:
        segments = [Segment("accent", "  " + ARROW + " ", True)]
        if target.kind == TAB:
            segments.append(Segment("text", "new tab in "))
            segments.append(Segment("space", target.workspace_label or "this space", True))
        else:
            segments.append(Segment("text", "new space "))
            segments.append(Segment("space", target.workspace_label or "", True))
        if agent is not None and not agent.is_none:
            role = "warn" if skipping else theme.agent_role(agent.kind)
            segments.append(Segment("rule", "  ·  "))
            logo = theme.logo(agent.kind)
            if logo:
                segments.append(Segment(role, logo + GAP))
            segments.append(Segment(role, agent.label))
            # Spelled out rather than left to the chip's mark, because this is
            # the line that says what Enter does, and "prompts off" is part of it.
            if skipping:
                segments.append(Segment("warn", " (skip permissions)"))

    switch, switch_width = mode_switch(theme, mode)
    if _width(segments) + 3 + switch_width <= width:
        gap = width - _width(segments) - switch_width
        return _text(theme, segments) + " " * gap + switch
    if mode != SMART and not error:
        segments.append(Segment("muted", "   [" + mode + "]"))
    return _text(theme, _fit(segments, width))


MODE_NOTES = {
    SMART: "a tab when that folder already has a space open, a new space when it does not",
    TAB: "always a tab: that folder's own space when it has one, otherwise the one you are in",
    SPACE: "always a new space, even when that folder already has one",
}


def render_mode_note(theme: Theme, width: int, mode: str) -> str:
    """The create mode in words, under the line that shows its answer.

    The switch says which position the toggle is on; what that position means
    does not fit in a word. With the room for one more line, saying it is better
    than making it discoverable. Indented to start under the sentence it
    qualifies, not under the arrow.
    """
    return _text(
        theme,
        _fit(
            [
                Segment("", "    "),
                Segment("accent", mode),
                Segment("rule", " — "),
                Segment("muted", MODE_NOTES.get(mode, "")),
            ],
            width,
        ),
    )


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


def _hint_row(groups: Sequence[List[Segment]], gap: str) -> List[Segment]:
    segments: List[Segment] = [Segment("", "  ")]
    for index, group in enumerate(groups):
        if index:
            segments.append(Segment("dim", gap))
        segments.extend(group)
    return segments


def render_footer(
    theme: Theme,
    width: int,
    hints: Sequence[Tuple[str, str]],
    counter: str,
    guide: str = "",
) -> str:
    """The key hints, tightened and then shed from the right until they fit.

    Air goes before information: the gap between hints closes up first, and only
    a row that still does not fit loses a hint. What it loses it loses whole --
    half a hint, a key with its label cut away, reads as a key that does nothing.

    ``guide`` is the way into the help panel, and it is the one thing here that
    is never shed: the row a beginner needs most is the crowded one, so the key
    that explains the rest of it keeps its corner and gives up its label instead.
    """
    hint_groups = [
        [Segment("muted", key)] + ([Segment("dim", " " + label)] if label else [])
        for key, label in hints
    ]
    # The guide's label is the first thing tried and the first thing dropped, so
    # a popup with room says "? help" and a crowded one still says "?".
    spellings = [guide]
    if " " in guide:
        spellings.append(guide.split(" ", 1)[0])
    for index, spelling in enumerate(spellings):
        tail = _footer_tail(spelling, counter)
        segments, kept = _footer_hints(hint_groups, width - _width(tail) - 2)
        if kept == len(hint_groups) or index == len(spellings) - 1:
            break
    used = _width(segments)
    if used + _width(tail) + 1 <= width:
        segments.append(Segment("", " " * (width - used - _width(tail))))
        segments.extend(tail)
    # A popup too narrow for one hint gives the counter up as well, and _fit is
    # what keeps the row inside its width either way.
    return _text(theme, _fit(segments, width))


def _footer_tail(guide: str, counter: str) -> List[Segment]:
    """The right-hand corner: the way into the help, then how many folders."""
    tail: List[Segment] = []
    if guide:
        tail.append(Segment("muted", guide))
    if counter:
        tail.append(Segment("dim", ("  " if guide else "") + counter))
    return tail


def _footer_hints(groups: Sequence[List[Segment]], room: int) -> Tuple[List[Segment], int]:
    kept = list(groups)
    while True:
        for gap in ("   ", "  "):
            segments = _hint_row(kept, gap)
            if _width(segments) <= room:
                return segments, len(kept)
        if len(kept) <= 1:
            return segments, len(kept)
        kept.pop()


# -- preview ---------------------------------------------------------------

STATUS_WORDS = {
    "blocked": "needs you",
    "working": "working",
    "done": "done",
    "idle": "idle",
}

PREVIEW_LABEL = 8  # "branch" and two columns of air


class Preview(NamedTuple):
    """Everything the preview says about one folder, gathered by the picker."""

    name: str
    path: str  # as displayed: home folded to ~
    contents: Optional[Contents]  # None when it is not a folder (yet)
    branch: Optional[str] = None
    space: Optional[str] = None  # the label of the space it already has
    agents: Sequence[FolderAgent] = ()
    hidden: bool = False  # list dot-files too


def render_preview(theme: Theme, width: int, height: int, preview: Optional[Preview]) -> List[str]:
    """The folder under the cursor: where it is, who is in it, and what is inside.

    Laid out the way herdr-bar previews a pane -- a bold heading, the path under
    it, a rule -- so the two popups read as one family. The facts that decide
    what Enter does come first, because they are what the list has no room to
    spell out; the listing fills whatever is left and says how much it left out.
    """
    if preview is None or width <= 0 or height <= 0:
        return [""] * max(0, height)

    rows = [
        _text(theme, [Segment("text", truncate(preview.name, width), True)]),
        _text(theme, [Segment("muted", truncate_middle(preview.path, width))]),
        render_rule(theme, width),
    ]

    facts: List[List[Segment]] = []
    if preview.branch:
        facts.append([Segment("text", preview.branch)])
    labels = ["branch"] if preview.branch else []
    if preview.space:
        facts.append([Segment("space", preview.space, True)])
        labels.append("space")
    for index, agent in enumerate(preview.agents):
        line: List[Segment] = []
        glyph = STATUS_GLYPHS.get(agent.status)
        line.append(Segment(agent.status, glyph + " ") if glyph else Segment("", "  "))
        logo = theme.logo(agent.kind)
        if logo:
            line.append(Segment(theme.agent_role(agent.kind), logo + GAP))
        line.append(Segment(theme.agent_role(agent.kind), agent.name or agent.kind or "agent"))
        word = STATUS_WORDS.get(agent.status)
        if word:
            line.append(Segment(agent.status, "  " + word))
        facts.append(line)
        labels.append("agents" if not index else "")
    for label, fact in zip(labels, facts):
        rows.append(
            _text(theme, _fit([Segment("dim", pad(label, PREVIEW_LABEL))] + fact, width))
        )
    if facts:
        rows.append("")

    room = height - len(rows)
    if room > 0:
        rows.extend(_preview_listing(theme, width, room, preview))
    return (rows + [""] * height)[:height]


def _preview_listing(theme: Theme, width: int, room: int, preview: Preview) -> List[str]:
    """What is inside the folder, directories first, cut to fit with a count."""
    listed = preview.contents
    if listed is None:
        return [_text(theme, [Segment("dim", truncate("not a folder yet", width))])]
    if not listed.readable:
        return [_text(theme, [Segment("dim", truncate("not readable", width))])]

    def shown(names: Sequence[str]) -> List[str]:
        return [name for name in names if preview.hidden or not name.startswith(".")]

    entries = [(name, True) for name in shown(listed.dirs)]
    entries += [(name, False) for name in shown(listed.files)]
    if not entries:
        return [_text(theme, [Segment("dim", truncate("empty", width))])]

    if len(entries) > room:
        keep = max(0, room - 1)  # the last row says how many did not fit
        more = len(entries) - keep
        entries = entries[:keep]
    else:
        more = 0
    out = []
    for name, is_dir in entries:
        if is_dir:
            segments = [Segment("text", truncate(name, max(1, width - 1))), Segment("dim", "/")]
        else:
            segments = [Segment("muted", truncate(name, width))]
        out.append(_text(theme, _fit(segments, width)))
    if more:
        out.append(_text(theme, [Segment("dim", truncate("… %d more" % more, width))]))
    return out


HELP_TITLE = "what this popup does"
HELP_CLOSE = "? or esc  close"

HELP_INTRO = (
    "Type a folder, pick an agent, press ⏎. The line above the hints says what ⏎ is about to do."
)

HELP_CREATE = "where ⏎ lands, kept across popups and restarts. You are on the marked one."

# The block the panel exists for: which position the toggle is on decides what
# Enter does, and none of that fits on the one line the picker can spare.
HELP_MODES = (
    (
        "smart",
        "a tab in the space that folder already has, a new space when it has none — and it \"has "
        "a space\" when some pane in one is standing in it, symlinks followed.",
    ),
    (
        "tab",
        "always a tab: in that folder's own space when it has one, otherwise in the space you are "
        "looking at now.",
    ),
    (
        "space",
        "always a new space, even when the folder already has one.",
    ),
)

HELP_NOTES = (
    (
        "^Y",
        "starts that agent with its permission prompts off, for this tab only — never remembered, "
        "unlike the agent. claude and codex have checked flags; anything else refuses rather than "
        "guess until skip_permission_flags in config.json teaches it one.",
    ),
    (
        "the list",
        "before you type: folders picked recently, then ones that already have a space — badged "
        "with its name, and ◉ ◐ ● ✓ when an agent is in it — then zoxide's list, then the "
        "children of your roots. Type a / ~ or . and it completes real directories instead.",
    ),
    (
        "the agent",
        "a shell, or any agent Herdr can start that is installed here. Each folder remembers the "
        "one you last opened it with and shows it on its row; the new one is named after the "
        "folder.",
    ),
)

# Two columns, because the keys are the part of this you scan rather than read
# and the rows they save are rows the prose above wanted.
HELP_KEYS = (
    (("⇥", "complete, or move the agent"), ("^W", "back one path segment")),
    (("→", "fill in the highlighted folder"), ("^U", "clear the input")),
    (("↑↓", "move the folders"), ("^R", "rescan the filesystem")),
    (("⇧⇥", "move the agents"), ("^O", "show or hide the folder preview")),
    (("⏎", "create it"), ("click", "select, click again to create")),
    (("esc", "give up and close the popup"), ("", "")),
)

HELP_KEY_COLUMN = 11
HELP_INDENT = 2


def render_help(theme: Theme, width: int, height: int, mode: str = SMART) -> List[str]:
    """The whole popup, given over to what the keys and the modes mean.

    Ordered by what someone opening it is stuck on rather than by how often a key
    is pressed: the create mode first, because it is the one thing on screen
    whose three positions are not self-evident, and the movement keys last,
    because those are the ones a guess gets right.

    The position the picker is on is marked with the same bar the list uses for
    the row it is on, so the panel answers "which one am I on" as well as "what
    are they" -- the first question anybody opens it with.

    Prose is wrapped rather than written to fit, so the panel says the whole
    thing at whatever width it is given instead of saying a shorter, vaguer
    version of it.
    """
    rule = render_rule(theme, width)
    rows = [_help_title(theme, width), rule]
    rows.extend(_help_paragraph(theme, width, HELP_INTRO))
    rows.append("")
    rows.extend(_help_entry(theme, width, "^T", HELP_CREATE, HELP_KEY_COLUMN))
    for name, text in HELP_MODES:
        rows.extend(_help_mode(theme, width, name, text, name == mode))
    rows.append("")
    for index, (key, text) in enumerate(HELP_NOTES):
        if index:
            rows.append("")
        rows.extend(_help_entry(theme, width, key, text, HELP_KEY_COLUMN))
    rows.append(rule)
    for left, right in HELP_KEYS:
        rows.append(_help_key_row(theme, width, left, right))
    # Shed from the bottom, the way the picker's own chrome does: what goes
    # first is the half a guess would have got right anyway.
    return rows[:height] + [""] * max(0, height - len(rows))


def _help_title(theme: Theme, width: int) -> str:
    gap = width - 2 - display_width(HELP_TITLE) - display_width(HELP_CLOSE)
    segments = [
        Segment("", " "),
        Segment("accent", truncate(HELP_TITLE, max(0, width - 2)), True),
    ]
    if gap >= 2:
        keys, label = HELP_CLOSE.split("  ", 1)
        segments.append(Segment("", " " * gap))
        segments.append(Segment("accent", keys))
        segments.append(Segment("dim", "  " + label))
    return _text(theme, segments)


def _help_paragraph(theme: Theme, width: int, text: str) -> List[str]:
    indent = " " * HELP_INDENT
    return [
        _text(theme, [Segment("", indent), Segment("text", line)])
        for line in wrap(text, max(1, width - HELP_INDENT * 2))
    ]


def _help_entry(theme: Theme, width: int, keys: str, text: str, column: int) -> List[str]:
    """A key, and everything there is to say about it, wrapped under itself."""
    room = max(1, width - HELP_INDENT - column - 1)
    lines = wrap(text, room)
    # A key is drawn as one, in the accent the picker's own chrome uses for the
    # thing you act on; a topic ("the list") is a heading, so it is bold text.
    topic = " " in keys
    rows = []
    for index, line in enumerate(lines):
        label = keys if index == 0 else ""
        rows.append(
            _text(
                theme,
                [
                    Segment("", " " * HELP_INDENT),
                    Segment(
                        "text" if topic else "accent", pad(truncate(label, column), column), True
                    ),
                    Segment("text", line),
                ],
            )
        )
    return rows


def _help_key_row(
    theme: Theme, width: int, left: Tuple[str, str], right: Tuple[str, str]
) -> str:
    half = max(10, (width - HELP_INDENT) // 2)
    segments: List[Segment] = [Segment("", " " * HELP_INDENT)]
    for index, (keys, text) in enumerate((left, right)):
        room = half - HELP_KEY_COLUMN if index == 0 else width - HELP_INDENT - half - 6
        segments.append(Segment("accent", pad(truncate(keys, 6), 6)))
        segments.append(Segment("text", pad(truncate(text, max(1, room)), max(1, room))))
    return _text(theme, _fit(segments, width))


def _help_mode(theme: Theme, width: int, name: str, text: str, current: bool) -> List[str]:
    """One create mode, marked with the same bar the list uses for its own row.

    Indented to start where the ^T entry's own text does, so the three positions
    read as the body of that entry rather than as entries of their own.
    """
    column = 7
    mark = SELECTED_BAR + " "
    indent = HELP_INDENT + HELP_KEY_COLUMN
    room = max(1, width - indent - len(mark) - column)
    rows = []
    for index, line in enumerate(wrap(text, room)):
        rows.append(
            _text(
                theme,
                _fit(
                    [
                        Segment("", " " * indent),
                        Segment("accent", mark if current and not index else "  ", True),
                        Segment(
                            "accent" if current else "muted",
                            pad(name if not index else "", column),
                            current,
                        ),
                        Segment("text" if current else "muted", line),
                    ],
                    width,
                ),
            )
        )
    return rows


def render_rule(theme: Theme, width: int, junction: int = -1, glyph: str = "┼") -> str:
    """A horizontal rule; ``junction`` is the column a vertical rule meets it at."""
    line = "─" * max(0, width)
    if 0 <= junction < width:
        line = line[:junction] + glyph + line[junction + 1 :]
    return _text(theme, [Segment("rule", line)])


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
    "chip_label",
    "compute_layout",
    "render_chips",
    "render_empty",
    "render_footer",
    "render_input",
    "render_preview",
    "render_row",
    "render_rule",
    "render_target",
    "scrollbar_column",
    "SPACE",
    "pad_row",
]
