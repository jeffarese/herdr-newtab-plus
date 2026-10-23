//! Compose the picker's rows.
//!
//! Everything is drawn as full rows into the popup's inner area: Herdr
//! already draws the popup border, so the picker stays borderless and
//! full-bleed. Rows are composed as (role, text) segments, measured in display
//! columns, and only then turned into escape sequences -- which keeps
//! truncation honest for CJK and emoji, and makes the layout testable by
//! stripping the colors back out.
//!
//! Getting those rows onto the screen without resending the ones that did not
//! move is `paint`'s job, not this module's.

use crate::agents::{Agent, SKIP_MARK};
use crate::icons::GAP;
use crate::paths::{parent_display, Contents, Scored, SOURCE_PATH, SOURCE_RECENT, SOURCE_SPACE, SOURCE_ZOXIDE};
use crate::session::{FolderAgent, Target, MODES, SMART, SPACE, TAB};
use crate::textutil::{display_width, pad, truncate, truncate_middle, wrap};
use crate::theme::{status_glyph, Theme, BOLD, RESET, UNDERLINE};
use std::rc::Rc;

pub const PROMPT: &str = "❯";
pub const SELECTED_BAR: &str = "▌";
pub const SCROLL_TRACK: &str = "│";
pub const SCROLL_THUMB: &str = "┃";
pub const ARROW: &str = "→";
pub const META_SEP: &str = " · ";

pub const PLACEHOLDER: &str = "folder — type a name, or a path to complete it";

pub const MIN_NAME: isize = 12;
pub const MAX_NAME: isize = 34;

/// The popup is sized for the help panel, which is a page of prose; the
/// picker wants a short list whatever it is given.
pub const MAX_LIST: isize = 10;

// The preview column, as herdr-bar sizes its own: a third of the popup, within
// bounds, and only when the list keeps enough to show a row whole beside it.
pub const PREVIEW_MIN: isize = 28;
pub const PREVIEW_MAX: isize = 56;
pub const MIN_LIST: isize = 46;
pub const SEPARATOR: &str = " │ ";
const SEPARATOR_LEN: isize = 3;
/// Below this many rows there is no room for the folder's contents under its
/// name and path.
pub const PREVIEW_ROWS: isize = 5;
/// How wide the popup has to be before the preview shows by itself. Asked for
/// -- with ^O, or `"preview": true` -- it only needs the room to fit.
pub const PREVIEW_AUTO: isize = 92;
pub const PREVIEW_FITS: isize = PREVIEW_MIN + MIN_LIST + SEPARATOR_LEN;

/// Python's `//`: floor division.
fn floordiv(a: isize, b: isize) -> isize {
    a.div_euclid(b)
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// `text[start:stop]` by characters.
fn slice(text: &str, start: usize, stop: usize) -> &str {
    let mut indices = text.char_indices().map(|(i, _)| i).chain(std::iter::once(text.len()));
    let begin = indices.clone().nth(start).unwrap_or(text.len());
    let end = indices.nth(stop).unwrap_or(text.len());
    if begin >= end {
        ""
    } else {
        &text[begin..end]
    }
}

fn slice_from(text: &str, start: usize) -> &str {
    let begin = text.char_indices().nth(start).map(|(i, _)| i).unwrap_or(text.len());
    &text[begin..]
}

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub inner_width: isize,
    pub list_top: isize,
    pub list_height: isize,
    pub rules: bool,
    pub chips: bool,
    pub target: bool,
    pub footer: bool,
    pub note: bool,
    pub spare: isize,
    /// The whole inner width when there is no preview.
    pub list_width: isize,
    pub preview_width: isize,
}

