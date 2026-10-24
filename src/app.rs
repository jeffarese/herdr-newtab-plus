//! The picker: state, keys, and the frame loop.
//!
//! One screen, two fields. The folder field leads because it is the one you
//! type; the agent field is a row of chips that is always visible, so the
//! optional half of the decision never has to be discovered. Between them sits
//! the only piece of inference in the plugin -- whether this folder becomes a
//! tab in a space that is already open or a space of its own -- spelled out on
//! the line above the hints so Enter is never a surprise.
//!
//! Nothing here talks to Herdr except through the snapshot taken when the
//! picker opens. Creating anything happens after the popup is gone, in `cli`.

use crate::agents::{can_skip_permissions, initial_index, Agent};
use crate::config::Config;
use crate::keys::{Event, KeyDecoder, MouseKind};
use crate::paint::Painter;
use crate::paths::{self, Row, Scored};
use crate::pyjson::Value;
use crate::pystr;
use crate::render::{self, Layout, Preview};
use crate::session::{
    folder_statuses_and_agents, next_mode, normalize, resolve_target_with, resolved, space_folders, FolderAgent,
    SpaceMap, Target,
};
use crate::state::Recents;
use crate::term::Terminal;
use crate::theme::Theme;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub const ESC_TIMEOUT: f64 = 0.06;
pub const STATUS_SECONDS: f64 = 2.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Cancel,
    Confirm,
}

/// What the picker returns: a folder, an agent, and where it should land.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub folder: String,
    pub agent: Agent,
    pub target: Target,
    pub skip_permissions: bool,
}

fn char_len(text: &str) -> usize {
    text.chars().count()
}

/// The byte offset of a character index, clamped to the end.
fn byte_at(text: &str, index: usize) -> usize {
    text.char_indices().nth(index).map(|(i, _)| i).unwrap_or(text.len())
}

pub struct Picker {
    pub snapshot: Value,
    pub catalog: Vec<Row>,
    pub chips: Vec<Agent>,
    pub config: Config,
    pub recents: Recents,
    pub theme: Theme,
    pub base: String,

    pub query: String,
    /// In characters, as the Python release counted it.
    pub cursor: usize,
    pub rows: Vec<Scored>,
    /// What the last catalog query matched, for the next one to narrow.
    narrowing: paths::Narrowing,
    pub selected: usize,
    pub offset: usize,
    pub chip: usize,
    chip_pinned: bool,
    /// Where Enter lands: the automatic decision, or one of the two overrides.
    /// One setting for the whole picker, and it outlives the popup.
    pub create_mode: &'static str,
    /// Deliberately not remembered per folder the way the agent is: starting
    /// an agent without its permission prompts is a decision about this run.
    pub skip_permissions: bool,
    /// The panel takes the whole popup while it is up.
    pub help: bool,

    // What the rows say about each folder beyond its name, fixed for the life
    // of the popup, so looked up once rather than per row per frame.
    row_agents: HashMap<String, String>,
    statuses: HashMap<String, String>,
    agents_in: HashMap<String, Vec<FolderAgent>>,
    spaces: SpaceMap,
    // What Enter decides with. The Python release worked this out afresh on
    // every frame; it only changes when the rescan key forgets the symlinks
    // it followed, so it is worked out again exactly then.
    target_spaces: SpaceMap,

    /// The preview follows herdr-bar's rules: on unless the config says
    /// false, shown by itself only on a popup wide enough to spare the column,
    /// and once ^O has been pressed, shown or hidden as asked wherever it fits.
    pub preview_enabled: bool,
    pub preview_chosen: bool,
    pub(crate) preview_showing: bool,
    pub(crate) list_right: isize,

    pub status: Option<String>,
    status_until: Instant,
    dirty: bool,
    painter: Painter,
    pub(crate) list_height: isize,
    pub(crate) row_top: isize,
    pub(crate) chip_row: isize,
    pub(crate) chip_spans: Vec<(isize, isize)>,
}

