//! Rasterising: a frame of styled cells, its caption and key chip, to rgb24.
//! A port of the old Pillow painter (scripts/record_demo.py), look for look.

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// What Herdr hands the pty: `height - 2` rows and `width - 3` columns of the
/// manifest's 110x30 popup. Filming anything else would be filming a popup
/// that ships to nobody.
pub const COLS: usize = 107;
pub const ROWS: usize = 28;

const FONT_SIZE: f32 = 26.0;
const CAPTION_SIZE: f32 = 25.0;
const CHIP_SIZE: f32 = 22.0;
const LINE_RATIO: f32 = 1.36;
pub const CHIP_TTL: f32 = 0.7;

type Rgb = [u8; 3];

const CANVAS_BG: Rgb = [10, 11, 15];
pub const PANEL_BG: Rgb = [20, 22, 29];
const PANEL_EDGE: Rgb = [44, 49, 62];
const PANEL_GLOW: Rgb = [30, 34, 44];
const CAPTION_FG: Rgb = [206, 213, 227];
const CHIP_BG: Rgb = [32, 36, 47];
const CHIP_EDGE: Rgb = [64, 71, 88];
const CHIP_FG: Rgb = [214, 221, 235];
pub const DEFAULT_FG: Rgb = [211, 216, 227];

/// The 16 ANSI slots, in a palette that suits the dark canvas. Everything the
/// theme asks for is one of these, a 256-cube index, or a hex triple.
const BASE16: [Rgb; 16] = [
    [26, 28, 35],
    [226, 112, 122],
    [127, 184, 131],
    [215, 176, 106],
    [111, 158, 217],
    [185, 138, 209],
    [99, 179, 196],
    [200, 205, 216],
    [107, 114, 128],
    [255, 123, 134],
    [143, 220, 154],
    [255, 212, 121],
    [130, 183, 255],
    [215, 163, 240],
    [126, 224, 232],
    [238, 241, 247],
];

const FONT_DIRS: [&str; 4] =
    ["~/Library/Fonts", "/Library/Fonts", "/System/Library/Fonts", "/usr/share/fonts/truetype"];
const FONT_CANDIDATES: [(&str, &str); 5] = [
    ("IosevkaTerm-Extended.ttf", "IosevkaTerm-ExtendedBold.ttf"),
    ("JetBrainsMono-Regular.ttf", "JetBrainsMono-Bold.ttf"),
    ("FiraCode-Regular.ttf", "FiraCode-Bold.ttf"),
    ("DejaVuSansMono.ttf", "DejaVuSansMono-Bold.ttf"),
    ("Menlo.ttc", "Menlo.ttc"),
];
const UI_FONTS: [&str; 3] =
    ["/System/Library/Fonts/SFNSDisplay.ttf", "/System/Library/Fonts/Helvetica.ttc", "/Library/Fonts/Arial.ttf"];
/// herdr-radar's icon font, the one the picker's logos are drawn in.
const ICON_PREFIX: &str = "HerdrAgentIconsMax";

/// The agent logos live in the icon font's private-use block (src/icons.rs).
fn is_logo(c: char) -> bool {
    ('\u{E1A0}'..='\u{E1B7}').contains(&c)
}

/// char -> (reaches up, reaches down, crosses, heavy).
fn box_shape(c: char) -> Option<(bool, bool, bool, bool)> {
    Some(match c {
        '│' => (true, true, false, false),
        '┃' => (true, true, false, true),
        '┬' => (false, true, true, false),
        '┴' => (true, false, true, false),
        '─' => (false, false, true, false),
        _ => return None,
    })
}

pub fn palette(index: usize) -> Rgb {
    if index < 16 {
        return BASE16[index];
    }
    if index < 232 {
        let levels = [0u8, 95, 135, 175, 215, 255];
        let i = index - 16;
        return [levels[i / 36], levels[(i / 6) % 6], levels[i % 6]];
    }
    let value = (8 + (index - 232) * 10) as u8;
    [value, value, value]
}