/// Give the list the rows the chrome does not need, up to a cap.
///
/// The chrome sheds from the bottom up as the popup shrinks: the key hints go
/// first, then the line that says what Enter will do, then the agent row. The
/// input and at least one suggestion always survive.
///
/// Past the cap the rows go to the note that spells the create mode out, and
/// then to air between the list and the chrome. The preview, when asked for
/// and when it fits, takes a column off the right of the list and every row
/// the list has, air included.
pub fn compute_layout(width: isize, height: isize, preview: bool) -> Layout {
    let inner = (width - 2).max(1); // one column of padding on each side

    let footer = height >= 9;
    let target = height >= 7;
    let chips = height >= 5;
    let rules = height >= 8;

    let list_top = if rules { 2 } else { 1 };
    let mut chrome = list_top;
    chrome += rules as isize; // the rule below the list
    chrome += chips as isize;
    chrome += target as isize;
    chrome += footer as isize;

    let mut room = (height - chrome).max(1);
    let note = target && room > MAX_LIST + 1;
    if note {
        room -= 1;
    }

    let (mut list_width, mut preview_width) = (inner, 0);
    if preview && inner >= PREVIEW_FITS && room >= PREVIEW_ROWS {
        preview_width = PREVIEW_MAX.min(PREVIEW_MIN.max(floordiv(inner, 3)));
        list_width = inner - preview_width - SEPARATOR_LEN;
    }
    Layout {
        inner_width: inner,
        list_top,
        list_height: room.min(MAX_LIST),
        rules,
        chips,
        target,
        footer,
        note,
        spare: (room - MAX_LIST).max(0),
        list_width,
        preview_width,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment {
    pub role: String,
    pub text: String,
    pub bold: bool,
    pub underline: bool,
}

impl Segment {
    pub fn new(role: &str, text: impl Into<String>) -> Segment {
        Segment { role: role.to_string(), text: text.into(), bold: false, underline: false }
    }

    pub fn bold(role: &str, text: impl Into<String>, bold: bool) -> Segment {
        Segment { role: role.to_string(), text: text.into(), bold, underline: false }
    }

    pub fn styled(role: &str, text: impl Into<String>, bold: bool, underline: bool) -> Segment {
        Segment { role: role.to_string(), text: text.into(), bold, underline }
    }

    fn with_text(&self, text: String) -> Segment {
        Segment { role: self.role.clone(), text, bold: self.bold, underline: self.underline }
    }
}

pub fn text_of(theme: &Theme, segments: &[Segment], background: &str) -> String {
    let mut out = String::new();
    for segment in segments {
        if segment.text.is_empty() {
            continue;
        }
        let color = if segment.role.is_empty() { "" } else { theme.fg(&segment.role) };
        if color.is_empty() && !segment.bold && !segment.underline {
            out.push_str(&segment.text);
            continue;
        }
        if segment.bold {
            out.push_str(BOLD);
        }
        if segment.underline {
            out.push_str(UNDERLINE);
        }
        out.push_str(color);
        out.push_str(&segment.text);
        out.push_str(RESET);
        out.push_str(background);
    }
    out
}

fn text(theme: &Theme, segments: &[Segment]) -> String {
    text_of(theme, segments, "")
}

fn width_of(segments: &[Segment]) -> isize {
    segments.iter().map(|segment| display_width(&segment.text) as isize).sum()
}

/// Split text so matched characters can be lit differently from the rest.
fn highlight(text: &str, positions: &[usize], base_role: &str, bold: bool) -> Vec<Segment> {
    let length = char_len(text);
    let marks: Vec<usize> = positions.iter().copied().filter(|&p| p < length).collect();
    if marks.is_empty() {
        return vec![Segment::bold(base_role, text, bold)];
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut run = String::new();
    let mut run_marked = false;
    // Matched characters are usually one or two letters adrift in a longer
    // name, so color alone reads as noise: underline them too and the
    // subsequence is legible at a glance.
    let flush = |segments: &mut Vec<Segment>, run: &str, marked: bool| {
        if run.is_empty() {
            return;
        }
        segments.push(Segment::styled(if marked { "match" } else { base_role }, run, bold || marked, marked));
    };
    for (index, c) in text.chars().enumerate() {
        let marked = marks.contains(&index);
        if marked != run_marked && !run.is_empty() {
            flush(&mut segments, &run, run_marked);
            run.clear();
        }
        run_marked = marked;
        run.push(c);
    }
    flush(&mut segments, &run, run_marked);
    segments
}

// -- input -----------------------------------------------------------------

/// Slide a long path so the cursor stays on screen, keeping the tail visible.
fn window(text: &str, cursor: usize, width: isize) -> (String, usize) {
    if display_width(text) as isize <= width {
        return (text.to_string(), cursor);
    }
    let mut start = 0;
    while (display_width(slice(text, start, cursor)) as isize) > width - 1 && start < cursor {
        start += 1;
    }
    (slice_from(text, start).to_string(), cursor - start)
}

/// The prompt line: what is typed, the block cursor, and the inline completion.
///
/// The completion is drawn after the cursor in the ghost role rather than
/// inserted into the query, so it is always obvious which characters are
/// yours and which are the picker's guess.
pub fn render_input(theme: &Theme, width: isize, query: &str, cursor: usize, ghost: &str) -> String {
    let field = (width - 2).max(4);
    let prompt = text(theme, &[Segment::bold("accent", format!("{} ", PROMPT), true)]);
    if query.is_empty() {
        let hint = truncate(PLACEHOLDER, (field - 2).max(0));
        return prompt + "\x1b[7m \x1b[27m" + &text(theme, &[Segment::new("dim", hint)]);
    }

    let (visible, position) = window(query, cursor.min(char_len(query)), field - display_width(ghost) as isize);
    let visible_len = char_len(&visible);
    let position = position.min(visible_len);
    let before = slice(&visible, 0, position);
    let (under, after, tail) = if position < visible_len {
        (slice(&visible, position, position + 1).to_string(), slice_from(&visible, position + 1), ghost)
    } else {
        let under = if ghost.is_empty() { " ".to_string() } else { slice(ghost, 0, 1).to_string() };
        (under, "", slice_from(ghost, 1))
    };

    prompt
        + &text(theme, &[Segment::new("text", before)])
        + "\x1b[7m"
        + &under
        + "\x1b[27m"
        + &text(theme, &[Segment::new("text", after)])
        + &text(theme, &[Segment::new("ghost", truncate(tail, (field - display_width(&visible) as isize).max(0)))])
}

// -- suggestion rows -------------------------------------------------------

fn badge(row: &Scored) -> String {
    let suggestion = &row.suggestion;
    if suggestion.source == SOURCE_SPACE {
        return if suggestion.detail.is_empty() { "space".to_string() } else { suggestion.detail.clone() };
    }
    let word = match suggestion.source {
        SOURCE_RECENT => "recent",
        SOURCE_ZOXIDE => "frecent",
        SOURCE_PATH => "",
        _ => "",
    };
    if !word.is_empty() {
        return word.to_string();
    }
    if suggestion.is_git {
        "git".to_string()
    } else {
        String::new()
    }
}

/// How wide the name column has to be for the rows currently on screen.
///
/// Sized to what is actually visible rather than to a fixed share of the
/// popup, so a list of short names does not leave a corridor of empty space
/// before the paths, and scrolling into longer ones widens it instead of
/// truncating them.
pub fn name_column(rows: &[Scored], width: isize) -> isize {
    let longest = rows.iter().map(|row| display_width(&row.suggestion.name) as isize).max().unwrap_or(0);
    MIN_NAME.max(MAX_NAME.min(longest).min(MIN_NAME.max(floordiv(width, 2))))
}

/// The right-hand end of a row: who is already there, and where it came from.
///
/// An agent standing in the folder leads with its status glyph, in the colours
/// herdr's sidebar uses for it. The folder's own agent -- the one the chips
/// will move to when the row is selected -- is named here only when there is
/// no logo at the front of the row to say it instead.
fn meta(theme: &Theme, row: &Scored, agent: Option<&str>, status: Option<&str>) -> Vec<Segment> {
    let mut pieces: Vec<Vec<Segment>> = Vec::new();
    if let Some(agent) = agent.filter(|a| !a.is_empty()) {
        if theme.logo(Some(agent)).is_empty() {
            pieces.push(vec![Segment::new(&theme.agent_role(Some(agent)), agent)]);
        }
    }
    let badge = badge(row);
    let status = status.unwrap_or("");
    let glyph = status_glyph(status);
    let mut place: Vec<Segment> = Vec::new();
    if !glyph.is_empty() {
        place.push(Segment::new(status, format!("{}{}", glyph, if badge.is_empty() { "" } else { " " })));
    }
    if !badge.is_empty() {
        place.push(Segment::new(if row.suggestion.source == SOURCE_SPACE { "space" } else { "dim" }, badge));
    }
    if !place.is_empty() {
        pieces.push(place);
    }
    let mut out: Vec<Segment> = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if index > 0 {
            out.push(Segment::new("rule", META_SEP));
        }
        out.extend(piece);
    }
    out
}

/// One folder: its name, where it lives, and what already knows about it.
///
/// `agent` is the agent this folder was last opened with, and `status` the
/// most urgent status of any agent standing in it right now. With the icon
/// font the agent's logo leads the row, in a gutter every row keeps whether it
/// has a logo or not, so the names stay in one column as the list scrolls.
pub fn render_row(
    theme: &Theme,
    row: &Scored,
    width: isize,
    selected: bool,
    name_room: Option<isize>,
    agent: Option<&str>,
    status: Option<&str>,
) -> String {
    let background = if selected { theme.selection_bg.as_str() } else { "" };

    let marker =
        if selected { Segment::bold("accent", format!("{} ", SELECTED_BAR), true) } else { Segment::new("", "  ") };
    let mut segments: Vec<Segment> = vec![marker];
    if theme.agent_icons {
        let logo = theme.logo(agent);
        segments.push(if !logo.is_empty() {
            Segment::new(&theme.agent_role(agent), format!("{}{}", logo, GAP))
        } else {
            Segment::new("", format!(" {}", GAP))
        });
    }

    let meta = if width >= 44 { meta(theme, row, agent, status) } else { Vec::new() };
    let meta_width = if meta.is_empty() { 0 } else { width_of(&meta) + 2 };

    let lead = width_of(&segments);
    let name_room = name_room.unwrap_or_else(|| MIN_NAME.max(MAX_NAME.min(floordiv(width - lead - meta_width, 2))));
    let name = truncate(&row.suggestion.name, name_room);
    segments.extend(highlight(&name, &row.name_positions, "text", selected));
    segments.push(Segment::new("", " ".repeat((name_room - display_width(&name) as isize + 1).max(1) as usize)));

    let parent_room = (width - width_of(&segments) - meta_width).max(0);
    if parent_room >= 4 {
        let parent = truncate_middle(&parent_display(&row.suggestion.path), parent_room);
        segments.extend(highlight(&parent, &row.path_positions, "muted", false));
    }

    if meta_width > 0 {
        let used = width_of(&segments);
        segments.push(Segment::new("", " ".repeat((width - used - width_of(&meta)).max(1) as usize)));
        segments.extend(meta);
    }

    let segments = fit(&segments, width);
    let rendered = text_of(theme, &segments, background);
    if background.is_empty() {
        return rendered;
    }
    // A selected row is a band across the list, so it has to be filled to the
    // edge rather than only behind its own text.
    format!("{}{}{}{}", background, rendered, " ".repeat((width - width_of(&segments)).max(0) as usize), RESET)
}

/// What the list says when it has nothing to show, and what to do about it.
pub fn render_empty(theme: &Theme, width: isize, height: isize, query: &str) -> Vec<String> {
    let (headline, hint) = if !crate::pystr::strip(query).is_empty() {
        ("no folder matches", "⏎ still creates it, if the path exists")
    } else {
        ("no folders to suggest", "start with ~ / or . to complete real folders")
    };
    let room = (width - 2).max(0);
    let mut rows = vec![
        text(theme, &[Segment::new("", "  "), Segment::bold("text", truncate(headline, room), true)]),
        text(theme, &[Segment::new("", "  "), Segment::new("muted", truncate(hint, room))]),
    ];
    rows.extend(std::iter::repeat_n(String::new(), height.max(0) as usize));
    rows.truncate(height.max(1) as usize);
    rows
}

// -- agent chips -----------------------------------------------------------

pub const CHIP_LEAD: &str = "  agent ";
pub const CHIP_SEP: &str = " ";
/// Stands in the logo column for the plain shell, as in herdr-bar.
pub const SHELL_MARK: &str = "$";

/// The text of one chip, marked when it is about to skip its prompts.
///
/// Only the selected chip can carry the mark, because the setting only
/// applies to the agent Enter would start.
pub fn chip_label(chip: &Agent, selected: bool, skipping: bool) -> String {
    if selected && skipping && !chip.is_none() {
        return format!("{}{}", chip.label, SKIP_MARK);
    }
    chip.label.clone()
}

/// One chip, padded by a column on each side so a selected one reads as a pill.
///
/// The logo keeps its vendor's colour whether or not the chip is selected:
/// it is what the eye finds the agent by. The label is what changes.
fn chip(theme: &Theme, chip: &Agent, selected: bool, skipping: bool, pill: bool) -> Vec<Segment> {
    let label = chip_label(chip, selected, skipping);
    let warn = selected && skipping && !chip.is_none();
    let role = theme.agent_role(Some(&chip.kind));
    let mut out = vec![Segment::new("", " ")];
    if theme.agent_icons {
        let logo = if chip.is_none() { SHELL_MARK.to_string() } else { theme.logo(Some(&chip.kind)) };
        if !logo.is_empty() {
            let logo_role = if chip.is_none() {
                "muted"
            } else if warn {
                "warn"
            } else {
                role.as_str()
            };
            out.push(Segment::new(logo_role, format!("{}{}", logo, GAP)));
        }
    }
    if selected {
        let label_role = if chip.is_none() {
            "text"
        } else if warn {
            "warn"
        } else {
            role.as_str()
        };
        // Without a selection colour to draw the pill in, the underline is
        // what is left to say which chip it is.
        out.push(Segment::styled(label_role, label, true, !pill));
    } else {
        out.push(Segment::new("muted", label));
    }
    out.push(Segment::new("", " "));
    out
}

/// The agent row, and the columns each chip covers so a click can find it.
///
/// The spans come from the same pass that draws the row, so a chip widened by
/// its skip mark, or a row scrolled to keep the selection on screen, can
/// never put a click on the wrong agent. A chip scrolled out of view spans
/// nothing.
pub fn chips_row(
    theme: &Theme,
    width: isize,
    chips: &[Agent],
    selected: usize,
    skipping: bool,
) -> (String, Vec<(isize, isize)>) {
    let pill = !theme.selection_bg.is_empty();
    let cells: Vec<Vec<Segment>> =
        chips.iter().enumerate().map(|(index, c)| chip(theme, c, index == selected, skipping, pill)).collect();
    let widths: Vec<isize> = cells.iter().map(|cell| width_of(cell)).collect();
    let lead = display_width(CHIP_LEAD) as isize;
    let count = cells.len();
    let sep = CHIP_SEP.len() as isize;

    let needed = |head: usize, tail: usize| -> isize {
        let inner: isize = if head < tail { widths[head..tail].iter().sum() } else { 0 };
        let inner = inner + (tail as isize - head as isize - 1).max(0) * sep;
        lead + if head > 0 { 2 } else { 0 } + inner + if tail < count { 2 } else { 0 }
    };

    let (mut head, mut tail) = (0usize, count);
    if needed(0, count) > width {
        // Too many agents at once: scroll until the selection fits, never past
        // it, so the chip you are on is always one you can see.
        head = selected.saturating_sub(2);
        while head < selected && needed(head, selected + 1) > width {
            head += 1;
        }
        tail = selected + 1;
        while tail < count && needed(head, tail + 1) <= width {
            tail += 1;
        }
    }

    let mut spans: Vec<(isize, isize)> = vec![(-1, -1); count];
    let mut out = vec![text(theme, &[Segment::new("muted", CHIP_LEAD)])];
    let mut column = lead;
    if head > 0 {
        out.push(text(theme, &[Segment::new("dim", "‹ ")]));
        column += 2;
    }
    for index in head..tail.min(count) {
        if index > head {
            out.push(CHIP_SEP.to_string());
            column += sep;
        }
        if index == selected && pill {
            out.push(format!("{}{}{}", theme.selection_bg, text_of(theme, &cells[index], &theme.selection_bg), RESET));
        } else {
            out.push(text(theme, &cells[index]));
        }
        spans[index] = (column, column + widths[index]);
        column += widths[index];
    }
    if tail < count {
        out.push(text(theme, &[Segment::new("dim", " ›")]));
        column += 2;
    }

    if column > width {
        // Not even the selected chip fits: say what can be said, unpainted by
        // the pill, rather than run off the edge.
        let mut flat = vec![Segment::new("muted", CHIP_LEAD)];
        for cell in &cells[head.min(count)..tail.min(count)] {
            flat.extend(cell.iter().cloned());
        }
        return (text(theme, &fit(&flat, width)), vec![(-1, -1); count]);
    }
    (out.concat(), spans)
}

/// The agent row, always visible so the choice never has to be discovered.
#[cfg(test)]
pub fn render_chips(theme: &Theme, width: isize, chips: &[Agent], selected: usize, skipping: bool) -> String {
    chips_row(theme, width, chips, selected, skipping).0
}

// -- target and footer -----------------------------------------------------

/// All three create modes, with the one the picker is on drawn as a pill.
pub fn mode_switch(theme: &Theme, mode: &str) -> (String, isize) {
    let pill = theme.selection_bg.as_str();
    let mut out = String::new();
    for (index, name) in MODES.iter().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        if *name != mode {
            out.push_str(&text(theme, &[Segment::new("dim", format!(" {} ", name))]));
        } else if !pill.is_empty() {
            let label = text_of(theme, &[Segment::bold("accent", *name, true)], pill);
            out.push_str(&format!("{} {} {}", pill, label, RESET));
        } else {
            out.push_str(&format!(" {} ", text(theme, &[Segment::styled("accent", *name, true, true)])));
        }
    }
    let width: isize = MODES.iter().map(|name| name.len() as isize + 2).sum::<isize>() + MODES.len() as isize - 1;
    (out, width)
}

/// What Enter is about to do, spelled out before it happens.
///
/// The create mode sits at the right-hand end as a three-way switch, so the
/// sentence and the rule that produced it are read together. A popup too
/// narrow for the switch badges an overridden mode instead.
pub fn render_target(
    theme: &Theme,
    width: isize,
    target: Option<&Target>,
    agent: Option<&Agent>,
    error: &str,
    skipping: bool,
    mode: &str,
) -> String {
    let mut segments: Vec<Segment>;
    if !error.is_empty() {
        segments = vec![Segment::new("", "  "), Segment::new("error", error)];
    } else if let Some(target) = target {
        segments = vec![Segment::bold("accent", format!("  {} ", ARROW), true)];
        if target.kind == TAB {
            segments.push(Segment::new("text", "new tab in "));
            let label = target.workspace_label.as_deref().filter(|l| !l.is_empty()).unwrap_or("this space");
            segments.push(Segment::bold("space", label, true));
        } else {
            segments.push(Segment::new("text", "new space "));
            segments.push(Segment::bold("space", target.workspace_label.clone().unwrap_or_default(), true));
        }
        if let Some(agent) = agent.filter(|a| !a.is_none()) {
            let role = if skipping { "warn".to_string() } else { theme.agent_role(Some(&agent.kind)) };
            segments.push(Segment::new("rule", "  ·  "));
            let logo = theme.logo(Some(&agent.kind));
            if !logo.is_empty() {
                segments.push(Segment::new(&role, format!("{}{}", logo, GAP)));
            }
            segments.push(Segment::new(&role, agent.label.clone()));
            // Spelled out rather than left to the chip's mark, because this is
            // the line that says what Enter does.
            if skipping {
                segments.push(Segment::new("warn", " (skip permissions)"));
            }
        }
    } else {
        return String::new();
    }

    let (switch, switch_width) = mode_switch(theme, mode);
    if width_of(&segments) + 3 + switch_width <= width {
        let gap = width - width_of(&segments) - switch_width;
        return text(theme, &segments) + &" ".repeat(gap.max(0) as usize) + &switch;
    }
    if mode != SMART && error.is_empty() {
        segments.push(Segment::new("muted", format!("   [{}]", mode)));
    }
    text(theme, &fit(&segments, width))
}

pub fn mode_note(mode: &str) -> &'static str {
    match mode {
        SMART => "a tab when that folder already has a space open, a new space when it does not",
        TAB => "always a tab: that folder's own space when it has one, otherwise the one you are in",
        SPACE => "always a new space, even when that folder already has one",
        _ => "",
    }
}