impl Picker {
    pub fn new(
        snapshot: Value,
        catalog: Vec<Row>,
        chips: Vec<Agent>,
        config: Config,
        recents: Recents,
        theme: Theme,
        base: String,
    ) -> Picker {
        let chip = initial_index(&chips, &config.default_agent, recents.last_agent().as_deref());
        let create_mode = recents.create_mode().unwrap_or(config.create);
        let row_agents = if config.default_agent == "last" { recents.agents() } else { HashMap::new() };
        let (statuses, agents_in) = folder_statuses_and_agents(&snapshot);
        let spaces = space_folders(&snapshot);
        let target_spaces = spaces.clone();
        let preview_enabled = !config.preview.is_false();
        let mut picker = Picker {
            snapshot,
            catalog,
            chips,
            config,
            recents,
            theme,
            base,
            query: String::new(),
            cursor: 0,
            rows: Vec::new(),
            narrowing: paths::Narrowing::default(),
            selected: 0,
            offset: 0,
            chip,
            chip_pinned: false,
            create_mode,
            skip_permissions: false,
            help: false,
            row_agents,
            statuses,
            agents_in,
            spaces,
            target_spaces,
            preview_enabled,
            preview_chosen: false,
            preview_showing: false,
            list_right: 0,
            status: None,
            status_until: Instant::now(),
            dirty: true,
            painter: Painter::new(),
            list_height: 1,
            row_top: 0,
            chip_row: -1,
            chip_spans: Vec::new(),
        };
        picker.refilter(None);
        picker
    }

    /// The whole catalog, for a picker made from its leading rows. Nothing
    /// has been typed yet, so the list is the resting one again.
    pub fn set_catalog(&mut self, catalog: Vec<Row>) {
        self.catalog = catalog;
        self.narrowing = paths::Narrowing::default();
        self.chip = initial_index(&self.chips, &self.config.default_agent, self.recents.last_agent().as_deref());
        self.refilter(None);
    }

    // -- derived state ------------------------------------------------------

    pub fn refilter(&mut self, keep: Option<&str>) {
        self.rows = paths::suggest_narrowing(
            &self.query,
            &self.catalog,
            &self.base,
            self.config.hidden,
            self.config.max_suggestions,
            &mut self.narrowing,
        );
        self.selected = 0;
        if let Some(keep) = keep.filter(|k| !k.is_empty()) {
            if let Some(index) = self.rows.iter().position(|row| row.suggestion.path == keep) {
                self.selected = index;
            }
        }
        self.offset = 0;
        self.sync_agent();
        self.dirty = true;
    }

    pub fn selected_row(&self) -> Option<&Scored> {
        if self.rows.is_empty() {
            return None;
        }
        self.rows.get(self.selected.min(self.rows.len() - 1))
    }

    /// The folder Enter would use: the highlighted row, or what was typed.
    pub fn folder(&self) -> String {
        match self.selected_row() {
            Some(row) => row.suggestion.path.clone(),
            None => paths::resolve_input(&self.query, &self.base),
        }
    }

    pub fn agent(&self) -> &Agent {
        &self.chips[self.chip.min(self.chips.len() - 1)]
    }

    /// Whether the selected agent has a known way to skip its prompts.
    pub fn can_skip(&self) -> bool {
        can_skip_permissions(&self.agent().kind, &self.config.skip_permission_flags)
    }

    /// Whether Enter would actually pass the skip-permissions flags.
    ///
    /// The toggle is one piece of state for the whole picker, but it only
    /// means anything on a chip that can honour it.
    pub fn skipping(&self) -> bool {
        self.skip_permissions && self.can_skip()
    }

    pub fn toggle_skip_permissions(&mut self) {
        if self.skip_permissions {
            self.skip_permissions = false;
            self.dirty = true;
            return;
        }
        if !self.can_skip() {
            let agent = self.agent();
            let who = if agent.is_none() { "a plain shell".to_string() } else { agent.label.clone() };
            self.flash(&format!("no skip-permissions flag for {}", who));
            return;
        }
        self.skip_permissions = true;
        self.dirty = true;
    }

    pub fn ghost(&self) -> String {
        paths::completion_for(&self.query, self.selected_row().map(|row| &*row.suggestion))
    }

    pub fn target(&self) -> Option<Target> {
        let folder = self.folder();
        if folder.is_empty() {
            return None;
        }
        Some(resolve_target_with(&folder, &self.snapshot, self.create_mode, &self.target_spaces))
    }

    /// Move Enter along: decide for me, always a tab, always a space.
    pub fn cycle_create_mode(&mut self) {
        self.create_mode = next_mode(self.create_mode);
        self.recents.remember_create_mode(self.create_mode);
        self.dirty = true;
    }

