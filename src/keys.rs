//! Decode terminal input bytes into key, text, paste and mouse events.
//!
//! Herdr writes ordinary xterm-style sequences into the popup's pty, so this
//! is a standard CSI parser plus incremental UTF-8 decoding. A lone ESC is
//! ambiguous until the next byte arrives; `flush()` resolves it after the
//! caller's timeout.

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Key(String),
    Text(String),
    Paste(String),
    Mouse { kind: MouseKind, x: i128, y: i128 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MouseKind {
    Press,
    Release,
    WheelUp,
    WheelDown,
}

fn csi_final(c: u8) -> Option<&'static str> {
    Some(match c {
        b'A' => "up",
        b'B' => "down",
        b'C' => "right",
        b'D' => "left",
        b'H' => "home",
        b'F' => "end",
        b'Z' => "shift+tab",
        _ => return None,
    })
}

/// Function keys arrive as SS3 on most terminals and as CSI ~ on the rest. Only
/// the SS3 branch reads these: CSI P is a terminal's own command, never a key.
fn ss3_final(c: u8) -> Option<&'static str> {
    Some(match c {
        b'P' => "f1",
        b'Q' => "f2",
        b'R' => "f3",
        b'S' => "f4",
        other => return csi_final(other),
    })
}

fn csi_tilde(base: &str) -> Option<&'static str> {
    Some(match base {
        "11" => "f1",
        "12" => "f2",
        "13" => "f3",
        "14" => "f4",
        "1" => "home",
        "2" => "insert",
        "3" => "delete",
        "4" => "end",
        "5" => "pgup",
        "6" => "pgdn",
        "7" => "home",
        "8" => "end",
        _ => return None,
    })
}

fn ctrl_name(byte: u8) -> String {
    match byte {
        0x00 => "ctrl+space".into(),
        0x08 => "backspace".into(),
        0x09 => "tab".into(),
        0x0A => "enter".into(),
        0x0D => "enter".into(),
        0x1B => "esc".into(),
        0x7F => "backspace".into(),
        other => format!("ctrl+{}", (other + 0x60) as char),
    }
}

fn modifier_prefix(modifier: u128) -> &'static str {
    match modifier {
        2 => "shift+",
        3 => "alt+",
        4 => "alt+shift+",
        5 => "ctrl+",
        6 => "ctrl+shift+",
        7 => "ctrl+alt+",
        8 => "ctrl+alt+shift+",
        _ => "",
    }
}

#[derive(Default)]
pub struct KeyDecoder {
    buffer: Vec<u8>,
    paste: Option<Vec<u8>>,
}

impl KeyDecoder {
    pub fn new() -> KeyDecoder {
        KeyDecoder::default()
    }

    pub fn feed(&mut self, data: &[u8]) -> Vec<Event> {
        self.buffer.extend_from_slice(data);
        let mut events = Vec::new();
        while !self.buffer.is_empty() {
            let (consumed, event) = self.step();
            if consumed == 0 {
                break;
            }
            self.buffer.drain(..consumed);
            if let Some(event) = event {
                events.push(event);
            }
        }
        events
    }

    pub fn pending_escape(&self) -> bool {
        self.buffer == b"\x1b" || self.buffer == b"\x1bO"
    }

    /// Resolve a dangling escape once the caller decides it stands alone.
    ///
    /// `ESC O` is the front of a function key that arrived split across two
    /// reads, and it is also alt+O. Waiting is what makes the first one work;
    /// the timeout is what stops the second one being swallowed.
    pub fn flush(&mut self) -> Vec<Event> {
        if self.buffer == b"\x1b" {
            self.buffer.clear();
            return vec![Event::Key("esc".into())];
        }
        if self.buffer == b"\x1bO" {
            self.buffer.clear();
            return vec![Event::Key("alt+O".into())];
        }
        Vec::new()
    }

    fn step(&mut self) -> (usize, Option<Event>) {
        if self.paste.is_some() {
            return self.step_paste();
        }
        let buffer = &self.buffer;
        let first = buffer[0];
        if first == 0x1B {
            if buffer.len() == 1 {
                return (0, None);
            }
            let second = buffer[1];
            if second == 0x5B {
                return self.step_csi();
            }
            if second == 0x4F {
                // SS3: cursor and function keys. Split across reads: wait, or flush().
                if buffer.len() < 3 {
                    return (0, None);
                }
                return match ss3_final(buffer[2]) {
                    Some(name) => (3, Some(Event::Key(name.into()))),
                    None => (3, None),
                };
            }
            if second == 0x5D {
                return self.step_osc();
            }
            if second == 0x1B {
                return (1, Some(Event::Key("esc".into())));
            }
            let (text, size) = decode_text(&buffer[1..]);
            if size == 0 {
                return (0, None);
            }
            return match text {
                Some(text) => (1 + size, Some(Event::Key(format!("alt+{}", text)))),
                None => (1 + size, None),
            };
        }
        if first < 0x20 || first == 0x7F {
            return (1, Some(Event::Key(ctrl_name(first))));
        }
        let (text, size) = decode_text(buffer);
        if size == 0 {
            return (0, None);
        }
        (size, text.map(Event::Text))
    }

