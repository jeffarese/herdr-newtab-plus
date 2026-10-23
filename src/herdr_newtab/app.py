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
from .agents import Agent, can_skip_permissions, initial_index
from .config import Config
from .keys import KEY, MOUSE, PASTE, PRESS, TEXT, WHEEL_DOWN, WHEEL_UP, Event, KeyDecoder
from .paint import Painter
from .session import (
    Target,
    folder_agents,
    folder_statuses,
    next_mode,
    normalize,
    resolve_target,
    resolved,
    space_folders,
)
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
    skip_permissions: bool = False


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
        # Where Enter lands: the automatic decision, or one of the two overrides.
        # Unlike the agent this is one setting for the whole picker, and it
        # outlives the popup -- the toggle is how you say you would rather have
        # tabs than spaces, and saying it once should be enough.
        self.create_mode = recents.create_mode() or config.create
        # Deliberately not remembered per folder the way the agent is. Starting
        # an agent without its permission prompts is a decision about this run,
        # and a folder that quietly kept the setting for weeks would be one you
        # could only discover by reading the chip row.
        self.skip_permissions = False

        # The panel takes the whole popup while it is up: at 12 rows there is no
        # room to explain anything beside the thing being explained.
        self.help = False

        # What the rows say about each folder beyond its name: the agent it was
        # last opened with, and the status of any agent standing in it now.
        # Both are fixed for the life of the popup -- the snapshot is taken once
        # and the recents only change on the way out -- so they are looked up
        # once rather than per row per frame.
        self._row_agents: Dict[str, str] = (
            recents.agents() if config.default_agent == "last" else {}
        )
        self._statuses: Dict[str, str] = folder_statuses(snapshot)
        self._agents_in = folder_agents(snapshot)
        self._spaces = space_folders(snapshot)

        # The preview follows herdr-bar's rules: on unless the config says
        # false, shown by itself only on a popup wide enough to spare the
        # column, and once ^O has been pressed, shown or hidden as asked
        # wherever it fits at all.
        self.preview_enabled = config.preview is not False
        self.preview_chosen = False
        self._preview_showing = False
        self._list_right = 0

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

    def can_skip(self) -> bool:
        """Whether the selected agent has a known way to skip its prompts."""
        return can_skip_permissions(self.agent().kind, self.config.skip_permission_flags)

    def skipping(self) -> bool:
        """Whether Enter would actually pass the skip-permissions flags.

        The toggle is one piece of state for the whole picker, but it only means
        anything on a chip that can honour it. Reading it through here is what
        lets the row be cycled without the setting being silently lost, while
        never claiming a shell or an agent without a flag is being started
        unprompted.
        """
        return self.skip_permissions and self.can_skip()

    def toggle_skip_permissions(self) -> None:
        if self.skip_permissions:
            self.skip_permissions = False
            self._dirty = True
            return
        if not self.can_skip():
            agent = self.agent()
            self.flash(
                "no skip-permissions flag for %s"
                % (agent.label if not agent.is_none else "a plain shell")
            )
            return
        self.skip_permissions = True
        self._dirty = True

    def ghost(self) -> str:
        row = self.selected_row()
        return paths.completion_for(self.query, row.suggestion if row else None)

    def target(self) -> Optional[Target]:
        folder = self.folder()
        if not folder:
            return None
        return resolve_target(folder, self.snapshot, self.create_mode)

    def cycle_create_mode(self) -> None:
        """Move Enter along: decide for me, always a tab, always a space.

        Three positions rather than two because the automatic decision is the
        one worth being able to get back to, and a two-way switch would strand
        you on an override with no way back that is not a config file.
        """
        self.create_mode = next_mode(self.create_mode)
        self.recents.remember_create_mode(self.create_mode)
        self._dirty = True

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

    def toggle_help(self) -> None:
        self.help = not self.help
        self.status = None
        self._dirty = True

    def insert(self, text: str, pasted: bool = False) -> None:
        if self.help:
            self.toggle_help()
            return
        # A path can legitimately contain a question mark, so `?` only asks for
        # the panel where it cannot be something you meant to type. F1 is the
        # spelling that works whatever is in the input.
        if text == "?" and not pasted and not self.query:
            self.toggle_help()
            return
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
        if name in ("ctrl+c", "ctrl+q"):
            return CANCEL
        if self.help:
            # Anything at all closes it, which is the behaviour a panel you
            # opened by accident wants. Enter included: creating a tab from
            # behind the help is not what the key was pressed for.
            self.toggle_help()
            return None
        if name == "esc":
            return CANCEL
        if name == "f1":
            self.toggle_help()
            return None
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
        elif name == "ctrl+y":
            self.toggle_skip_permissions()
        elif name == "ctrl+t":
            self.cycle_create_mode()
        elif name == "ctrl+o":
            self.toggle_preview()
        elif name == "ctrl+r":
            paths.clear_cache()
            self.refilter(keep=self.folder())
            self.flash("rescanned")
        return None

    def toggle_preview(self) -> None:
        """Show the preview if it is hidden, hide it if it is showing.

        Measured against what is on screen rather than against the setting, so
        a preview "auto" kept hidden on a narrow popup comes up on the first
        press instead of the second.
        """
        self.preview_enabled = not self._preview_showing
        self.preview_chosen = True
        self._dirty = True

    def preview_fits(self, inner: int) -> bool:
        if not self.preview_enabled:
            return False
        if self.preview_chosen or self.config.preview is True:
            return inner >= render.PREVIEW_FITS
        return inner >= render.PREVIEW_AUTO

    def preview(self) -> Optional[render.Preview]:
        """What the preview says about the folder Enter would use."""
        folder = self.folder()
        if not folder:
            return None
        listed = paths.contents(folder)
        space = self._spaces.get(folder) or self._spaces.get(resolved(folder))
        return render.Preview(
            name=paths.folder_name(folder),
            path=paths.display(folder),
            contents=listed,
            branch=paths.git_branch(folder, listed) if listed is not None else None,
            space=space.label if space else None,
            agents=self._agents_in.get(folder) or self._agents_in.get(resolved(folder)) or (),
            hidden=self.config.hidden,
        )

    def _page(self) -> int:
        return max(1, self._list_height - 1)

    def handle_mouse(self, event: Event) -> Optional[str]:
        if self.help:
            if event.value == PRESS:
                self.toggle_help()
            return None
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

        # The list's own rows only: not the air under a short list, and not the
        # preview beside it, which shows a folder rather than offering one.
        line = row - self._row_top
        if not 0 <= line < self._list_height or event.x - 1 >= self._list_right:
            return None
        index = self.offset + line
        if 0 <= index < len(self.rows):
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
        width = layout.list_width or layout.inner_width
        height = layout.list_height
        self._scroll_into_view(height)
        if not self.rows:
            return render.render_empty(self.theme, width, height, self.query)

        bar = render.scrollbar_column(len(self.rows), height, self.offset, height)
        # Beside the preview the separator is the scrollbar's track, so the list
        # keeps its full width and there is one vertical line rather than two.
        beside = bool(layout.preview_width)
        body_width = width - (2 if bar and not beside else 0)
        visible = self.rows[self.offset : self.offset + height]
        name_room = render.name_column(visible, body_width)
        out: List[str] = []
        for line in range(height):
            index = self.offset + line
            if index >= len(self.rows):
                out.append("")
                continue
            path = self.rows[index].suggestion.path
            text = render.render_row(
                self.theme,
                self.rows[index],
                body_width,
                index == self.selected,
                name_room,
                self._row_agents.get(path),
                self._statuses.get(path),
            )
            if bar and not beside:
                thumb = bar[line] == render.SCROLL_THUMB
                text = (
                    render.pad_row(text, body_width)
                    + " "
                    + self.theme.paint("muted" if thumb else "rule", bar[line])
                )
            out.append(text)
        return out

    def _body_rows(self, layout: render.Layout) -> List[str]:
        """The list, and the preview beside it when there is one, row by row."""
        rows = self._list_rows(layout)
        if not layout.preview_width:
            return rows + [""] * layout.spare
        height = layout.list_height + layout.spare
        preview = render.render_preview(
            self.theme, layout.preview_width, height, self.preview()
        )
        bar = (
            render.scrollbar_column(
                len(self.rows), layout.list_height, self.offset, layout.list_height
            )
            if self.rows
            else {}
        )
        out = []
        for index in range(height):
            left = rows[index] if index < len(rows) else ""
            thumb = bar.get(index) == render.SCROLL_THUMB
            separator = (
                " "
                + self.theme.paint(
                    "muted" if thumb else "rule",
                    render.SCROLL_THUMB if thumb else render.SEPARATOR.strip(),
                )
                + " "
            )
            out.append(render.pad_row(left, layout.list_width) + separator + preview[index])
        return out

    def _hints(self) -> List[Tuple[str, str]]:
        # Offered exactly where it would do something. On a chip that cannot
        # honour it the hint goes away along with the chip's mark and the target
        # line's note, so nothing on screen claims a skip that is not going to
        # happen -- and the setting itself survives, ready for the chip it came
        # from.
        skip: List[Tuple[str, str]] = []
        if self.can_skip():
            skip = [("^Y", "prompts off" if self.skipping() else "skip perms")]
        # Named by the position it is in rather than by the one it is going to,
        # so the footer and the target line's badge always say the same thing.
        mode = [("^T", self.create_mode)]
        # ⇥ says what it would do right now rather than everything it can do, so
        # the pair of keys the input needs is always readable off one row. The
        # keys with nothing to report -- ^W, ^U, ^R, click -- stay in the panel.
        completion = (
            [("⇥", "complete"), ("⇧⇥", "agent")]
            if self.ghost()
            else [("⇥", "agent"), ("→", "complete")]
        )
        return completion + [("↑↓", "folder")] + mode + skip + [("⏎", "create"), ("esc", "close")]

    def _guide(self) -> str:
        """The corner that opens the panel -- while `?` is what it would do.

        Once something is typed a question mark belongs to the path, so the
        corner says the key that still works rather than one that no longer does.
        """
        return "? help" if not self.query else "F1 help"

    def frame(self, width: int, height: int) -> List[str]:
        """The whole screen as composed rows -- no terminal, no escape plumbing.

        Kept separate from ``draw`` so a frame can be rendered and asserted on
        without a pty, which is what the tests and the demo script both do.
        """
        pad = " "
        if self.help:
            self._chip_row = -1  # nothing on screen is clickable while it is up
            self._row_top = height
            return [
                pad + row
                for row in render.render_help(self.theme, width - 2, height, self.create_mode)
            ]

        layout = render.compute_layout(width, height, self.preview_fits(width - 2))
        self._preview_showing = bool(layout.preview_width)
        self._list_height = layout.list_height
        self._list_right = len(pad) + (layout.list_width or layout.inner_width)
        # Where the separator meets the rules above and below the list, so the
        # two panes read as one box divided rather than two drawn side by side.
        junction = layout.list_width + 1 if layout.preview_width else -1

        lines: List[str] = [pad + render.render_input(
            self.theme, layout.inner_width, self.query, self.cursor, self.ghost()
        )]
        if layout.rules:
            lines.append(pad + render.render_rule(self.theme, layout.inner_width, junction, "┬"))

        self._row_top = len(lines)
        # Rows the list was not allowed to take sit under it rather than under
        # the chrome, so the input stays at the top of the popup and the hints
        # stay on its floor, which is where the eye already looks for them.
        lines.extend(pad + row for row in self._body_rows(layout))

        if layout.rules:
            lines.append(pad + render.render_rule(self.theme, layout.inner_width, junction, "┴"))

        self._chip_row = -1
        if layout.chips:
            self._chip_row = len(lines)
            row, spans = render.chips_row(
                self.theme, layout.inner_width, self.chips, self.chip, self.skipping()
            )
            # Shifted by the pad column, so a span is in the same columns the
            # mouse reports (less one, as handle_mouse counts them).
            self._chip_spans = [
                (start + len(pad), end + len(pad)) if start >= 0 else (start, end)
                for start, end in spans
            ]
            lines.append(pad + row)
        if layout.target:
            lines.append(pad + render.render_target(
                self.theme,
                layout.inner_width,
                self.target(),
                self.agent(),
                self.error(),
                self.skipping(),
                self.create_mode,
            ))
        if layout.note:
            lines.append(pad + render.render_mode_note(
                self.theme, layout.inner_width, self.create_mode
            ))
        if layout.footer:
            counter = "%d/%d" % (self.selected + 1, len(self.rows)) if self.rows else ""
            lines.append(pad + render.render_footer(
                self.theme, layout.inner_width, self._hints(), counter, self._guide()
            ))
        return lines

    def draw(self, terminal: Terminal) -> None:
        width, height = terminal.size()
        terminal.write(self._painter.frame(self.frame(width, height), width, height))
        terminal.flush()
        self._dirty = False

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
        # Asked afresh: the frame's answer may be a listing read a while ago,
        # and this one is about to have a tab made in it.
        if not paths.is_directory(folder, fresh=True):
            self.flash("%s is not a folder" % paths.display(folder))
            return None
        target = resolve_target(folder, self.snapshot, self.create_mode)
        return Choice(normalize(folder), self.agent(), target, self.skipping())

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