    pub fn error(&self) -> String {
        if let Some(status) = &self.status {
            return status.clone();
        }
        let folder = self.folder();
        if folder.is_empty() {
            return String::new();
        }
        if !paths::is_directory(&folder, false) {
            return format!("{} does not exist", paths::display(&folder));
        }
        String::new()
    }

    /// Follow the folder's remembered agent until the row is touched by hand.
    fn sync_agent(&mut self) {
        if self.chip_pinned || self.config.default_agent != "last" {
            return;
        }
        let folder = self.folder();
        if folder.is_empty() {
            return;
        }
        let Some(remembered) = self.recents.agent_for(&folder) else { return };
        if let Some(index) = self.chips.iter().position(|chip| chip.kind == remembered) {
            self.chip = index;
        }
    }

    pub fn flash(&mut self, message: &str) {
        self.status = Some(message.to_string());
        self.status_until = Instant::now() + Duration::from_secs_f64(STATUS_SECONDS);
        self.dirty = true;
    }

    // -- editing ------------------------------------------------------------

    pub fn set_query(&mut self, query: String, cursor: Option<usize>) {
        let length = char_len(&query);
        self.cursor = cursor.map(|c| c.min(length)).unwrap_or(length);
        self.query = query;
        self.refilter(None);
    }

    pub fn toggle_help(&mut self) {
        self.help = !self.help;
        self.status = None;
        self.dirty = true;
    }

    pub fn insert(&mut self, text: &str, pasted: bool) {
        if self.help {
            self.toggle_help();
            return;
        }
        // A path can legitimately contain a question mark, so `?` only asks
        // for the panel where it cannot be something you meant to type.
        if text == "?" && !pasted && self.query.is_empty() {
            self.toggle_help();
            return;
        }
        let cleaned: String = text
            .chars()
            .map(|c| if pasted && (c == '\n' || c == '\r') { ' ' } else { c })
            .filter(|&c| c >= ' ' && c != '\x7f')
            .collect();
        if cleaned.is_empty() {
            return;
        }
        let at = byte_at(&self.query, self.cursor);
        let query = format!("{}{}{}", &self.query[..at], cleaned, &self.query[at..]);
        let cursor = self.cursor + char_len(&cleaned);
        self.set_query(query, Some(cursor));
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = byte_at(&self.query, self.cursor - 1);
        let end = byte_at(&self.query, self.cursor);
        let query = format!("{}{}", &self.query[..start], &self.query[end..]);
        let cursor = self.cursor - 1;
        self.set_query(query, Some(cursor));
    }

    pub fn delete_forward(&mut self) {
        if self.cursor >= char_len(&self.query) {
            return;
        }
        let start = byte_at(&self.query, self.cursor);
        let end = byte_at(&self.query, self.cursor + 1);
        let query = format!("{}{}", &self.query[..start], &self.query[end..]);
        let cursor = self.cursor;
        self.set_query(query, Some(cursor));
    }

    /// Backwards by one path segment, which is the unit a path is edited in.
    pub fn delete_word(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = byte_at(&self.query, self.cursor);
        let mut head: &str = pystr::rstrip(&self.query[..at]);
        if let Some(stripped) = head.strip_suffix('/') {
            head = stripped;
        }
        let chars: Vec<char> = head.chars().collect();
        let cut = chars.iter().rposition(|&c| c == '/' || c == ' ').map(|i| i + 1).unwrap_or(0);
        let cut_byte = byte_at(&self.query, cut);
        let query = format!("{}{}", &self.query[..cut_byte], &self.query[at..]);
        self.set_query(query, Some(cut));
    }

    /// Fill the input with the highlighted folder, ready to go one level deeper.
    pub fn accept_completion(&mut self) -> bool {
        let Some(row) = self.selected_row() else { return false };
        let text = paths::accept_text(&self.query, &row.suggestion);
        if text == self.query {
            return false;
        }
        self.set_query(text, None);
        true
    }

    // -- movement -----------------------------------------------------------

    pub fn move_by(&mut self, delta: isize, wrap: bool) {
        if self.rows.is_empty() {
            return;
        }
        let total = self.rows.len() as isize;
        let target = self.selected as isize + delta;
        self.selected = if wrap { target.rem_euclid(total) } else { target.clamp(0, total - 1) } as usize;
        self.sync_agent();
        self.dirty = true;
    }