    fn step_csi(&mut self) -> (usize, Option<Event>) {
        let buffer = &self.buffer;
        let mut index = 2;
        while index < buffer.len() && (0x30..=0x3F).contains(&buffer[index]) {
            index += 1;
        }
        while index < buffer.len() && (0x20..=0x2F).contains(&buffer[index]) {
            index += 1;
        }
        if index >= buffer.len() {
            return (0, None);
        }
        let final_byte = buffer[index];
        let params: String = buffer[2..index].iter().map(|&b| b as char).collect();
        let length = index + 1;

        if params.starts_with('<') && (final_byte == b'M' || final_byte == b'm') {
            return (length, mouse_event(&params[1..], final_byte));
        }
        if final_byte == b'~' {
            if params == "200" {
                self.paste = Some(Vec::new());
                return (length, None);
            }
            let (base, modifier) = split_params(&params);
            return match csi_tilde(&base) {
                Some(name) => (length, Some(Event::Key(format!("{}{}", modifier_prefix(modifier), name)))),
                None => (length, None),
            };
        }
        let Some(name) = csi_final(final_byte) else { return (length, None) };
        let (_, modifier) = split_params(&params);
        let name =
            if name == "shift+tab" { name.to_string() } else { format!("{}{}", modifier_prefix(modifier), name) };
        (length, Some(Event::Key(name)))
    }

    fn step_osc(&mut self) -> (usize, Option<Event>) {
        let buffer = &self.buffer;
        let bel = buffer[2..].iter().position(|&b| b == 0x07).map(|p| p + 2);
        let st = buffer[2..].windows(2).position(|w| w == b"\x1b\\").map(|p| p + 2);
        if let Some(bel) = bel {
            if st.map(|s| bel < s).unwrap_or(true) {
                return (bel + 1, None);
            }
        }
        if let Some(st) = st {
            return (st + 2, None);
        }
        if buffer.len() > 512 {
            return (2, None);
        }
        (0, None)
    }

    fn step_paste(&mut self) -> (usize, Option<Event>) {
        let buffer = &self.buffer;
        let end = buffer.windows(6).position(|w| w == b"\x1b[201~");
        let paste = self.paste.as_mut().unwrap();
        match end {
            None => {
                let keep = buffer.len().saturating_sub(5);
                paste.extend_from_slice(&buffer[..keep]);
                (keep, None)
            }
            Some(end) => {
                paste.extend_from_slice(&buffer[..end]);
                let text = String::from_utf8_lossy(paste).into_owned();
                self.paste = None;
                (end + 6, Some(Event::Paste(text)))
            }
        }
    }
}

fn decode_text(data: &[u8]) -> (Option<String>, usize) {
    let first = data[0];
    let size = if first < 0x80 {
        1
    } else if first >= 0xF0 {
        4
    } else if first >= 0xE0 {
        3
    } else if first >= 0xC0 {
        2
    } else {
        return (None, 1); // stray continuation byte
    };
    if data.len() < size {
        return (None, 0);
    }
    match std::str::from_utf8(&data[..size]) {
        Ok(text) => (Some(text.to_string()), size),
        Err(_) => (None, 1),
    }
}

fn digits_value(text: &str) -> Option<u128> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(text.parse::<u128>().unwrap_or(u128::MAX))
}

fn split_params(params: &str) -> (String, u128) {
    let parts: Vec<&str> = params.split(';').collect();
    let base = if parts[0].is_empty() { "1".to_string() } else { parts[0].to_string() };
    let mut modifier = 0;
    if parts.len() > 1 {
        if let Some(value) = digits_value(parts[1]) {
            modifier = value;
        }
    }
    (base, modifier)
}