/// The create mode in words, under the line that shows its answer.
pub fn render_mode_note(theme: &Theme, width: isize, mode: &str) -> String {
    text(
        theme,
        &fit(
            &[
                Segment::new("", "    "),
                Segment::new("accent", mode),
                Segment::new("rule", " — "),
                Segment::new("muted", mode_note(mode)),
            ],
            width,
        ),
    )
}

pub fn fit(segments: &[Segment], width: isize) -> Vec<Segment> {
    let mut out: Vec<Segment> = Vec::new();
    let mut used = 0isize;
    for segment in segments {
        let room = width - used;
        if room <= 0 {
            break;
        }
        let cut = truncate(&segment.text, room);
        used += display_width(&cut) as isize;
        out.push(segment.with_text(cut));
    }
    out
}

fn hint_row(groups: &[Vec<Segment>], gap: &str) -> Vec<Segment> {
    let mut segments = vec![Segment::new("", "  ")];
    for (index, group) in groups.iter().enumerate() {
        if index > 0 {
            segments.push(Segment::new("dim", gap));
        }
        segments.extend(group.iter().cloned());
    }
    segments
}

/// The key hints, tightened and then shed from the right until they fit.
///
/// Air goes before information: the gap between hints closes up first, and
/// only a row that still does not fit loses a hint -- whole. `guide` is the
/// way into the help panel, and it is never shed: a crowded row keeps the key
/// and gives up its label instead.
pub fn render_footer(theme: &Theme, width: isize, hints: &[(String, String)], counter: &str, guide: &str) -> String {
    let hint_groups: Vec<Vec<Segment>> = hints
        .iter()
        .map(|(key, label)| {
            let mut group = vec![Segment::new("muted", key.clone())];
            if !label.is_empty() {
                group.push(Segment::new("dim", format!(" {}", label)));
            }
            group
        })
        .collect();
    let mut spellings = vec![guide.to_string()];
    if let Some((first, _)) = guide.split_once(' ') {
        spellings.push(first.to_string());
    }
    let mut segments: Vec<Segment> = Vec::new();
    let mut tail: Vec<Segment> = Vec::new();
    for (index, spelling) in spellings.iter().enumerate() {
        tail = footer_tail(spelling, counter);
        let (found, kept) = footer_hints(&hint_groups, width - width_of(&tail) - 2);
        segments = found;
        if kept == hint_groups.len() || index == spellings.len() - 1 {
            break;
        }
    }
    let used = width_of(&segments);
    if used + width_of(&tail) < width {
        segments.push(Segment::new("", " ".repeat((width - used - width_of(&tail)).max(0) as usize)));
        segments.extend(tail);
    }
    text(theme, &fit(&segments, width))
}