/// A color as the screen model keeps it: `38;5;n` or `48;2;r;g;b`.
pub fn sgr_color(spec: Option<&str>) -> Option<Rgb> {
    let parts: Vec<usize> = spec?.split(';').map(|p| p.parse().unwrap_or(0)).collect();
    match parts.get(1) {
        Some(5) => Some(palette(parts.get(2).copied().unwrap_or(0) % 256)),
        Some(2) if parts.len() >= 5 => Some([parts[2] as u8, parts[3] as u8, parts[4] as u8]),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
pub struct Cell {
    /// '\0' is the second half of a wide character.
    pub ch: char,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub bold: bool,
    pub underline: bool,
}

pub struct Frame {
    pub cells: Vec<Cell>,
    pub caption: String,
    pub chip: Option<String>,
    pub chip_age: f32,
    pub outcome: String,
}

fn expand(directory: &str) -> PathBuf {
    match directory.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(directory),
    }
}

fn load(path: &Path) -> Option<FontVec> {
    let data = std::fs::read(path).ok()?;
    FontVec::try_from_vec_and_index(data, 0).ok()
}

fn find_font(bold: bool) -> FontVec {
    for (regular, heavy) in FONT_CANDIDATES {
        for directory in FONT_DIRS {
            if let Some(font) = load(&expand(directory).join(if bold { heavy } else { regular })) {
                return font;
            }
        }
    }
    panic!("no monospace font found; install Iosevka Term or JetBrains Mono")
}

fn ui_font() -> FontVec {
    UI_FONTS.iter().find_map(|path| load(Path::new(path))).unwrap_or_else(|| find_font(false))
}

/// The icon font, wherever this machine keeps its fonts.
pub fn icon_font_path() -> Option<PathBuf> {
    for directory in FONT_DIRS {
        let Ok(entries) = std::fs::read_dir(expand(directory)) else { continue };
        let mut names: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                name.starts_with(ICON_PREFIX) && name.ends_with(".ttf")
            })
            .collect();
        names.sort();
        if let Some(path) = names.into_iter().next() {
            return Some(path);
        }
    }
    None
}

fn icon_font() -> FontVec {
    icon_font_path()
        .and_then(|path| load(&path))
        .unwrap_or_else(|| panic!("install herdr-radar's Herdr Agent Icons Max font to film the vendor logos"))
}

/// Pillow's `truetype(path, size)`: `size` pixels to the em.
fn scale(font: &FontVec, size: f32) -> PxScale {
    let em = font.units_per_em().unwrap_or(1000.0);
    PxScale::from(size * font.height_unscaled() / em)
}

/// A rasterised glyph: coverage, its size and offset from the pen.
struct Mask {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    coverage: Vec<f32>,
}

const MAIN: usize = 0;
const BOLD: usize = 1;
const ICON: usize = 2;
const CAPTION: usize = 3;
const CAPTION_FALLBACK: usize = 4;
const CHIP: usize = 5;
const CHIP_FALLBACK: usize = 6;
const UI_GRID: usize = 7;

pub struct Painter {
    fonts: Vec<(FontVec, PxScale)>,
    masks: HashMap<(usize, char), Option<Rc<Mask>>>,
    cell_w: i32,
    cell_h: i32,
    baseline: i32,
    pad: i32,
    panel: (i32, i32, i32, i32),
    pub width: usize,
    pub height: usize,
    image: Vec<u8>,
}

impl Painter {
    /// `compact` trims the dead canvas around the panel, so a README-width
    /// GIF spends its pixels on the text instead of the margin.
    pub fn new(compact: bool) -> Painter {
        let mut fonts = Vec::new();
        for (font, size) in [
            (find_font(false), FONT_SIZE),
            (find_font(true), FONT_SIZE),
            (icon_font(), FONT_SIZE),
            (ui_font(), CAPTION_SIZE),
            // The keycaps in the captions are terminal glyphs; the UI font
            // has no idea what ⇥ or ⏎ are, so the monospace face stands in.
            (find_font(false), CAPTION_SIZE),
            (ui_font(), CHIP_SIZE),
            (find_font(false), CHIP_SIZE),
            (ui_font(), FONT_SIZE),
        ] {
            let px = scale(&font, size);
            fonts.push((font, px));
        }
        let (main, px) = &fonts[MAIN];
        let main = main.as_scaled(*px);
        let cell_w = main.h_advance(main.glyph_id('M')).ceil() as i32;
        let cell_h = (FONT_SIZE * LINE_RATIO).ceil() as i32;
        let pad = if compact { 20 } else { 28 };
        let margin = if compact { 24 } else { 46 };
        let strip = if compact { 76 } else { 108 };
        let panel_w = COLS as i32 * cell_w + pad * 2;
        let panel_h = ROWS as i32 * cell_h + pad * 2;
        let panel = (margin, margin, margin + panel_w, margin + panel_h);
        let width = panel_w + margin * 2;
        let height = panel_h + margin + strip;
        let (width, height) = ((width + width % 2) as usize, (height + height % 2) as usize);
        // The baseline nudge keeps glyphs optically centred in their cell.
        let ascent = main.ascent().ceil() as i32;
        let baseline = (cell_h - ascent) / 2;
        Painter {
            fonts,
            masks: HashMap::new(),
            cell_w,
            cell_h,
            baseline,
            pad,
            panel,
            width,
            height,
            image: vec![0; width * height * 3],
        }
    }

    fn covers(&self, font: usize, c: char) -> bool {
        c.is_whitespace() || self.fonts[font].0.glyph_id(c) != GlyphId(0)
    }

