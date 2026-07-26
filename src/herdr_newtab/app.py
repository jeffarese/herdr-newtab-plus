"""The picker: state, keys, and the frame loop.

One screen, two fields. The folder field leads because it is the one you type;
the agent field is a row of chips that is always visible, so the optional half
of the decision never has to be discovered. Between them sits the only piece of
inference in the plugin -- whether this folder becomes a tab in a space that is
already open or a space of its own -- spelled out on the line above the hints so
Enter is never a surprise.

Nothing here talks to Herdr except through the snapshot taken when the picker
opens. Creating anything happens after the popup is gone, in ``cli``.
"""

from __future__ import annotations

import os
import time
from typing import Dict, List, NamedTuple, Optional, Sequence, Tuple

from . import paths, render
from .agents import Agent, initial_index
from .config import Config
from .keys import KEY, MOUSE, PASTE, PRESS, TEXT, WHEEL_DOWN, WHEEL_UP, Event, KeyDecoder
from .paint import Painter
from .session import Target, normalize, resolve_target
from .state import Recents
from .term import Terminal
from .theme import Theme

ESC_TIMEOUT = 0.06
STATUS_SECONDS = 2.5

CANCEL = "cancel"
CONFIRM = "confirm"


class Choice(NamedTuple):
    """What the picker returns: a folder, an agent, and where it should land."""

    folder: str
    agent: Agent
    target: Target