/// The right-hand corner: the way into the help, then how many folders.
fn footer_tail(guide: &str, counter: &str) -> Vec<Segment> {
    let mut tail = Vec::new();
    if !guide.is_empty() {
        tail.push(Segment::new("muted", guide));
    }
    if !counter.is_empty() {
        tail.push(Segment::new("dim", format!("{}{}", if guide.is_empty() { "" } else { "  " }, counter)));
    }
    tail
}

fn footer_hints(groups: &[Vec<Segment>], room: isize) -> (Vec<Segment>, usize) {
    let mut kept: Vec<Vec<Segment>> = groups.to_vec();
    loop {
        let mut segments = Vec::new();
        for gap in ["   ", "  "] {
            segments = hint_row(&kept, gap);
            if width_of(&segments) <= room {
                return (segments, kept.len());
            }
        }
        if kept.len() <= 1 {
            return (segments, kept.len());
        }
        kept.pop();
    }
}

// -- preview ---------------------------------------------------------------

pub fn status_word(status: &str) -> &'static str {
    match status {
        "blocked" => "needs you",
        "working" => "working",
        "done" => "done",
        "idle" => "idle",
        _ => "",
    }
}

/// "branch" and two columns of air.
pub const PREVIEW_LABEL: isize = 8;

/// Everything the preview says about one folder, gathered by the picker.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub name: String,
    pub path: String,                   // as displayed: home folded to ~
    pub contents: Option<Rc<Contents>>, // None when it is not a folder (yet)
    pub branch: Option<String>,
    pub space: Option<String>, // the label of the space it already has
    pub agents: Vec<FolderAgent>,
    pub hidden: bool, // list dot-files too
}