    fn ascent(&self, font: usize) -> f32 {
        let (f, px) = &self.fonts[font];
        f.as_scaled(*px).ascent().ceil()
    }

    fn advance(&self, font: usize, c: char) -> f32 {
        let (f, px) = &self.fonts[font];
        let f = f.as_scaled(*px);
        f.h_advance(f.glyph_id(c))
    }

    fn mask(&mut self, font: usize, c: char) -> Option<Rc<Mask>> {
        if !self.masks.contains_key(&(font, c)) {
            let (f, px) = &self.fonts[font];
            let glyph = f.glyph_id(c).with_scale_and_position(*px, ab_glyph::point(0.0, 0.0));
            let mask = f.outline_glyph(glyph).map(|outlined| {
                let bounds = outlined.px_bounds();
                let (width, height) = (bounds.width() as usize, bounds.height() as usize);
                let mut coverage = vec![0f32; width * height];
                outlined.draw(|x, y, v| {
                    if let Some(slot) = coverage.get_mut(y as usize * width + x as usize) {
                        *slot = v;
                    }
                });
                Rc::new(Mask { left: bounds.min.x as i32, top: bounds.min.y as i32, width, height, coverage })
            });
            self.masks.insert((font, c), mask);
        }
        self.masks[&(font, c)].clone()
    }

