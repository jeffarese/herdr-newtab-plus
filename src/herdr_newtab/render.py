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
from .paths import SOURCE_PATH, SOURCE_RECENT, SOURCE_SPACE, SOURCE_ZOXIDE, Scored, parent_display
from .session import SMART, SPACE, TAB, Target
from .textutil import display_width, pad, truncate, truncate_middle, wrap
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


# The popup is sized for the help panel, which is a page of prose; the picker
# wants a short list whatever it is given. Ten is more than the six that fitted
# before and still a number you take in at a glance rather than read.
MAX_LIST = 10


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


def compute_layout(width: int, height: int) -> Layout:
    """Give the list the rows the chrome does not need, up to a cap.

    The chrome sheds from the bottom up as the popup shrinks: the key hints go
    first, then the line that says what Enter will do, then the agent row. The
    input and at least one suggestion always survive.

    Growing works the other way round. Past the cap the rows go to the note that
    spells the create mode out, and then to nothing: they sit as air between the
    list and the chrome, which stays on the floor of the popup where it was.
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
    return _text(theme, [Segment("", "  "), Segment("muted", truncate(message, max(0, width - 2)))])


# -- agent chips -----------------------------------------------------------


def chip_label(chip: Agent, selected: bool, skipping: bool) -> str:
    """The text of one chip, marked when it is about to skip its prompts.

    Only the selected chip can carry the mark, because the setting only applies
    to the agent Enter would start. Kept here rather than inlined so the click
    spans measured in ``app`` are computed from the same rule that draws them.
    """
    if selected and skipping and not chip.is_none:
        return chip.label + SKIP_MARK
    return chip.label


def render_chips(
    theme: Theme,
    width: int,
    chips: Sequence[Agent],
    selected: int,
    skipping: bool = False,
) -> str:
    """The agent row, always visible so the choice never has to be discovered."""
    segments: List[Segment] = [Segment("muted", "  agent  ")]
    for index, chip in enumerate(chips):
        if index:
            segments.append(Segment("dim", " · "))
        label = chip_label(chip, index == selected, skipping)
        if index == selected:
            role = "muted" if chip.is_none else ("warn" if skipping else "agent")
            segments.append(Segment(role, label, True, True))
        else:
            segments.append(Segment("muted" if chip.is_none else "text", label))
    if _width(segments) <= width:
        return _text(theme, segments)

    # Too many agents to show at once: scroll the row until the selection fits,
    # never past it, so the chip you are on is always the one you can see.
    labels = [chip_label(chip, index == selected, skipping) for index, chip in enumerate(chips)]
    head = max(0, selected - 2)
    while head < selected and display_width("  agent  " + " · ".join(labels[head:])) > width:
        head += 1
    return _text(theme, [segments[0]] + _chips_from(theme, chips, selected, head, width, skipping))


def _chips_from(
    theme: Theme,
    chips: Sequence[Agent],
    selected: int,
    head: int,
    width: int,
    skipping: bool = False,
) -> List[Segment]:
    segments: List[Segment] = []
    used = display_width("  agent  ")
    if head:
        segments.append(Segment("dim", "‹ "))
        used += 2
    for index in range(head, len(chips)):
        chip = chips[index]
        label = chip_label(chip, index == selected, skipping)
        piece = (" · " if index > head else "") + label
        if used + display_width(piece) > width - 2:
            segments.append(Segment("dim", " ›"))
            break
        if index > head:
            segments.append(Segment("dim", " · "))
        if index == selected:
            role = "muted" if chip.is_none else ("warn" if skipping else "agent")
            segments.append(Segment(role, label, True, True))
        else:
            segments.append(Segment("muted" if chip.is_none else "text", label))
        used += display_width(piece)
    return segments


# -- target and footer -----------------------------------------------------


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

    An overridden ``mode`` is badged at the end, because then the sentence is no
    longer the whole story: it says where this folder is going, and the badge
    says that every folder is going there until the toggle moves.
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
        segments.append(Segment("warn" if skipping else "agent", agent.label))
        # Spelled out rather than left to the chip's mark, because this is the
        # line that says what Enter does, and "prompts off" is part of that.
        if skipping:
            segments.append(Segment("warn", " (skip permissions)"))
    if mode != SMART:
        segments.append(Segment("muted", "   [" + mode + "]"))
    return _text(theme, _fit(segments, width))


MODE_NOTES = {
    SMART: "a tab when that folder already has a space open, a new space when it does not",
    TAB: "always a tab — that folder's own space when it has one, otherwise the one you are in",
    SPACE: "always a new space, even when that folder already has one",
}


def render_mode_note(theme: Theme, width: int, mode: str) -> str:
    """The create mode in words, under the line that shows its answer.

    The target line says what Enter does to this folder; a badge saying `[tab]`
    says the answer was insisted on but not what insisting means. With the room
    for one more line, saying it is better than making it discoverable.
    """
    return _text(
        theme,
        _fit(
            [
                Segment("muted", "     ^T "),
                Segment("space", mode + "  "),
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


HELP_TITLE = "what this popup does"
HELP_CLOSE = "? or esc  close"

HELP_INTRO = (
    "Type a folder, pick an agent, press ⏎. The line above the hints says what ⏎ is about to do."
)

HELP_CREATE = (
    "where ⏎ lands. Three positions, and it stays on the one you leave it on — popup to popup, and "
    "across restarts. You are on the marked one."
)

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
        "always a new space, even when the folder already has one. Duplicates are yours to make.",
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
        "before you type: folders picked recently, then ones that already have a space (badged "
        "with its name), then zoxide's list, then the children of your roots. Type a / ~ or . and "
        "it completes real directories instead.",
    ),
    (
        "the agent",
        "a plain shell plus every agent Herdr can start that is installed here. Each folder "
        "remembers the one you last opened it with; the new one is named after the folder.",
    ),
)

# Two columns, because the keys are the part of this you scan rather than read
# and the rows they save are rows the prose above wanted.
HELP_KEYS = (
    (("⇥", "complete, or move the agent"), ("^W", "back one path segment")),
    (("→", "fill in the highlighted folder"), ("^U", "clear the input")),
    (("↑↓", "move the folders"), ("^R", "rescan the filesystem")),
    (("⇧⇥", "move the agents"), ("click", "select, click again to create")),
    (("⏎", "create it"), ("esc", "give up and close the popup")),
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
    rows.extend(_help_entry(theme, width, "^T", HELP_CREATE, 5))
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
        segments.append(Segment("", " " * gap))
        segments.append(Segment("muted", HELP_CLOSE))
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
    rows = []
    for index, line in enumerate(lines):
        label = keys if index == 0 else ""
        rows.append(
            _text(
                theme,
                [
                    Segment("", " " * HELP_INDENT),
                    Segment("muted", pad(truncate(label, column), column)),
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
        segments.append(Segment("muted", pad(truncate(keys, 6), 6)))
        segments.append(Segment("text", pad(truncate(text, max(1, room)), max(1, room))))
    return _text(theme, _fit(segments, width))


def _help_mode(theme: Theme, width: int, name: str, text: str, current: bool) -> List[str]:
    """One create mode, marked with the same bar the list uses for its own row."""
    column = 10
    mark = SELECTED_BAR + " "
    room = max(1, width - 7 - column)
    rows = []
    for index, line in enumerate(wrap(text, room)):
        rows.append(
            _text(
                theme,
                [
                    Segment("", "     "),
                    Segment("accent", mark if current and not index else "  ", True),
                    Segment("space", pad(name if not index else "", column - 2), current),
                    Segment("text" if current else "muted", line),
                ],
            )
        )
    return rows


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
    "chip_label",
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