/// The folder under the cursor: where it is, who is in it, and what is inside.
///
/// Laid out the way herdr-bar previews a pane -- a bold heading, the path
/// under it, a rule -- so the two popups read as one family. The facts that
/// decide what Enter does come first; the listing fills whatever is left and
/// says how much it left out.
pub fn render_preview(theme: &Theme, width: isize, height: isize, preview: Option<&Preview>) -> Vec<String> {
    let Some(preview) = preview.filter(|_| width > 0 && height > 0) else {
        return vec![String::new(); height.max(0) as usize];
    };

    let mut rows = vec![
        text(theme, &[Segment::bold("text", truncate(&preview.name, width), true)]),
        text(theme, &[Segment::new("muted", truncate_middle(&preview.path, width))]),
        render_rule(theme, width, -1, "┼"),
    ];

    let mut facts: Vec<Vec<Segment>> = Vec::new();
    let mut labels: Vec<&str> = Vec::new();
    if let Some(branch) = preview.branch.as_deref().filter(|b| !b.is_empty()) {
        facts.push(vec![Segment::new("text", branch)]);
        labels.push("branch");
    }
    if let Some(space) = preview.space.as_deref().filter(|s| !s.is_empty()) {
        facts.push(vec![Segment::bold("space", space, true)]);
        labels.push("space");
    }
    for (index, agent) in preview.agents.iter().enumerate() {
        let mut line: Vec<Segment> = Vec::new();
        let glyph = status_glyph(&agent.status);
        line.push(if !glyph.is_empty() {
            Segment::new(&agent.status, format!("{} ", glyph))
        } else {
            Segment::new("", "  ")
        });
        let kind = Some(agent.kind.as_str());
        let logo = theme.logo(kind);
        if !logo.is_empty() {
            line.push(Segment::new(&theme.agent_role(kind), format!("{}{}", logo, GAP)));
        }
        let name = if !agent.name.is_empty() {
            agent.name.as_str()
        } else if !agent.kind.is_empty() {
            agent.kind.as_str()
        } else {
            "agent"
        };
        line.push(Segment::new(&theme.agent_role(kind), name));
        let word = status_word(&agent.status);
        if !word.is_empty() {
            line.push(Segment::new(&agent.status, format!("  {}", word)));
        }
        facts.push(line);
        labels.push(if index == 0 { "agents" } else { "" });
    }
    for (label, fact) in labels.iter().zip(facts.iter()) {
        let mut segments = vec![Segment::new("dim", pad(label, PREVIEW_LABEL))];
        segments.extend(fact.iter().cloned());
        rows.push(text(theme, &fit(&segments, width)));
    }
    if !facts.is_empty() {
        rows.push(String::new());
    }

    let room = height - rows.len() as isize;
    if room > 0 {
        rows.extend(preview_listing(theme, width, room, preview));
    }
    rows.resize(rows.len().max(height as usize), String::new());
    rows.truncate(height as usize);
    rows
}