    pub fn cycle_agent(&mut self, delta: isize) {
        if self.chips.len() < 2 {
            return;
        }
        self.chip = (self.chip as isize + delta).rem_euclid(self.chips.len() as isize) as usize;
        self.chip_pinned = true;
        self.dirty = true;
    }

    // -- keys ---------------------------------------------------------------

    pub fn handle_key(&mut self, name: &str) -> Option<Outcome> {
        if name == "ctrl+c" || name == "ctrl+q" {
            return Some(Outcome::Cancel);
        }
        if self.help {
            // Anything at all closes it, Enter included: creating a tab from
            // behind the help is not what the key was pressed for.
            self.toggle_help();
            return None;
        }
        match name {
            "esc" => return Some(Outcome::Cancel),
            "f1" => {
                self.toggle_help();
                return None;
            }
            "enter" => return Some(Outcome::Confirm),
            "up" | "ctrl+p" => self.move_by(-1, true),
            "down" | "ctrl+n" => self.move_by(1, true),
            "pgup" => self.move_by(-self.page(), false),
            "pgdn" => self.move_by(self.page(), false),
            "tab" => {
                // Shell habit first: complete while there is visibly something
                // to complete, and only once the path stands on its own move on
                // to the agent.
                if !self.ghost().is_empty() {
                    self.accept_completion();
                } else {
                    self.cycle_agent(1);
                }
            }
            "shift+tab" => self.cycle_agent(-1),
            "right" | "ctrl+f" => {
                if self.cursor >= char_len(&self.query) {
                    if !self.accept_completion() {
                        self.flash("nothing to complete");
                    }
                } else {
                    self.cursor += 1;
                    self.dirty = true;
                }
            }
            "left" | "ctrl+b" => {
                self.cursor = self.cursor.saturating_sub(1);
                self.dirty = true;
            }
            "home" | "ctrl+a" => {
                self.cursor = 0;
                self.dirty = true;
            }
            "end" | "ctrl+e" => {
                self.cursor = char_len(&self.query);
                self.dirty = true;
            }
            "backspace" => self.backspace(),
            "delete" => self.delete_forward(),
            "ctrl+w" | "alt+backspace" => self.delete_word(),
            "ctrl+u" => self.set_query(String::new(), None),
            "ctrl+k" => {
                let at = byte_at(&self.query, self.cursor);
                let head = self.query[..at].to_string();
                let cursor = self.cursor;
                self.set_query(head, Some(cursor));
            }
            "ctrl+y" => self.toggle_skip_permissions(),
            "ctrl+t" => self.cycle_create_mode(),
            "ctrl+o" => self.toggle_preview(),
            "ctrl+r" => {
                paths::clear_cache();
                self.narrowing = paths::Narrowing::default();
                self.target_spaces = space_folders(&self.snapshot);
                let folder = self.folder();
                self.refilter(Some(&folder));
                self.flash("rescanned");
            }
            _ => {}
        }
        None
    }

    /// Show the preview if it is hidden, hide it if it is showing.
    ///
    /// Measured against what is on screen rather than against the setting, so
    /// a preview "auto" kept hidden on a narrow popup comes up on the first
    /// press instead of the second.
    pub fn toggle_preview(&mut self) {
        self.preview_enabled = !self.preview_showing;
        self.preview_chosen = true;
        self.dirty = true;
    }

    pub fn preview_fits(&self, inner: isize) -> bool {
        if !self.preview_enabled {
            return false;
        }
        if self.preview_chosen || self.config.preview.is_true() {
            return inner >= render::PREVIEW_FITS;
        }
        inner >= render::PREVIEW_AUTO
    }

    /// What the preview says about the folder Enter would use.
    pub fn preview(&self) -> Option<Preview> {
        let folder = self.folder();
        if folder.is_empty() {
            return None;
        }
        let listed = paths::contents(&folder);
        let space = self.spaces.get(&folder).or_else(|| self.spaces.get(&resolved(&folder)));
        let agents =
            self.agents_in.get(&folder).or_else(|| self.agents_in.get(&resolved(&folder))).cloned().unwrap_or_default();
        let branch = listed.as_deref().and_then(|listed| paths::git_branch(&folder, Some(listed)));
        Some(Preview {
            name: paths::folder_name(&folder),
            path: paths::display(&folder),
            contents: listed,
            branch,
            space: space.map(|space| space.label.clone()),
            agents,
            hidden: self.config.hidden,
        })
    }