class Picker(object):
    def __init__(
        self,
        snapshot: Dict[str, object],
        catalog: Sequence[paths.Suggestion],
        chips: Sequence[Agent],
        config: Config,
        recents: Recents,
        theme: Theme,
        base: str,
    ) -> None:
        self.snapshot = snapshot
        self.catalog = list(catalog)
        self.chips = list(chips)
        self.config = config
        self.recents = recents
        self.theme = theme
        self.base = base

        self.query = ""
        self.cursor = 0
        self.rows: List[paths.Scored] = []
        self.selected = 0
        self.offset = 0
        self.chip = initial_index(self.chips, config.default_agent, recents.last_agent())
        self._chip_pinned = False

        self.status: Optional[str] = None
        self.status_until = 0.0
        self._dirty = True
        self._painter = Painter()
        self._list_height = 1
        self._row_top = 0
        self._chip_row = -1
        self._chip_spans: List[Tuple[int, int]] = []

        self.refilter()

    # -- derived state ------------------------------------------------------

    def refilter(self, keep: Optional[str] = None) -> None:
        self.rows = paths.suggest(
            self.query,
            self.catalog,
            self.base,
            self.config.hidden,
            self.config.max_suggestions,
        )
        self.selected = 0
        if keep:
            for index, row in enumerate(self.rows):
                if row.suggestion.path == keep:
                    self.selected = index
                    break
        self.offset = 0
        self._sync_agent()
        self._dirty = True

    def selected_row(self) -> Optional[paths.Scored]:
        if not self.rows:
            return None
        return self.rows[min(self.selected, len(self.rows) - 1)]

    def folder(self) -> str:
        """The folder Enter would use: the highlighted row, or what was typed."""
        row = self.selected_row()
        if row is not None:
            return row.suggestion.path
        return paths.resolve_input(self.query, self.base)

    def agent(self) -> Agent:
        return self.chips[min(self.chip, len(self.chips) - 1)]

    def ghost(self) -> str:
        row = self.selected_row()
        return paths.completion_for(self.query, row.suggestion if row else None)

    def target(self) -> Optional[Target]:
        folder = self.folder()
        if not folder:
            return None
        return resolve_target(folder, self.snapshot, self.config.create)

    def error(self) -> str:
        if self.status:
            return self.status
        folder = self.folder()
        if not folder:
            return ""
        if not paths.is_directory(folder):
            return "%s does not exist" % paths.display(folder)
        return ""

    def _sync_agent(self) -> None:
        """Follow the folder's remembered agent until the row is touched by hand.

        Moving through the list re-answers the agent question for free, which is
        most of why the picker knows about agents at all. Once the chips have
        been moved deliberately, that choice sticks.
        """
        if self._chip_pinned or self.config.default_agent != "last":
            return
        folder = self.folder()
        if not folder:
            return
        remembered = self.recents.agent_for(folder)
        if remembered is None:
            return
        for index, chip in enumerate(self.chips):
            if chip.kind == remembered:
                self.chip = index
                return

    def flash(self, message: str) -> None:
        self.status = message
        self.status_until = time.monotonic() + STATUS_SECONDS
        self._dirty = True

    # -- editing ------------------------------------------------------------

    def set_query(self, query: str, cursor: Optional[int] = None) -> None:
        self.query = query
        self.cursor = len(query) if cursor is None else max(0, min(cursor, len(query)))
        self.refilter()

    def insert(self, text: str, pasted: bool = False) -> None:
        cleaned = text.replace("\n", " ").replace("\r", " ") if pasted else text
        cleaned = "".join(char for char in cleaned if char >= " " and char != "\x7f")
        if not cleaned:
            return
        self.set_query(
            self.query[: self.cursor] + cleaned + self.query[self.cursor :],
            self.cursor + len(cleaned),
        )

    def backspace(self) -> None:
        if self.cursor <= 0:
            return
        self.set_query(
            self.query[: self.cursor - 1] + self.query[self.cursor :],
            self.cursor - 1,
        )

    def delete_forward(self) -> None:
        if self.cursor >= len(self.query):
            return
        self.set_query(self.query[: self.cursor] + self.query[self.cursor + 1 :], self.cursor)

    def delete_word(self) -> None:
        """Backwards by one path segment, which is the unit a path is edited in."""
        if self.cursor <= 0:
            return
        head = self.query[: self.cursor].rstrip()
        head = head[: -1] if head.endswith(os.sep) else head
        cut = max(head.rfind(os.sep), head.rfind(" "))
        cut = cut + 1 if cut >= 0 else 0
        self.set_query(self.query[:cut] + self.query[self.cursor :], cut)

    def accept_completion(self) -> bool:
        """Fill the input with the highlighted folder, ready to go one level deeper."""
        row = self.selected_row()
        if row is None:
            return False
        text = paths.accept_text(self.query, row.suggestion)
        if text == self.query:
            return False
        self.set_query(text)
        return True

    # -- movement -----------------------------------------------------------

    def move(self, delta: int, wrap: bool = True) -> None:
        if not self.rows:
            return
        total = len(self.rows)
        target = self.selected + delta
        self.selected = target % total if wrap else max(0, min(total - 1, target))
        self._sync_agent()
        self._dirty = True

    def cycle_agent(self, delta: int) -> None:
        if len(self.chips) < 2:
            return
        self.chip = (self.chip + delta) % len(self.chips)
        self._chip_pinned = True
        self._dirty = True

    # -- keys ---------------------------------------------------------------

    def handle_key(self, name: str) -> Optional[str]:
        if name in ("esc", "ctrl+c", "ctrl+q"):
            return CANCEL
        if name == "enter":
            return CONFIRM

        if name in ("up", "ctrl+p"):
            self.move(-1)
        elif name in ("down", "ctrl+n"):
            self.move(1)
        elif name == "pgup":
            self.move(-self._page(), wrap=False)
        elif name == "pgdn":
            self.move(self._page(), wrap=False)
        elif name == "tab":
            # Shell habit first: complete while there is visibly something to
            # complete, and only once the path stands on its own move on to the
            # agent. The condition is the ghost rather than "is there a row",
            # so what Tab is about to do is always on screen before it happens.
            if self.ghost():
                self.accept_completion()
            else:
                self.cycle_agent(1)
        elif name == "shift+tab":
            self.cycle_agent(-1)
        elif name in ("right", "ctrl+f"):
            if self.cursor >= len(self.query):
                if not self.accept_completion():
                    self.flash("nothing to complete")
            else:
                self.cursor += 1
                self._dirty = True
        elif name in ("left", "ctrl+b"):
            self.cursor = max(0, self.cursor - 1)
            self._dirty = True
        elif name in ("home", "ctrl+a"):
            self.cursor = 0
            self._dirty = True
        elif name in ("end", "ctrl+e"):
            self.cursor = len(self.query)
            self._dirty = True
        elif name == "backspace":
            self.backspace()
        elif name == "delete":
            self.delete_forward()
        elif name in ("ctrl+w", "alt+backspace"):
            self.delete_word()
        elif name == "ctrl+u":
            self.set_query("")
        elif name == "ctrl+k":
            self.set_query(self.query[: self.cursor], self.cursor)
        elif name == "ctrl+r":
            paths.clear_cache()
            self.refilter(keep=self.folder())
            self.flash("rescanned")
        return None

    def _page(self) -> int:
        return max(1, self._list_height - 1)

    def handle_mouse(self, event: Event) -> Optional[str]:
        if event.value == WHEEL_UP:
            self.move(-3, wrap=False)
            return None
        if event.value == WHEEL_DOWN:
            self.move(3, wrap=False)
            return None
        if event.value != PRESS:
            return None

        row = event.y - 1
        if row == self._chip_row:
            for index, (start, end) in enumerate(self._chip_spans):
                if start <= event.x - 1 < end:
                    self.chip = index
                    self._chip_pinned = True
                    self._dirty = True
                    return None
            return None

        index = self.offset + (row - self._row_top)
        if self._row_top <= row and 0 <= index < len(self.rows):
            already = index == self.selected
            self.selected = index
            self._sync_agent()
            self._dirty = True
            # A second click on the row you are already on is the commit, so a
            # single click can still be used to look before you leap.
            return CONFIRM if already else None
        return None

    # -- drawing ------------------------------------------------------------

    def invalidate(self) -> None:
        self._painter.reset()
        self._dirty = True

    def _scroll_into_view(self, height: int) -> None:
        if self.selected < self.offset:
            self.offset = self.selected
        elif self.selected >= self.offset + height:
            self.offset = self.selected - height + 1
        self.offset = max(0, min(self.offset, max(0, len(self.rows) - height)))

    def _list_rows(self, layout: render.Layout) -> List[str]:
        width = layout.inner_width
        height = layout.list_height
        self._scroll_into_view(height)
        if not self.rows:
            return [render.render_empty(self.theme, width, self.query)] + [""] * (height - 1)

        bar = render.scrollbar_column(len(self.rows), height, self.offset, height)
        body_width = width - (2 if bar else 0)
        visible = self.rows[self.offset : self.offset + height]
        name_room = render.name_column(visible, body_width)
        out: List[str] = []
        for line in range(height):
            index = self.offset + line
            if index >= len(self.rows):
                out.append("")
                continue
            text = render.render_row(
                self.theme,
                self.rows[index],
                body_width,
                index == self.selected,
                name_room,
            )
            if bar:
                text = render.pad_row(text, body_width) + " " + self.theme.paint("dim", bar[line])
            out.append(text)
        return out

    def _hints(self) -> List[Tuple[str, str]]:
        if self.ghost():
            return [
                ("⇥", "complete"),
                ("↑↓", "folder"),
                ("⇧⇥", "agent"),
                ("⏎", "create"),
                ("esc", ""),
            ]
        return [
            ("⇥", "agent"),
            ("↑↓", "folder"),
            ("→", "complete"),
            ("⏎", "create"),
            ("esc", ""),
        ]

    def frame(self, width: int, height: int) -> List[str]:
        """The whole screen as composed rows -- no terminal, no escape plumbing.

        Kept separate from ``draw`` so a frame can be rendered and asserted on
        without a pty, which is what the tests and the demo script both do.
        """
        layout = render.compute_layout(width, height)
        self._list_height = layout.list_height

        pad = " "
        lines: List[str] = [pad + render.render_input(
            self.theme, layout.inner_width, self.query, self.cursor, self.ghost()
        )]
        if layout.rules:
            lines.append(pad + render.render_rule(self.theme, layout.inner_width))

        self._row_top = len(lines)
        lines.extend(pad + row for row in self._list_rows(layout))

        if layout.rules:
            lines.append(pad + render.render_rule(self.theme, layout.inner_width))

        self._chip_row = -1
        if layout.chips:
            self._chip_row = len(lines)
            self._chip_spans = self._measure_chips()
            lines.append(pad + render.render_chips(
                self.theme, layout.inner_width, self.chips, self.chip
            ))
        if layout.target:
            lines.append(pad + render.render_target(
                self.theme, layout.inner_width, self.target(), self.agent(), self.error()
            ))
        if layout.footer:
            counter = "%d/%d" % (self.selected + 1, len(self.rows)) if self.rows else ""
            lines.append(pad + render.render_footer(
                self.theme, layout.inner_width, self._hints(), counter
            ))
        return lines

    def draw(self, terminal: Terminal) -> None:
        width, height = terminal.size()
        terminal.write(self._painter.frame(self.frame(width, height), width, height))
        terminal.flush()
        self._dirty = False

    def _measure_chips(self) -> List[Tuple[int, int]]:
        """Column spans of each chip, so a click can land on one."""
        spans: List[Tuple[int, int]] = []
        column = 1 + len("  agent  ")  # the leading pad column, then the label
        for index, chip in enumerate(self.chips):
            if index:
                column += 3  # " · "
            spans.append((column, column + len(chip.label)))
            column += len(chip.label)
        return spans

    # -- loop ---------------------------------------------------------------

    def run(self, terminal: Terminal) -> Optional[Choice]:
        decoder = KeyDecoder()
        self.draw(terminal)
        escape_deadline: Optional[float] = None

        while True:
            timeout = self._next_timeout(escape_deadline)
            if terminal.wait(timeout):
                data = terminal.read()
                if not data:
                    return None
                outcome = self._consume(decoder.feed(data))
                escape_deadline = (
                    time.monotonic() + ESC_TIMEOUT if decoder.pending_escape() else None
                )
                if outcome == CANCEL:
                    return None
                if outcome == CONFIRM:
                    choice = self.choice()
                    if choice is not None:
                        return choice
            elif escape_deadline is not None and time.monotonic() >= escape_deadline:
                escape_deadline = None
                if self._consume(decoder.flush()) == CANCEL:
                    return None

            if terminal.take_resize():
                self.invalidate()
            if self.status and time.monotonic() >= self.status_until:
                self.status = None
                self._dirty = True
            if self._dirty:
                self.draw(terminal)

    def choice(self) -> Optional[Choice]:
        folder = self.folder()
        if not folder:
            self.flash("type a folder first")
            return None
        if not paths.is_directory(folder):
            self.flash("%s is not a folder" % paths.display(folder))
            return None
        target = resolve_target(folder, self.snapshot, self.config.create)
        return Choice(normalize(folder), self.agent(), target)

    def _consume(self, events: Sequence[Event]) -> Optional[str]:
        outcome: Optional[str] = None
        for event in events:
            if event.kind == KEY:
                outcome = self.handle_key(event.value) or outcome
            elif event.kind == TEXT:
                self.insert(event.value)
            elif event.kind == PASTE:
                self.insert(event.value, pasted=True)
            elif event.kind == MOUSE:
                outcome = self.handle_mouse(event) or outcome
            if outcome in (CANCEL, CONFIRM):
                return outcome
        return outcome

    def _next_timeout(self, escape_deadline: Optional[float]) -> float:
        deadlines: List[float] = []
        if escape_deadline is not None:
            deadlines.append(escape_deadline)
        if self.status:
            deadlines.append(self.status_until)
        if not deadlines:
            return 30.0
        return max(0.01, min(deadlines) - time.monotonic())