/// What is inside the folder, directories first, cut to fit with a count.
fn preview_listing(theme: &Theme, width: isize, room: isize, preview: &Preview) -> Vec<String> {
    let Some(listed) = preview.contents.as_deref() else {
        return vec![text(theme, &[Segment::new("dim", truncate("not a folder yet", width))])];
    };
    if !listed.readable {
        return vec![text(theme, &[Segment::new("dim", truncate("not readable", width))])];
    }
    let shown = |name: &&String| preview.hidden || !name.starts_with('.');
    let mut entries: Vec<(&str, bool)> = listed.dirs.iter().filter(shown).map(|n| (n.as_str(), true)).collect();
    entries.extend(listed.files.iter().filter(shown).map(|n| (n.as_str(), false)));
    if entries.is_empty() {
        return vec![text(theme, &[Segment::new("dim", truncate("empty", width))])];
    }

    let mut more = 0;
    if entries.len() as isize > room {
        let keep = (room - 1).max(0) as usize; // the last row says how many did not fit
        more = entries.len() - keep;
        entries.truncate(keep);
    }
    let mut out = Vec::new();
    for (name, is_dir) in entries {
        let segments = if is_dir {
            vec![Segment::new("text", truncate(name, (width - 1).max(1))), Segment::new("dim", "/")]
        } else {
            vec![Segment::new("muted", truncate(name, width))]
        };
        out.push(text(theme, &fit(&segments, width)));
    }
    if more > 0 {
        out.push(text(theme, &[Segment::new("dim", truncate(&format!("… {} more", more), width))]));
    }
    out
}

