//! Python's `str` semantics, for a port that has to agree with it.
//!
//! The Python release measured, split, folded and compared text with CPython's
//! own Unicode rules, and every one of those answers shows up somewhere a user
//! can see it: in a column width, a highlight, a sort order. So the predicates
//! here read tables generated once from CPython itself (see the header of
//! `src/unicode_tables.rs`) rather than Rust's, which follow a different
//! Unicode release and, for "space" and "digit", a different idea of what
//! those words mean.
//!
//! Strings that Python indexes by code point are handled as `&[char]`.

use crate::unicode_tables as t;

fn in_ranges(table: &[(u32, u32)], c: char) -> bool {
    let code = c as u32;
    table
        .binary_search_by(|&(low, high)| {
            if high < code {
                std::cmp::Ordering::Less
            } else if low > code {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn mapped(table: &'static [(u32, &'static str)], c: char) -> Option<&'static str> {
    table.binary_search_by_key(&(c as u32), |&(code, _)| code).ok().map(|index| table[index].1)
}

pub fn is_space(c: char) -> bool {
    if c.is_ascii() {
        return matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c' | '\x1c'..='\x1f');
    }
    in_ranges(t::SPACE, c)
}

pub fn is_digit(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_digit();
    }
    in_ranges(t::DIGIT, c)
}

/// The value of a decimal digit, as `int()` reads it.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return Some(c as u32 - '0' as u32);
    }
    let table = t::DECIMAL_ZERO;
    table.binary_search_by_key(&(c as u32), |&(code, _)| code).ok().map(|index| c as u32 - table[index].1)
}

pub fn is_alpha(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_alphabetic();
    }
    in_ranges(t::ALPHA, c)
}

pub fn is_lower(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_lowercase();
    }
    in_ranges(t::LOWER, c)
}

pub fn is_upper(c: char) -> bool {
    if c.is_ascii() {
        return c.is_ascii_uppercase();
    }
    in_ranges(t::UPPER, c)
}

pub fn is_title(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    in_ranges(t::TITLE, c)
}

fn is_cased(c: char) -> bool {
    is_lower(c) || is_upper(c) || is_title(c)
}

fn is_case_ignorable(c: char) -> bool {
    in_ranges(t::CASE_IGNORABLE, c)
}

pub fn is_printable(c: char) -> bool {
    if c.is_ascii() {
        return (' '..='~').contains(&c);
    }
    in_ranges(t::PRINTABLE, c)
}

pub fn is_combining(c: char) -> bool {
    if (c as u32) < 0x300 {
        return false;
    }
    in_ranges(t::COMBINING, c)
}

pub fn is_wide(c: char) -> bool {
    if (c as u32) < 0x1100 {
        return false;
    }
    in_ranges(t::WIDE, c)
}

/// `str.isprintable()` over a whole string, with `str.isascii()` alongside.
pub fn is_ascii_printable(text: &str) -> bool {
    text.bytes().all(|b| (0x20..0x7f).contains(&b))
}

/// `str.islower()`: at least one cased character, and none of them upper.
pub fn str_islower(text: &str) -> bool {
    let mut cased = false;
    for c in text.chars() {
        if is_upper(c) || is_title(c) {
            return false;
        }
        if !cased && is_lower(c) {
            cased = true;
        }
    }
    cased
}

/// `str.isdigit()`: non-empty and every character a digit.
pub fn str_isdigit(text: &str) -> bool {
    !text.is_empty() && text.chars().all(is_digit)
}

