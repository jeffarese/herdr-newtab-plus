//! Escape sequences both terminal backends send, and the OSC 11 reply parser.

pub const ALT_SCREEN_ON: &str = "\x1b[?1049h";
pub const ALT_SCREEN_OFF: &str = "\x1b[?1049l";
pub const CURSOR_HIDE: &str = "\x1b[?25l";
pub const CURSOR_SHOW: &str = "\x1b[?25h";
pub const WRAP_OFF: &str = "\x1b[?7l";
pub const WRAP_ON: &str = "\x1b[?7h";
pub const PASTE_ON: &str = "\x1b[?2004h";
pub const PASTE_OFF: &str = "\x1b[?2004l";
pub const MOUSE_ON: &str = "\x1b[?1000h\x1b[?1006h";
pub const MOUSE_OFF: &str = "\x1b[?1006l\x1b[?1000l";

/// "What is your background color?" The reply names it as rgb:RRRR/GGGG/BBBB.
pub const APPEARANCE_QUERY: &str = "\x1b]11;?\x07";

pub fn enter_sequence(mouse: bool) -> String {
    let mut sequence = format!("{}{}{}{}", ALT_SCREEN_ON, CURSOR_HIDE, WRAP_OFF, PASTE_ON);
    if mouse {
        sequence.push_str(MOUSE_ON);
    }
    sequence
}

pub fn exit_sequence(mouse: bool) -> String {
    let sequence = format!("{}{}{}{}\x1b[0m", PASTE_OFF, WRAP_ON, CURSOR_SHOW, ALT_SCREEN_OFF);
    if mouse {
        format!("{}{}", MOUSE_OFF, sequence)
    } else {
        sequence
    }
}

/// Whether an OSC reply has been terminated, by BEL or by ST.
pub fn reply_complete(buffer: &[u8]) -> bool {
    buffer.contains(&0x07) || buffer.windows(2).any(|w| w == b"\x1b\\")
}

fn hex_run(buffer: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < buffer.len() && buffer[end].is_ascii_hexdigit() {
        end += 1;
    }
    end
}

/// Read "dark" or "light" out of an OSC 11 reply; None when there is none.
pub fn appearance_from_reply(buffer: &[u8]) -> Option<&'static str> {
    let mut from = 0;
    while let Some(found) = buffer[from..].windows(4).position(|w| w == b"rgb:") {
        let start = from + found + 4;
        let mut groups: Vec<&[u8]> = Vec::new();
        let mut at = start;
        for index in 0..3 {
            let end = hex_run(buffer, at);
            if end == at {
                break;
            }
            groups.push(&buffer[at..end]);
            at = end;
            if index < 2 {
                if at < buffer.len() && buffer[at] == b'/' {
                    at += 1;
                } else {
                    break;
                }
            }
        }
        if groups.len() == 3 {
            let mut channels = [0.0f64; 3];
            for (slot, group) in channels.iter_mut().zip(&groups) {
                let mut value = 0.0f64;
                for &b in group.iter() {
                    value = value * 16.0 + (b as char).to_digit(16).unwrap() as f64;
                }
                let scale = 16f64.powi(group.len() as i32) - 1.0;
                *slot = value / scale;
            }
            let luminance = 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
            return Some(if luminance < 0.5 { "dark" } else { "light" });
        }
        from = from + found + 1;
    }
    None
}