pub const HELP_TITLE: &str = "what this popup does";
pub const HELP_CLOSE: &str = "? or esc  close";

pub const HELP_INTRO: &str =
    "Type a folder, pick an agent, press ⏎. The line above the hints says what ⏎ is about to do.";

pub const HELP_CREATE: &str = "where ⏎ lands, kept across popups and restarts. You are on the marked one.";

/// The block the panel exists for: which position the toggle is on decides
/// what Enter does, and none of that fits on the one line the picker spares.
pub const HELP_MODES: [(&str, &str); 3] = [
    (
        "smart",
        "a tab in the space that folder already has, a new space when it has none — and it \"has \
         a space\" when some pane in one is standing in it, symlinks followed.",
    ),
    (
        "tab",
        "always a tab: in that folder's own space when it has one, otherwise in the space you are \
         looking at now.",
    ),
    ("space", "always a new space, even when the folder already has one."),
];

pub const HELP_NOTES: [(&str, &str); 3] = [
    (
        "^Y",
        "starts that agent with its permission prompts off, for this tab only — never remembered, \
         unlike the agent. claude and codex have checked flags; anything else refuses rather than \
         guess until skip_permission_flags in config.json teaches it one.",
    ),
    (
        "the list",
        "before you type: folders picked recently, then ones that already have a space — badged \
         with its name, and ◉ ◐ ● ✓ when an agent is in it — then zoxide's list, then the \
         children of your roots. Type a / ~ or . and it completes real directories instead.",
    ),
    (
        "the agent",
        "a shell, or any agent Herdr can start that is installed here. Each folder remembers the \
         one you last opened it with and shows it on its row; the new one is named after the \
         folder.",
    ),
];

/// A key and what it does.
pub type HelpKey = (&'static str, &'static str);

/// Two columns, because the keys are the part of this you scan rather than
/// read and the rows they save are rows the prose above wanted.
pub const HELP_KEYS: [(HelpKey, HelpKey); 6] = [
    (("⇥", "complete, or move the agent"), ("^W", "back one path segment")),
    (("→", "fill in the highlighted folder"), ("^U", "clear the input")),
    (("↑↓", "move the folders"), ("^R", "rescan the filesystem")),
    (("⇧⇥", "move the agents"), ("^O", "show or hide the folder preview")),
    (("⏎", "create it"), ("click", "select, click again to create")),
    (("esc", "give up and close the popup"), ("", "")),
];

pub const HELP_KEY_COLUMN: isize = 11;
pub const HELP_INDENT: isize = 2;

/// The whole popup, given over to what the keys and the modes mean.
///
/// Ordered by what someone opening it is stuck on: the create mode first, and
/// the movement keys last. The position the picker is on is marked with the
/// same bar the list uses for the row it is on. Prose is wrapped rather than
/// written to fit.
pub fn render_help(theme: &Theme, width: isize, height: isize, mode: &str) -> Vec<String> {
    let rule = render_rule(theme, width, -1, "┼");
    let mut rows = vec![help_title(theme, width), rule.clone()];
    rows.extend(help_paragraph(theme, width, HELP_INTRO));
    rows.push(String::new());
    rows.extend(help_entry(theme, width, "^T", HELP_CREATE, HELP_KEY_COLUMN));
    for (name, body) in HELP_MODES {
        rows.extend(help_mode(theme, width, name, body, name == mode));
    }
    rows.push(String::new());
    for (index, (key, body)) in HELP_NOTES.iter().enumerate() {
        if index > 0 {
            rows.push(String::new());
        }
        rows.extend(help_entry(theme, width, key, body, HELP_KEY_COLUMN));
    }
    rows.push(rule);
    for (left, right) in HELP_KEYS {
        rows.push(help_key_row(theme, width, left, right));
    }
    // Shed from the bottom, the way the picker's own chrome does.
    let height = height.max(0) as usize;
    let missing = height.saturating_sub(rows.len());
    rows.truncate(height);
    rows.extend(std::iter::repeat_n(String::new(), missing));
    rows
}