/// `str.lower()`, final sigma included.
pub fn lower(text: &str) -> String {
    if text.is_ascii() {
        return text.to_ascii_lowercase();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (index, &c) in chars.iter().enumerate() {
        if c == '\u{3A3}' {
            out.push(if final_sigma(&chars, index) { '\u{3C2}' } else { '\u{3C3}' });
            continue;
        }
        push_lower(&mut out, c);
    }
    out
}

/// One character's lowercase mapping on its own, which is what Python's
/// `char.lower()` gives for a one-character string.
pub fn lower_char(c: char) -> String {
    let mut out = String::new();
    push_lower(&mut out, c);
    out
}

fn push_lower(out: &mut String, c: char) {
    if c.is_ascii() {
        out.push(c.to_ascii_lowercase());
    } else if let Some(text) = mapped(t::LOWER_MAP, c) {
        out.push_str(text);
    } else {
        out.push(c);
    }
}

fn final_sigma(chars: &[char], index: usize) -> bool {
    // CPython's handle_capital_sigma: a cased letter before (skipping case
    // ignorables), and no cased letter after.
    let mut j = index;
    let mut before = false;
    while j > 0 {
        j -= 1;
        let c = chars[j];
        if is_case_ignorable(c) {
            continue;
        }
        before = is_cased(c);
        break;
    }
    if !before {
        return false;
    }
    let mut k = index + 1;
    while k < chars.len() {
        let c = chars[k];
        if is_case_ignorable(c) {
            k += 1;
            continue;
        }
        return !is_cased(c);
    }
    true
}

/// `str.strip()` with no arguments.
pub fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

pub fn rstrip(text: &str) -> &str {
    text.trim_end_matches(is_space)
}

/// `str.split()` with no arguments: runs of whitespace, no empty parts.
pub fn split_whitespace(text: &str) -> Vec<&str> {
    text.split(is_space).filter(|part| !part.is_empty()).collect()
}

/// `str.splitlines()`.
pub fn splitlines(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut iter = text.char_indices().peekable();
    while let Some((index, c)) = iter.next() {
        let boundary =
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}');
        if !boundary {
            continue;
        }
        out.push(&text[start..index]);
        let mut end = index + c.len_utf8();
        if c == '\r' {
            if let Some(&(next_index, '\n')) = iter.peek() {
                iter.next();
                end = next_index + 1;
            }
        }
        start = end;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

/// `int(text)` for a string: optional whitespace and sign, Unicode decimal
/// digits, single underscores between digits. None where Python raises;
/// saturated at the i128 bounds where Python would keep going.
pub fn parse_int(text: &str) -> Option<i128> {
    let text = strip(text);
    let mut chars = text.chars().peekable();
    let mut negative = false;
    if let Some(&sign) = chars.peek() {
        if sign == '+' || sign == '-' {
            negative = sign == '-';
            chars.next();
        }
    }
    let mut value: i128 = 0;
    let mut digits = 0;
    let mut previous_underscore = false;
    for c in chars {
        if c == '_' {
            if digits == 0 || previous_underscore {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        let digit = decimal_value(c)?;
        previous_underscore = false;
        digits += 1;
        // Past i128 the value saturates: every caller compares it against
        // small bounds, which a saturated value fails the way Python's would.
        value = value.saturating_mul(10).saturating_add(digit as i128);
    }
    // Python refuses to convert more than 4300 digits.
    if digits == 0 || previous_underscore || digits > 4300 {
        return None;
    }
    Some(if negative { -value } else { value })
}

/// `int(text, base)`: like `parse_int`, with letters for digits past nine
/// and an optional `0x`/`0o`/`0b` prefix matching the base.
pub fn parse_int_base(text: &str, base: u32) -> Option<i128> {
    let text = strip(text);
    let mut rest = text;
    let mut negative = false;
    if let Some(stripped) = rest.strip_prefix('+') {
        rest = stripped;
    } else if let Some(stripped) = rest.strip_prefix('-') {
        rest = stripped;
        negative = true;
    }
    let prefix = match base {
        16 => Some(["0x", "0X"]),
        8 => Some(["0o", "0O"]),
        2 => Some(["0b", "0B"]),
        _ => None,
    };
    let mut after_prefix = false;
    if let Some([a, b]) = prefix {
        if let Some(stripped) = rest.strip_prefix(a).or_else(|| rest.strip_prefix(b)) {
            rest = stripped;
            after_prefix = true;
        }
    }
    let mut value: i128 = 0;
    let mut digits = 0;
    let mut previous_underscore = false;
    for c in rest.chars() {
        if c == '_' {
            if previous_underscore || (digits == 0 && !after_prefix) {
                return None;
            }
            previous_underscore = true;
            continue;
        }
        let digit = if let Some(d) = decimal_value(c) {
            d
        } else if c.is_ascii_alphabetic() {
            c.to_ascii_lowercase() as u32 - 'a' as u32 + 10
        } else {
            return None;
        };
        if digit >= base {
            return None;
        }
        previous_underscore = false;
        digits += 1;
        value = value.checked_mul(base as i128)?.checked_add(digit as i128)?;
    }
    if digits == 0 || previous_underscore {
        return None;
    }
    Some(if negative { -value } else { value })
}

/// `float(text)` for a str: Python's spelling rules, None where it raises.
///
/// Surrounding whitespace, a sign, `inf`/`infinity`/`nan` in any case,
/// underscores between digits, and any Unicode decimal digit, as CPython
/// accepts them.
pub fn parse_float(text: &str) -> Option<f64> {
    let trimmed = strip(text);
    if trimmed.is_empty() {
        return None;
    }
    let mut ascii = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            ascii.push(char::from_digit(decimal_value(c)?, 10)?);
        }
    }
    let bytes = ascii.as_bytes();
    let mut cleaned = String::with_capacity(ascii.len());
    for (index, &b) in bytes.iter().enumerate() {
        if b == b'_' {
            let before = index > 0 && bytes[index - 1].is_ascii_digit();
            let after = bytes.get(index + 1).map(|n| n.is_ascii_digit()).unwrap_or(false);
            if !before || !after {
                return None;
            }
            continue;
        }
        cleaned.push(b as char);
    }
    let body = cleaned.strip_prefix(['+', '-']).unwrap_or(&cleaned);
    let lowered = body.to_ascii_lowercase();
    if matches!(lowered.as_str(), "inf" | "infinity" | "nan") {
        let value = if lowered == "nan" { f64::NAN } else { f64::INFINITY };
        return Some(if cleaned.starts_with('-') { -value } else { value });
    }
    // Rust's parser also reads "inf" spellings and nothing Python rejects
    // beyond those, once they are handled above: digits, one point, an
    // exponent.
    if !body.bytes().all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-')) {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigma() {
        assert_eq!(lower("ΑΣ"), "ας");
        assert_eq!(lower("Σ"), "σ");
        assert_eq!(lower("ΑΣΑ"), "ασα");
    }

    #[test]
    fn lines() {
        assert_eq!(splitlines("a\r\nb\rc\n"), vec!["a", "b", "c"]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn ints() {
        assert_eq!(parse_int(" 1_000 "), Some(1000));
        assert_eq!(parse_int("-٣"), Some(-3));
        assert_eq!(parse_int("1__0"), None);
        assert_eq!(parse_int("²"), None);
    }
}
