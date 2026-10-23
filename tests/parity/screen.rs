//! The alt screen, and what was printed on the main one (parity.py's Screen).
//!
//! A deliberately small VT emulator: it knows what the picker draws with and
//! records everything else (private modes, OSC strings, unknown CSI) in
//! `modes`, so a build that starts emitting something new shows up there.

use crate::pyjson::{self, Value};
use crate::pystr;

#[derive(Clone, PartialEq, Default)]
struct Style {
    fg: Option<String>,
    bg: Option<String>,
    bold: bool,
    underline: bool,
    reverse: bool,
}

impl Style {
    /// `json.dumps(style)` of parity.py's `(fg, bg, bold, underline, reverse)`.
    fn json(&self) -> String {
        let text = |v: &Option<String>| v.clone().map(Value::Str).unwrap_or(Value::Null);
        pyjson::dumps(
            &Value::List(vec![
                text(&self.fg),
                text(&self.bg),
                Value::Bool(self.bold),
                Value::Bool(self.underline),
                Value::Bool(self.reverse),
            ]),
            false,
        )
    }
}

/// Columns a character takes, by Python's `unicodedata`.
fn char_width(c: char) -> usize {
    if pystr::is_combining(c) {
        return 0;
    }
    let code = c as u32;
    if code < 32 || code == 127 {
        return 0;
    }
    if pystr::is_wide(c) {
        2
    } else {
        1
    }
}

fn sgr(params: &str, style: &Style) -> Style {
    let mut s = style.clone();
    let parts: Vec<&str> = if params.is_empty() { vec!["0"] } else { params.split(';').collect() };
    let mut index = 0;
    while index < parts.len() {
        let part = if parts[index].is_empty() { "0" } else { parts[index] };
        if part == "38" || part == "48" {
            let mode = parts.get(index + 1).copied().unwrap_or("");
            let length = if mode == "2" { 3 } else { 1 };
            let value = parts[index..(index + 2 + length).min(parts.len())].join(";");
            if part == "38" {
                s.fg = Some(value);
            } else {
                s.bg = Some(value);
            }
            index += 2 + length;
            continue;
        }
        match part {
            "0" => s = Style::default(),
            "1" => s.bold = true,
            "4" => s.underline = true,
            "7" => s.reverse = true,
            "22" => s.bold = false,
            "24" => s.underline = false,
            "27" => s.reverse = false,
            "39" => s.fg = None,
            "49" => s.bg = None,
            _ => {}
        }
        index += 1;
    }
    s
}

/// `int(part) if part.isdigit() else 0`, saturating instead of growing.
fn number(part: &str) -> usize {
    if part.is_empty() || !part.chars().all(pystr::is_digit) {
        return 0;
    }
    part.chars().fold(0usize, |n, c| n.saturating_mul(10).saturating_add(pystr::decimal_value(c).unwrap_or(0) as usize))
}

pub struct Screen {
    width: usize,
    height: usize,
    /// None is the second half of a wide character ("" in parity.py).
    cells: Vec<Vec<(Option<char>, Style)>>,
    x: usize,
    y: usize,
    style: Style,
    alt: bool,
    pub main_text: String,
    pub modes: Vec<String>,
    /// An escape sequence cut off at the end of the last feed.
    pending: String,
}

fn blank(width: usize, height: usize) -> Vec<Vec<(Option<char>, Style)>> {
    vec![vec![(Some(' '), Style::default()); width]; height]
}

impl Screen {
    pub fn new(width: usize, height: usize) -> Screen {
        Screen {
            width,
            height,
            cells: blank(width, height),
            x: 0,
            y: 0,
            style: Style::default(),
            alt: false,
            main_text: String::new(),
            modes: Vec::new(),
            pending: String::new(),
        }
    }

    pub fn resize(&mut self, width: usize, height: usize) {
        let mut cells = blank(width, height);
        for (y, row) in cells.iter_mut().enumerate().take(height.min(self.height)) {
            for (x, cell) in row.iter_mut().enumerate().take(width.min(self.width)) {
                *cell = self.cells[y][x].clone();
            }
        }
        (self.cells, self.width, self.height) = (cells, width, height);
    }