    fn page(&self) -> isize {
        (self.list_height - 1).max(1)
    }

    pub fn handle_mouse(&mut self, kind: MouseKind, x: i128, y: i128) -> Option<Outcome> {
        if self.help {
            if kind == MouseKind::Press {
                self.toggle_help();
            }
            return None;
        }
        match kind {
            MouseKind::WheelUp => {
                self.move_by(-3, false);
                return None;
            }
            MouseKind::WheelDown => {
                self.move_by(3, false);
                return None;
            }
            MouseKind::Release => return None,
            MouseKind::Press => {}
        }

        let row = y - 1;
        if row == self.chip_row as i128 {
            for (index, &(start, end)) in self.chip_spans.iter().enumerate() {
                if (start as i128) < x && x - 1 < end as i128 {
                    self.chip = index;
                    self.chip_pinned = true;
                    self.dirty = true;
                    return None;
                }
            }
            return None;
        }

        // The list's own rows only: not the air under a short list, and not
        // the preview beside it, which shows a folder rather than offering one.
        let line = row - self.row_top as i128;
        if !(0..self.list_height as i128).contains(&line) || x > self.list_right as i128 {
            return None;
        }
        let index = self.offset as i128 + line;
        if 0 <= index && index < self.rows.len() as i128 {
            let index = index as usize;
            let already = index == self.selected;
            self.selected = index;
            self.sync_agent();
            self.dirty = true;
            // A second click on the row you are already on is the commit, so a
            // single click can still be used to look before you leap.
            return if already { Some(Outcome::Confirm) } else { None };
        }
        None
    }

    // -- drawing ------------------------------------------------------------

    pub fn invalidate(&mut self) {
        self.painter.reset();
        self.dirty = true;
    }

    fn scroll_into_view(&mut self, height: isize) {
        let height = height.max(0) as usize;
        if self.selected < self.offset {
            self.offset = self.selected;
        } else if self.selected >= self.offset + height {
            self.offset = self.selected + 1 - height.min(self.selected + 1);
        }
        self.offset = self.offset.min(self.rows.len().saturating_sub(height));
    }

    fn list_rows(&mut self, layout: &Layout) -> Vec<String> {
        let width = if layout.list_width != 0 { layout.list_width } else { layout.inner_width };
        let height = layout.list_height;
        self.scroll_into_view(height);
        if self.rows.is_empty() {
            return render::render_empty(&self.theme, width, height, &self.query);
        }

        let bar = render::scrollbar_column(self.rows.len(), height, self.offset, height);
        // Beside the preview the separator is the scrollbar's track, so the
        // list keeps its full width and there is one vertical line.
        let beside = layout.preview_width != 0;
        let body_width = width - if !bar.is_empty() && !beside { 2 } else { 0 };
        let end = (self.offset + height as usize).min(self.rows.len());
        let name_room = render::name_column(&self.rows[self.offset.min(end)..end], body_width);
        let mut out = Vec::with_capacity(height as usize);
        for line in 0..height as usize {
            let index = self.offset + line;
            if index >= self.rows.len() {
                out.push(String::new());
                continue;
            }
            let row = &self.rows[index];
            let path = &row.suggestion.path;
            let mut text = render::render_row(
                &self.theme,
                row,
                body_width,
                index == self.selected,
                Some(name_room),
                self.row_agents.get(path).map(|s| s.as_str()),
                self.statuses.get(path).map(|s| s.as_str()),
            );
            if !bar.is_empty() && !beside {
                let thumb = bar[line] == render::SCROLL_THUMB;
                text = format!(
                    "{} {}",
                    render::pad_row(&text, body_width),
                    self.theme.paint(if thumb { "muted" } else { "rule" }, bar[line])
                );
            }
            out.push(text);
        }
        out
    }