    fn blend_pixel(&mut self, x: i32, y: i32, color: Rgb, alpha: f32) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height || alpha <= 0.0 {
            return;
        }
        let at = (y as usize * self.width + x as usize) * 3;
        let alpha = alpha.min(1.0);
        for (slot, front) in self.image[at..at + 3].iter_mut().zip(color) {
            let back = *slot as f32;
            *slot = (back + (front as f32 - back) * alpha).round() as u8;
        }
    }

    /// Pillow's `rectangle`, corners inclusive.
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Rgb) {
        for y in y0.max(0)..=y1.min(self.height as i32 - 1) {
            for x in x0.max(0)..=x1.min(self.width as i32 - 1) {
                let at = (y as usize * self.width + x as usize) * 3;
                self.image[at..at + 3].copy_from_slice(&color);
            }
        }
    }

    fn rounded_fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, radius: i32, color: Rgb) {
        let r = radius as f32;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let cx = px.clamp(x0 as f32 + r, x1 as f32 + 1.0 - r);
                let cy = py.clamp(y0 as f32 + r, y1 as f32 + 1.0 - r);
                if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                    self.rect(x, y, x, y, color);
                }
            }
        }
    }

    /// Pillow's `rounded_rectangle` with an optional outline.
    fn rounded(&mut self, b: (i32, i32, i32, i32), radius: i32, fill: Rgb, outline: Option<(Rgb, i32)>) {
        match outline {
            None => self.rounded_fill(b.0, b.1, b.2, b.3, radius, fill),
            Some((edge, w)) => {
                self.rounded_fill(b.0, b.1, b.2, b.3, radius, edge);
                self.rounded_fill(b.0 + w, b.1 + w, b.2 - w, b.3 - w, (radius - w).max(0), fill);
            }
        }
    }

    /// One glyph with its pen at (`x`, `baseline_y`).
    fn glyph(&mut self, font: usize, c: char, x: i32, baseline_y: i32, color: Rgb) {
        let Some(mask) = self.mask(font, c) else { return };
        for row in 0..mask.height {
            for col in 0..mask.width {
                let alpha = mask.coverage[row * mask.width + col];
                self.blend_pixel(x + mask.left + col as i32, baseline_y + mask.top + row as i32, color, alpha);
            }
        }
    }

    /// A line in `font`, borrowing `fallback` for what it cannot render;
    /// `y` is the top of the line, as Pillow's `text()` takes it.
    fn text(&mut self, x: f32, y: i32, text: &str, font: usize, fallback: usize, color: Rgb) -> f32 {
        let mut pen = x;
        for c in text.chars() {
            let chosen = if self.covers(font, c) { font } else { fallback };
            let baseline = y + self.ascent(chosen) as i32;
            self.glyph(chosen, c, pen.round() as i32, baseline, color);
            pen += self.advance(chosen, c);
        }
        pen - x
    }

    fn text_length(&self, text: &str, font: usize, fallback: usize) -> f32 {
        text.chars().map(|c| self.advance(if self.covers(font, c) { font } else { fallback }, c)).sum()
    }

    /// A line-drawing character as lines, edge to edge of its cell.
    ///
    /// A terminal joins these into one unbroken rule; a font's glyph stops
    /// short of the line height the rows are laid out at, which would film
    /// the divider beside the preview as a dashed line.
    fn box_char(&mut self, shape: (bool, bool, bool, bool), x: i32, y: i32, color: Rgb) {
        let (up, down, across, heavy) = shape;
        let (cw, ch) = (self.cell_w, self.cell_h);
        let thin = ((ch as f32 / 20.0).round() as i32).max(1);
        let weight = if heavy { thin * 2 } else { thin };
        let (middle_x, middle_y) = (x + cw / 2, y + ch / 2);
        if across {
            let top = middle_y - thin / 2;
            self.rect(x, top, x + cw - 1, top + thin - 1, color);
        }
        if up || down {
            let left = middle_x - weight / 2;
            self.rect(
                left,
                if up { y } else { middle_y },
                left + weight - 1,
                if down { y + ch - 1 } else { middle_y },
                color,
            );
        }
    }

    pub fn render(&mut self, frame: &Frame) -> &[u8] {
        self.image.as_chunks_mut::<3>().0.fill(CANVAS_BG);
        let (left, top, right, bottom) = self.panel;
        self.rounded((left - 6, top - 6, right + 6, bottom + 6), 20, PANEL_GLOW, None);
        self.rounded((left, top, right, bottom), 15, PANEL_BG, Some((PANEL_EDGE, 2)));

        let (origin_x, origin_y) = (left + self.pad, top + self.pad);
        let (cw, ch) = (self.cell_w, self.cell_h);
        for row in 0..ROWS {
            let cells = &frame.cells[row * COLS..(row + 1) * COLS];
            let y = origin_y + row as i32 * ch;
            for (column, cell) in cells.iter().enumerate() {
                if let Some(back) = cell.bg {
                    let x = origin_x + column as i32 * cw;
                    self.rect(x, y, x + cw - 1, y + ch - 1, back);
                }
            }
            for (column, cell) in cells.iter().enumerate() {
                let x = origin_x + column as i32 * cw;
                if cell.underline {
                    // Drawn per cell so a run stays continuous across spaces.
                    let rule_y = y + ch - (ch / 12).max(2);
                    self.rect(x, rule_y, x + cw - 1, rule_y + (ch / 18).max(1), cell.fg);
                }
                if cell.ch == '\0' || cell.ch == ' ' {
                    continue;
                }
                if let Some(shape) = box_shape(cell.ch) {
                    self.box_char(shape, x, y, cell.fg);
                    continue;
                }
                if is_logo(cell.ch) {
                    // Centre the ink in its cell, whatever the icon font's
                    // own ascent and advance.
                    if let Some(mask) = self.mask(ICON, cell.ch) {
                        let (w, h, l, t) = (mask.width as i32, mask.height as i32, mask.left, mask.top);
                        self.glyph(ICON, cell.ch, x + (cw - w) / 2 - l, y + (ch - h) / 2 - t, cell.fg);
                    }
                    continue;
                }
                let font = if cell.bold { BOLD } else { MAIN };
                let font = if self.covers(font, cell.ch) {
                    font
                } else if self.covers(MAIN, cell.ch) {
                    MAIN
                } else {
                    UI_GRID
                };
                let baseline = y + self.baseline + self.ascent(MAIN) as i32;
                self.glyph(font, cell.ch, x, baseline, cell.fg);
            }
        }

        let caption_y = bottom + 30;
        if !frame.outcome.is_empty() {
            self.text((left + 4) as f32, caption_y, &frame.outcome, CAPTION, CAPTION_FALLBACK, BASE16[12]);
        } else if !frame.caption.is_empty() {
            self.text((left + 4) as f32, caption_y, &frame.caption, CAPTION, CAPTION_FALLBACK, CAPTION_FG);
        }
        if let Some(label) = &frame.chip {
            self.chip(label, frame.chip_age, right, caption_y);
        }
        &self.image
    }

    fn chip(&mut self, label: &str, age: f32, right: i32, y: i32) {
        let text_w = self.text_length(label, CHIP, CHIP_FALLBACK) as i32;
        let width = (text_w + 28).max(48);
        let height = 42;
        let b = (right - width + 4, y - 8, right + 4, y - 8 + height);
        let fade = (1.0 - (age / CHIP_TTL).powi(2)).max(0.0);
        let fill = blend(CANVAS_BG, CHIP_BG, fade);
        let edge = blend(CANVAS_BG, CHIP_EDGE, fade);
        self.rounded(b, 10, fill, Some((edge, 2)));
        let x = b.0 as f32 + (width - text_w) as f32 / 2.0;
        self.text(x, y, label, CHIP, CHIP_FALLBACK, blend(CANVAS_BG, CHIP_FG, fade));
    }
}

fn blend(back: Rgb, front: Rgb, amount: f32) -> Rgb {
    let mix = |i: usize| (back[i] as f32 + (front[i] as f32 - back[i] as f32) * amount) as u8;
    [mix(0), mix(1), mix(2)]
}