fn mouse_event(params: &str, final_byte: u8) -> Option<Event> {
    let fields: Vec<&str> = params.split(';').collect();
    if fields.len() != 3 {
        return None;
    }
    // int(): trailing intermediate bytes such as a space are allowed.
    let parse = crate::pystr::parse_int;
    let button = parse(fields[0])?;
    let column = parse(fields[1])?;
    let row = parse(fields[2])?;
    if button & 64 != 0 {
        let kind = if button & 1 == 0 { MouseKind::WheelUp } else { MouseKind::WheelDown };
        return Some(Event::Mouse { kind, x: column, y: row });
    }
    if button & 32 != 0 {
        return None;
    }
    if final_byte == b'm' {
        return Some(Event::Mouse { kind: MouseKind::Release, x: column, y: row });
    }
    if button & 3 == 0 {
        return Some(Event::Mouse { kind: MouseKind::Press, x: column, y: row });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(s: &str) -> Event {
        Event::Key(s.into())
    }

    fn text(s: &str) -> Event {
        Event::Text(s.into())
    }

    fn mouse_kinds(events: &[Event]) -> Vec<MouseKind> {
        events
            .iter()
            .map(|e| match e {
                Event::Mouse { kind, .. } => *kind,
                other => panic!("not a mouse event: {other:?}"),
            })
            .collect()
    }

    #[test]
    fn plain_text() {
        assert_eq!(KeyDecoder::new().feed(b"hi"), vec![text("h"), text("i")]);
    }

    #[test]
    fn control_keys() {
        assert_eq!(
            KeyDecoder::new().feed(b"\r\t\x7f\x15\x0f"),
            vec![key("enter"), key("tab"), key("backspace"), key("ctrl+u"), key("ctrl+o")]
        );
    }

    #[test]
    fn line_feed_is_enter_and_ctrl_k_is_itself() {
        assert_eq!(KeyDecoder::new().feed(b"\x0a\x0b"), vec![key("enter"), key("ctrl+k")]);
    }

    #[test]
    fn function_keys_in_both_spellings() {
        assert_eq!(KeyDecoder::new().feed(b"\x1bOP\x1b[11~\x1bOS"), vec![key("f1"), key("f1"), key("f4")]);
    }

    #[test]
    fn a_function_key_split_across_reads_is_not_alt_o() {
        let mut decoder = KeyDecoder::new();
        assert_eq!(decoder.feed(b"\x1bO"), vec![]);
        assert!(decoder.pending_escape());
        assert_eq!(decoder.feed(b"P"), vec![key("f1")]);
    }

    #[test]
    fn a_lone_escape_o_flushes_as_alt_o() {
        let mut decoder = KeyDecoder::new();
        decoder.feed(b"\x1bO");
        assert_eq!(decoder.flush(), vec![key("alt+O")]);
    }

    #[test]
    fn arrows_and_navigation() {
        let data = b"\x1b[A\x1b[B\x1b[C\x1b[D\x1b[5~\x1b[6~\x1b[H\x1b[F\x1b[3~\x1b[Z";
        let expected: Vec<Event> =
            ["up", "down", "right", "left", "pgup", "pgdn", "home", "end", "delete", "shift+tab"]
                .iter()
                .map(|k| key(k))
                .collect();
        assert_eq!(KeyDecoder::new().feed(data), expected);
    }

    #[test]
    fn application_cursor_keys() {
        assert_eq!(KeyDecoder::new().feed(b"\x1bOA"), vec![key("up")]);
    }

    #[test]
    fn modified_arrows() {
        assert_eq!(KeyDecoder::new().feed(b"\x1b[1;5A"), vec![key("ctrl+up")]);
    }

    #[test]
    fn escape_needs_a_flush() {
        let mut decoder = KeyDecoder::new();
        assert_eq!(decoder.feed(b"\x1b"), vec![]);
        assert!(decoder.pending_escape());
        assert_eq!(decoder.flush(), vec![key("esc")]);
    }

    #[test]
    fn escape_followed_by_a_letter_is_alt() {
        assert_eq!(KeyDecoder::new().feed(b"\x1bx"), vec![key("alt+x")]);
    }

    #[test]
    fn sequence_split_across_reads() {
        let mut decoder = KeyDecoder::new();
        assert_eq!(decoder.feed(b"\x1b["), vec![]);
        assert_eq!(decoder.feed(b"B"), vec![key("down")]);
    }

    #[test]
    fn utf8_split_across_reads() {
        let mut decoder = KeyDecoder::new();
        assert_eq!(decoder.feed(b"\xc3"), vec![]);
        assert_eq!(decoder.feed(b"\xa9"), vec![text("é")]);
    }

    #[test]
    fn wide_characters() {
        assert_eq!(KeyDecoder::new().feed("日".as_bytes()), vec![text("日")]);
    }

    #[test]
    fn bracketed_paste() {
        assert_eq!(KeyDecoder::new().feed(b"\x1b[200~one two\x1b[201~"), vec![Event::Paste("one two".into())]);
    }

    #[test]
    fn paste_split_across_reads() {
        let mut decoder = KeyDecoder::new();
        assert_eq!(decoder.feed(b"\x1b[200~par"), vec![]);
        assert_eq!(decoder.feed(b"tial\x1b[201~"), vec![Event::Paste("partial".into())]);
    }

    #[test]
    fn mouse_press_and_wheel() {
        let events = KeyDecoder::new().feed(b"\x1b[<0;12;5M\x1b[<64;1;1M\x1b[<65;1;1M");
        assert_eq!(mouse_kinds(&events), vec![MouseKind::Press, MouseKind::WheelUp, MouseKind::WheelDown]);
        assert_eq!(events[0], Event::Mouse { kind: MouseKind::Press, x: 12, y: 5 });
    }

    #[test]
    fn osc_replies_are_swallowed() {
        assert_eq!(KeyDecoder::new().feed(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x07k"), vec![text("k")]);
    }

    #[test]
    fn osc_reply_with_string_terminator() {
        assert_eq!(KeyDecoder::new().feed(b"\x1b]11;rgb:0/0/0\x1b\\z"), vec![text("z")]);
    }

    #[test]
    fn unknown_sequences_are_ignored() {
        assert_eq!(KeyDecoder::new().feed(b"\x1b[?25h"), vec![]);
    }
}