    /// The list, and the preview beside it when there is one, row by row.
    fn body_rows(&mut self, layout: &Layout) -> Vec<String> {
        let mut rows = self.list_rows(layout);
        if layout.preview_width == 0 {
            rows.extend(std::iter::repeat_n(String::new(), layout.spare.max(0) as usize));
            return rows;
        }
        let height = layout.list_height + layout.spare;
        let preview = render::render_preview(&self.theme, layout.preview_width, height, self.preview().as_ref());
        let bar = if self.rows.is_empty() {
            Vec::new()
        } else {
            render::scrollbar_column(self.rows.len(), layout.list_height, self.offset, layout.list_height)
        };
        let mut out = Vec::with_capacity(height as usize);
        for (index, line) in preview.iter().enumerate().take(height.max(0) as usize) {
            let left = rows.get(index).map(|s| s.as_str()).unwrap_or("");
            let thumb = bar.get(index).copied() == Some(render::SCROLL_THUMB);
            let glyph = if thumb { render::SCROLL_THUMB } else { render::SEPARATOR.trim() };
            let separator = format!(" {} ", self.theme.paint(if thumb { "muted" } else { "rule" }, glyph));
            out.push(format!("{}{}{}", render::pad_row(left, layout.list_width), separator, line));
        }
        out
    }

    pub(crate) fn hints(&self) -> Vec<(String, String)> {
        // Offered exactly where it would do something: on a chip that cannot
        // honour it the hint goes away along with the chip's mark.
        let mut hints: Vec<(String, String)> = Vec::new();
        let pair = |key: &str, label: &str| (key.to_string(), label.to_string());
        // ⇥ says what it would do right now rather than everything it can do.
        if !self.ghost().is_empty() {
            hints.push(pair("⇥", "complete"));
            hints.push(pair("⇧⇥", "agent"));
        } else {
            hints.push(pair("⇥", "agent"));
            hints.push(pair("→", "complete"));
        }
        hints.push(pair("↑↓", "folder"));
        // Named by the position it is in rather than by the one it is going
        // to, so the footer and the target line's badge always agree.
        hints.push(pair("^T", self.create_mode));
        if self.can_skip() {
            hints.push(pair("^Y", if self.skipping() { "prompts off" } else { "skip perms" }));
        }
        hints.push(pair("⏎", "create"));
        hints.push(pair("esc", "close"));
        hints
    }