    pub fn feed(&mut self, text: &str) {
        let data: Vec<char> = std::mem::take(&mut self.pending).chars().chain(text.chars()).collect();
        let length = data.len();
        let mut index = 0;
        while index < length {
            let c = data[index];
            if c != '\x1b' {
                if !self.alt {
                    self.main_text.push(c);
                } else if c == '\n' {
                    self.y = self.y.saturating_add(1);
                } else if c == '\r' {
                    self.x = 0;
                } else {
                    self.put(c);
                }
                index += 1;
                continue;
            }
            if index + 1 >= length {
                self.pending = data[index..].iter().collect();
                break;
            }
            let introducer = data[index + 1];
            if introducer == '[' {
                let mut end = index + 2;
                while end < length && !('\x40'..='\x7e').contains(&data[end]) {
                    end += 1;
                }
                if end >= length {
                    self.pending = data[index..].iter().collect();
                    break;
                }
                let params: String = data[index + 2..end].iter().collect();
                self.csi(&params, data[end]);
                index = end + 1;
                continue;
            }
            if introducer == ']' {
                let bel = (index..length).find(|&i| data[i] == '\x07');
                let st = (index..length.saturating_sub(1)).find(|&i| data[i] == '\x1b' && data[i + 1] == '\\');
                let end = match (bel, st) {
                    (Some(b), Some(s)) if b < s => (b, 1),
                    (_, Some(s)) => (s, 2),
                    (Some(b), None) => (b, 1),
                    (None, None) => {
                        self.pending = data[index..].iter().collect();
                        break;
                    }
                };
                let body: String = data[index + 2..end.0].iter().collect();
                self.modes.push(format!("osc:{}", body));
                index = end.0 + end.1;
                continue;
            }
            index += 2;
        }
    }

    fn put(&mut self, c: char) {
        let width = char_width(c);
        if width == 0 || self.y >= self.height || self.x >= self.width {
            return;
        }
        self.cells[self.y][self.x] = (Some(c), self.style.clone());
        if width == 2 && self.x + 1 < self.width {
            self.cells[self.y][self.x + 1] = (None, self.style.clone());
        }
        self.x += width;
    }

    fn clear(&mut self, y: usize, start: usize, stop: usize) {
        if y < self.height {
            for x in start..stop.min(self.width) {
                self.cells[y][x] = (Some(' '), Style::default());
            }
        }
    }

    fn csi(&mut self, params: &str, fin: char) {
        if params.starts_with('?') && (fin == 'h' || fin == 'l') {
            self.modes.push(format!("{}{}", params, fin));
            if params == "?1049" {
                self.alt = fin == 'h';
                if self.alt {
                    for y in 0..self.height {
                        self.clear(y, 0, self.width);
                    }
                }
            }
            return;
        }
        let numbers: Vec<usize> = params.split(';').map(number).collect();
        match fin {
            'm' => self.style = sgr(params, &self.style),
            'H' | 'f' => {
                self.y = numbers.first().copied().filter(|n| *n != 0).unwrap_or(1) - 1;
                self.x = numbers.get(1).copied().filter(|n| *n != 0).unwrap_or(1) - 1;
            }
            'K' => match numbers[0] {
                0 => self.clear(self.y, self.x, self.width),
                1 => self.clear(self.y, 0, self.x.saturating_add(1)),
                _ => self.clear(self.y, 0, self.width),
            },
            'J' => {
                if numbers[0] == 0 {
                    self.clear(self.y, self.x, self.width);
                    for y in self.y.saturating_add(1)..self.height {
                        self.clear(y, 0, self.width);
                    }
                } else {
                    for y in 0..self.height {
                        self.clear(y, 0, self.width);
                    }
                }
            }
            'C' => self.x = self.x.saturating_add(if numbers[0] != 0 { numbers[0] } else { 1 }),
            _ => self.modes.push(format!("csi:{}{}", params, fin)),
        }
    }

    /// One cell, for the demo recorder: the character (None for the second
    /// half of a wide one), then fg, bg, bold, underline and reverse. Colors
    /// are the SGR parameters as written, e.g. `38;5;237` or `48;2;1;2;3`.
    #[allow(dead_code)]
    pub fn cell(&self, y: usize, x: usize) -> (Option<char>, Option<&str>, Option<&str>, bool, bool, bool) {
        let (c, s) = &self.cells[y][x];
        (*c, s.fg.as_deref(), s.bg.as_deref(), s.bold, s.underline, s.reverse)
    }

    /// Each row as its text, then its style runs.
    pub fn dump(&self) -> Vec<String> {
        let default = Style::default();
        self.cells
            .iter()
            .map(|row| {
                let text: String = row.iter().filter_map(|(c, _)| *c).collect();
                let mut runs = Vec::new();
                let mut start = 0;
                for x in 1..=row.len() {
                    if x == row.len() || row[x].1 != row[start].1 {
                        let style = &row[start].1;
                        if *style != default {
                            runs.push(format!("{}-{}:{}", start, x, style.json()));
                        }
                        start = x;
                    }
                }
                let mut line = pystr::rstrip(&text).to_string();
                if !runs.is_empty() {
                    line.push_str("  ‖ ");
                    line.push_str(&runs.join(" "));
                }
                line
            })
            .collect()
    }
}