fn help_title(theme: &Theme, width: isize) -> String {
    let gap = width - 2 - display_width(HELP_TITLE) as isize - display_width(HELP_CLOSE) as isize;
    let mut segments =
        vec![Segment::new("", " "), Segment::bold("accent", truncate(HELP_TITLE, (width - 2).max(0)), true)];
    if gap >= 2 {
        let (keys, label) = HELP_CLOSE.split_once("  ").unwrap_or((HELP_CLOSE, ""));
        segments.push(Segment::new("", " ".repeat(gap as usize)));
        segments.push(Segment::new("accent", keys));
        segments.push(Segment::new("dim", format!("  {}", label)));
    }
    text(theme, &segments)
}

fn help_paragraph(theme: &Theme, width: isize, body: &str) -> Vec<String> {
    let indent = " ".repeat(HELP_INDENT as usize);
    wrap(body, (width - HELP_INDENT * 2).max(1))
        .into_iter()
        .map(|line| text(theme, &[Segment::new("", indent.clone()), Segment::new("text", line)]))
        .collect()
}

/// A key, and everything there is to say about it, wrapped under itself.
fn help_entry(theme: &Theme, width: isize, keys: &str, body: &str, column: isize) -> Vec<String> {
    let room = (width - HELP_INDENT - column - 1).max(1);
    let lines = wrap(body, room);
    // A key is drawn as one, in the accent the picker's own chrome uses for
    // the thing you act on; a topic ("the list") is a heading, so bold text.
    let topic = keys.contains(' ');
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let label = if index == 0 { keys } else { "" };
            text(
                theme,
                &[
                    Segment::new("", " ".repeat(HELP_INDENT as usize)),
                    Segment::bold(if topic { "text" } else { "accent" }, pad(&truncate(label, column), column), true),
                    Segment::new("text", line),
                ],
            )
        })
        .collect()
}

fn help_key_row(theme: &Theme, width: isize, left: (&str, &str), right: (&str, &str)) -> String {
    let half = 10.max(floordiv(width - HELP_INDENT, 2));
    let mut segments = vec![Segment::new("", " ".repeat(HELP_INDENT as usize))];
    for (index, (keys, body)) in [left, right].into_iter().enumerate() {
        let room = if index == 0 { half - HELP_KEY_COLUMN } else { width - HELP_INDENT - half - 6 };
        segments.push(Segment::new("accent", pad(&truncate(keys, 6), 6)));
        segments.push(Segment::new("text", pad(&truncate(body, room.max(1)), room.max(1))));
    }
    text(theme, &fit(&segments, width))
}

/// One create mode, marked with the same bar the list uses for its own row.
///
/// Indented to start where the ^T entry's own text does, so the three
/// positions read as the body of that entry rather than as entries of their
/// own.
fn help_mode(theme: &Theme, width: isize, name: &str, body: &str, current: bool) -> Vec<String> {
    let column = 7;
    let mark = format!("{} ", SELECTED_BAR);
    let indent = HELP_INDENT + HELP_KEY_COLUMN;
    let room = (width - indent - char_len(&mark) as isize - column).max(1);
    wrap(body, room)
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            text(
                theme,
                &fit(
                    &[
                        Segment::new("", " ".repeat(indent as usize)),
                        Segment::bold(
                            "accent",
                            if current && index == 0 { mark.clone() } else { "  ".to_string() },
                            true,
                        ),
                        Segment::bold(
                            if current { "accent" } else { "muted" },
                            pad(if index == 0 { name } else { "" }, column),
                            current,
                        ),
                        Segment::new(if current { "text" } else { "muted" }, line),
                    ],
                    width,
                ),
            )
        })
        .collect()
}

/// A horizontal rule; `junction` is the column a vertical rule meets it at.
pub fn render_rule(theme: &Theme, width: isize, junction: isize, glyph: &str) -> String {
    let count = width.max(0) as usize;
    let mut line = String::with_capacity(count * 3);
    for index in 0..count {
        if index as isize == junction {
            line.push_str(glyph);
        } else {
            line.push('─');
        }
    }
    text(theme, &[Segment::new("rule", line)])
}

/// Python's `round()`: halves go to the even neighbour.
fn py_round(value: f64) -> i64 {
    value.round_ties_even() as i64
}

/// Track and thumb for a list taller than its window, one entry per row, or
/// nothing when the list fits.
pub fn scrollbar_column(total: usize, visible: isize, offset: usize, height: isize) -> Vec<&'static str> {
    if total as isize <= visible || height <= 0 {
        return Vec::new();
    }
    let thumb = 1.max(py_round(height as f64 * visible as f64 / total as f64));
    let span = height as i64 - thumb;
    let reach = 1.max(total as i64 - visible as i64);
    let start = py_round(span as f64 * (offset as i64).min(reach) as f64 / reach as f64);
    (0..height as i64)
        .map(|index| if start <= index && index < start + thumb { SCROLL_THUMB } else { SCROLL_TRACK })
        .collect()
}

pub fn pad_row(text: &str, width: isize) -> String {
    pad(text, width)
}