    /// The corner that opens the panel -- while `?` is what it would do.
    pub(crate) fn guide(&self) -> &'static str {
        if self.query.is_empty() {
            "? help"
        } else {
            "F1 help"
        }
    }

    /// The whole screen as composed rows -- no terminal, no escape plumbing.
    ///
    /// Kept separate from `draw` so a frame can be rendered and asserted on
    /// without a pty, which is what the tests and the recorder both do.
    pub fn frame(&mut self, width: isize, height: isize) -> Vec<String> {
        let pad = " ";
        if self.help {
            self.chip_row = -1; // nothing on screen is clickable while it is up
            self.row_top = height;
            return render::render_help(&self.theme, width - 2, height, self.create_mode)
                .into_iter()
                .map(|row| format!("{}{}", pad, row))
                .collect();
        }

        let layout = render::compute_layout(width, height, self.preview_fits(width - 2));
        self.preview_showing = layout.preview_width != 0;
        self.list_height = layout.list_height;
        self.list_right =
            pad.len() as isize + if layout.list_width != 0 { layout.list_width } else { layout.inner_width };
        // Where the separator meets the rules above and below the list, so the
        // two panes read as one box divided rather than two drawn side by side.
        let junction = if layout.preview_width != 0 { layout.list_width + 1 } else { -1 };

        let ghost = self.ghost();
        let mut lines = vec![format!(
            "{}{}",
            pad,
            render::render_input(&self.theme, layout.inner_width, &self.query, self.cursor, &ghost)
        )];
        if layout.rules {
            lines.push(format!("{}{}", pad, render::render_rule(&self.theme, layout.inner_width, junction, "┬")));
        }

        self.row_top = lines.len() as isize;
        // Rows the list was not allowed to take sit under it rather than under
        // the chrome, so the input stays at the top and the hints on the floor.
        for row in self.body_rows(&layout) {
            lines.push(format!("{}{}", pad, row));
        }

        if layout.rules {
            lines.push(format!("{}{}", pad, render::render_rule(&self.theme, layout.inner_width, junction, "┴")));
        }

        self.chip_row = -1;
        if layout.chips {
            self.chip_row = lines.len() as isize;
            let (row, spans) =
                render::chips_row(&self.theme, layout.inner_width, &self.chips, self.chip, self.skipping());
            // Shifted by the pad column, so a span is in the same columns the
            // mouse reports (less one, as handle_mouse counts them).
            let shift = pad.len() as isize;
            self.chip_spans = spans
                .into_iter()
                .map(|(start, end)| if start >= 0 { (start + shift, end + shift) } else { (start, end) })
                .collect();
            lines.push(format!("{}{}", pad, row));
        }
        if layout.target {
            let target = self.target();
            let error = self.error();
            lines.push(format!(
                "{}{}",
                pad,
                render::render_target(
                    &self.theme,
                    layout.inner_width,
                    target.as_ref(),
                    Some(self.agent()),
                    &error,
                    self.skipping(),
                    self.create_mode,
                )
            ));
        }
        if layout.note {
            lines.push(format!(
                "{}{}",
                pad,
                render::render_mode_note(&self.theme, layout.inner_width, self.create_mode)
            ));
        }
        if layout.footer {
            let counter =
                if self.rows.is_empty() { String::new() } else { format!("{}/{}", self.selected + 1, self.rows.len()) };
            let hints = self.hints();
            lines.push(format!(
                "{}{}",
                pad,
                render::render_footer(&self.theme, layout.inner_width, &hints, &counter, self.guide())
            ));
        }
        lines
    }

    pub fn draw(&mut self, terminal: &mut Terminal) {
        let (width, height) = terminal.size();
        let rows = self.frame(width as isize, height as isize);
        let out = self.painter.frame(&rows, width as usize, height as usize);
        terminal.write(&out);
        terminal.flush();
        self.dirty = false;
    }

    // -- loop ---------------------------------------------------------------

    pub fn run(&mut self, terminal: &mut Terminal) -> Option<Choice> {
        let mut decoder = KeyDecoder::new();
        self.draw(terminal);
        let mut escape_deadline: Option<Instant> = None;

        loop {
            let timeout = self.next_timeout(escape_deadline);
            if terminal.wait(timeout) {
                let data = terminal.read();
                if data.is_empty() {
                    return None;
                }
                let events = decoder.feed(&data);
                let outcome = self.consume(events);
                escape_deadline = if decoder.pending_escape() {
                    Some(Instant::now() + Duration::from_secs_f64(ESC_TIMEOUT))
                } else {
                    None
                };
                match outcome {
                    Some(Outcome::Cancel) => return None,
                    Some(Outcome::Confirm) => {
                        if let Some(choice) = self.choice() {
                            return Some(choice);
                        }
                    }
                    None => {}
                }
            } else if escape_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                escape_deadline = None;
                let events = decoder.flush();
                if self.consume(events) == Some(Outcome::Cancel) {
                    return None;
                }
            }

            if terminal.take_resize() {
                self.invalidate();
            }
            if self.status.is_some() && Instant::now() >= self.status_until {
                self.status = None;
                self.dirty = true;
            }
            if self.dirty {
                self.draw(terminal);
            }
        }
    }

    pub fn choice(&mut self) -> Option<Choice> {
        let folder = self.folder();
        if folder.is_empty() {
            self.flash("type a folder first");
            return None;
        }
        // Asked afresh: the frame's answer may be a listing read a while ago,
        // and this one is about to have a tab made in it.
        if !paths::is_directory(&folder, true) {
            self.flash(&format!("{} is not a folder", paths::display(&folder)));
            return None;
        }
        let target = resolve_target_with(&folder, &self.snapshot, self.create_mode, &self.target_spaces);
        Some(Choice {
            folder: normalize(&folder),
            agent: self.agent().clone(),
            target,
            skip_permissions: self.skipping(),
        })
    }

    pub fn consume(&mut self, events: Vec<Event>) -> Option<Outcome> {
        let mut outcome: Option<Outcome> = None;
        for event in events {
            match event {
                Event::Key(name) => outcome = self.handle_key(&name).or(outcome),
                Event::Text(text) => self.insert(&text, false),
                Event::Paste(text) => self.insert(&text, true),
                Event::Mouse { kind, x, y } => outcome = self.handle_mouse(kind, x, y).or(outcome),
            }
            if outcome.is_some() {
                return outcome;
            }
        }
        outcome
    }

    fn next_timeout(&self, escape_deadline: Option<Instant>) -> f64 {
        let mut deadlines: Vec<Instant> = Vec::new();
        if let Some(deadline) = escape_deadline {
            deadlines.push(deadline);
        }
        if self.status.is_some() {
            deadlines.push(self.status_until);
        }
        match deadlines.into_iter().min() {
            None => 30.0,
            Some(soonest) => soonest.saturating_duration_since(Instant::now()).as_secs_f64().max(0.01),
        }
    }
}
