//! JSON the way Python's `json` module reads and writes it.
//!
//! Herdr's own responses would parse with anything, but the files this plugin
//! keeps on disk -- `recent.json`, `auto-titles.json`, `tab-status.json` --
//! were written by `json.dumps`, and the release that replaces it must read
//! every one of them back, write them byte for byte the way the old one did,
//! and treat a broken config exactly as leniently. So: Python's grammar (NaN
//! and Infinity included), Python's output format, and Python's notions of
//! truth, equality and `str()` for the values that came out of it.

use crate::pystr;
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i128),
    /// An integer too large for i128, kept as its decimal text.
    BigInt(String),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Dict(Map),
}

/// An insertion-ordered dict, as Python's are.
#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

impl Map {
    pub fn new() -> Self {
        Map { entries: Vec::new() }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// Python `d[key] = value`: replaces in place, or appends.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        if let Some(slot) = self.get_mut(&key) {
            *slot = value;
        } else {
            self.entries.push((key, value));
        }
    }

    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let index = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(index).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&str, &Value) -> bool) {
        self.entries.retain(|(k, v)| keep(k, v));
    }
}

impl FromIterator<(String, Value)> for Map {
    fn from_iter<I: IntoIterator<Item = (String, Value)>>(iter: I) -> Self {
        let mut map = Map::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl PartialEq for Map {
    /// Python dict equality: same keys, equal values, order irrelevant.
    fn eq(&self, other: &Map) -> bool {
        self.len() == other.len()
            && self.iter().all(|(k, v)| other.get(k).map(|o| py_eq(v, o)).unwrap_or(false))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Value) -> bool {
        py_eq(self, other)
    }
}

pub static NULL: Value = Value::Null;

impl Value {
    pub fn str(text: impl Into<String>) -> Value {
        Value::Str(text.into())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_dict(&self) -> Option<&Map> {
        match self {
            Value::Dict(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_dict_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Dict(m) => Some(m),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// `isinstance(value, int)` -- which bools are, in Python.
    pub fn is_int(&self) -> bool {
        matches!(self, Value::Int(_) | Value::BigInt(_) | Value::Bool(_))
    }

    /// `isinstance(value, int) and not isinstance(value, bool)` is never what
    /// the Python code asked; this is the integer value it would have used.
    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Bool(b) => Some(*b as i128),
            _ => None,
        }
    }

    /// `value is True`.
    pub fn is_true(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    /// `value is False`.
    pub fn is_false(&self) -> bool {
        matches!(self, Value::Bool(false))
    }

    /// `d.get(key)` on something that may not be a dict: None either way.
    pub fn get(&self, key: &str) -> &Value {
        match self {
            Value::Dict(m) => m.get(key).unwrap_or(&NULL),
            _ => &NULL,
        }
    }

    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::BigInt(_) => true,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Dict(m) => !m.is_empty(),
        }
    }

    /// `value or []` for something iterated as a list.
    pub fn list_or_empty(&self) -> &[Value] {
        match self {
            Value::List(l) => l,
            _ => &[],
        }
    }
}

/// Python `==` between two JSON-born values (`True == 1`, `1 == 1.0`).
pub fn py_eq(a: &Value, b: &Value) -> bool {
    use Value::*;
    fn number(v: &Value) -> Option<f64> {
        match v {
            Bool(b) => Some(*b as i32 as f64),
            Int(i) => Some(*i as f64),
            Float(f) => Some(*f),
            _ => None,
        }
    }
    match (a, b) {
        (Null, Null) => true,
        (Str(x), Str(y)) => x == y,
        (List(x), List(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Dict(x), Dict(y)) => x == y,
        (BigInt(x), BigInt(y)) => x == y,
        (Bool(x), Bool(y)) => x == y,
        (Int(x), Int(y)) => x == y,
        (Bool(x), Int(y)) | (Int(y), Bool(x)) => *x as i128 == *y,
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        },
    }
}

// -- parsing ------------------------------------------------------------------

#[derive(Debug)]
pub struct ParseError;

const MAX_DEPTH: usize = 900;
/// Arrays at least this big, this near the top, are parsed in parallel.
const SPLIT_MIN_BYTES: usize = 128 * 1024;
const SPLIT_MAX_DEPTH: usize = 4;

struct Parser<'a> {
    chars: &'a [u8],
    at: usize,
    depth: usize,
}

/// `json.loads(text)`.
pub fn loads(text: &str) -> Result<Value, ParseError> {
    let mut parser = Parser { chars: text.as_bytes(), at: 0, depth: 0 };
    parser.skip_ws();
    let value = parser.value()?;
    parser.skip_ws();
    if parser.at != parser.chars.len() {
        return Err(ParseError);
    }
    Ok(value)
}

/// `json.loads(data)` for bytes: UTF-8, with or without a BOM.
pub fn loads_bytes(data: &[u8]) -> Result<Value, ParseError> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let text = std::str::from_utf8(data).map_err(|_| ParseError)?;
    loads(text)
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while self.at < self.chars.len() && matches!(self.chars[self.at], b' ' | b'\t' | b'\n' | b'\r') {
            self.at += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.chars.get(self.at).copied()
    }

    fn literal(&mut self, word: &str) -> bool {
        if self.chars[self.at..].starts_with(word.as_bytes()) {
            self.at += word.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Value, ParseError> {
        match self.peek().ok_or(ParseError)? {
            b'"' => {
                self.at += 1;
                Ok(Value::Str(self.string()?))
            }
            b'{' => self.object(),
            b'[' => self.array(),
            b'n' if self.literal("null") => Ok(Value::Null),
            b't' if self.literal("true") => Ok(Value::Bool(true)),
            b'f' if self.literal("false") => Ok(Value::Bool(false)),
            b'N' if self.literal("NaN") => Ok(Value::Float(f64::NAN)),
            b'I' if self.literal("Infinity") => Ok(Value::Float(f64::INFINITY)),
            b'-' if self.chars[self.at..].starts_with(b"-Infinity") => {
                self.at += 9;
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(ParseError),
        }
    }

    fn number(&mut self) -> Result<Value, ParseError> {
        let start = self.at;
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek() {
            Some(b'0') => self.at += 1,
            Some(b'1'..=b'9') => {
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.at += 1;
                }
            }
            _ => return Err(ParseError),
        }
        let mut float = false;
        if self.peek() == Some(b'.') && matches!(self.chars.get(self.at + 1), Some(b'0'..=b'9')) {
            float = true;
            self.at += 1;
            while matches!(self.peek(), Some(b'0'..=b'9')) {
                self.at += 1;
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            let mut probe = self.at + 1;
            if matches!(self.chars.get(probe), Some(b'+') | Some(b'-')) {
                probe += 1;
            }
            if matches!(self.chars.get(probe), Some(b'0'..=b'9')) {
                float = true;
                self.at = probe;
                while matches!(self.peek(), Some(b'0'..=b'9')) {
                    self.at += 1;
                }
            }
        }
        let text = std::str::from_utf8(&self.chars[start..self.at]).map_err(|_| ParseError)?;
        if float {
            return text.parse::<f64>().map(Value::Float).map_err(|_| ParseError);
        }
        let digits = text.trim_start_matches('-').len();
        if digits > 4300 {
            return Err(ParseError); // Python's int_max_str_digits
        }
        match text.parse::<i128>() {
            Ok(value) => Ok(Value::Int(value)),
            Err(_) => Ok(Value::BigInt(text.to_string())),
        }
    }

    fn hex4(&mut self) -> Result<u32, ParseError> {
        let slice = self.chars.get(self.at..self.at + 4).ok_or(ParseError)?;
        let text = std::str::from_utf8(slice).map_err(|_| ParseError)?;
        if !text.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ParseError);
        }
        self.at += 4;
        u32::from_str_radix(text, 16).map_err(|_| ParseError)
    }

    fn string(&mut self) -> Result<String, ParseError> {
        // Most strings have no escapes: one exact allocation, no copying twice.
        let start = self.at;
        let mut end = start;
        while end < self.chars.len() && !matches!(self.chars[end], b'"' | b'\\' | 0..=0x1f) {
            end += 1;
        }
        if self.chars.get(end) == Some(&b'"') {
            let text = std::str::from_utf8(&self.chars[start..end]).map_err(|_| ParseError)?;
            self.at = end + 1;
            return Ok(text.to_string());
        }
        let mut out = String::new();
        loop {
            let start = self.at;
            while self.at < self.chars.len() {
                let b = self.chars[self.at];
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.at += 1;
            }
            out.push_str(std::str::from_utf8(&self.chars[start..self.at]).map_err(|_| ParseError)?);
            let b = self.peek().ok_or(ParseError)?;
            self.at += 1;
            match b {
                b'"' => return Ok(out),
                b'\\' => {
                    let escape = self.peek().ok_or(ParseError)?;
                    self.at += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\x08'),
                        b'f' => out.push('\x0c'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code)
                                && self.chars[self.at..].starts_with(b"\\u")
                            {
                                let save = self.at;
                                self.at += 2;
                                let low = self.hex4()?;
                                if (0xDC00..0xE000).contains(&low) {
                                    code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                                } else {
                                    self.at = save;
                                }
                            }
                            // A lone surrogate is a valid Python str but not a
                            // Rust one; it can only ever be shown as this.
                            out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(ParseError),
                    }
                }
                _ => return Err(ParseError), // a raw control character
            }
        }
    }

    fn enter(&mut self) -> Result<(), ParseError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(ParseError);
        }
        Ok(())
    }

    fn array(&mut self) -> Result<Value, ParseError> {
        self.enter()?;
        if self.depth <= SPLIT_MAX_DEPTH && self.chars.len() - self.at >= SPLIT_MIN_BYTES {
            if let Some(items) = self.split_array() {
                self.depth -= 1;
                return Ok(Value::List(items?));
            }
        }
        self.at += 1;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Value::List(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value()?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b']') => {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(Value::List(items));
                }
                _ => return Err(ParseError),
            }
        }
    }

    /// A big array's elements, parsed in parallel; None to parse it in order.
    ///
    /// One quick pass finds where each element ends -- the commas outside
    /// strings at the array's own level -- and the elements are then parsed
    /// by `value` on a few threads, each one exactly as the sequential loop
    /// would, starting at the same depth. If the sequential parse succeeds
    /// the boundaries are the same ones it would find, and if any element
    /// fails to parse (or does not fill its slot) so would the sequential
    /// one, so the result is the same either way. Arrays that are not big,
    /// or that the pass cannot close, go the ordinary way.
    fn split_array(&mut self) -> Option<Result<Vec<Value>, ParseError>> {
        let open = self.at;
        // Closers seen so far, so that `[ ... }` or `[{ ... ]}` cannot close.
        let mut open_brackets: Vec<u8> = Vec::new();
        let mut in_string = false;
        let mut escaped = false;
        let mut cuts: Vec<usize> = vec![open];
        let mut close = None;
        for (index, &b) in self.chars[open..].iter().enumerate() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    in_string = false;
                }
                continue;
            }
            match b {
                b'"' => in_string = true,
                b'[' => open_brackets.push(b']'),
                b'{' => open_brackets.push(b'}'),
                b']' | b'}' => {
                    if open_brackets.pop() != Some(b) {
                        // Mismatched: the sequential parser reports it.
                        return None;
                    }
                    if open_brackets.is_empty() {
                        close = Some(open + index);
                        break;
                    }
                }
                b',' if open_brackets.len() == 1 => cuts.push(open + index),
                _ => {}
            }
        }
        let close = close?;
        if close - open < SPLIT_MIN_BYTES || cuts.len() < crate::par::MIN_PARALLEL {
            return None;
        }
        cuts.push(close);
        let slots: Vec<(usize, usize)> = cuts.windows(2).map(|pair| (pair[0] + 1, pair[1])).collect();
        let (chars, depth) = (self.chars, self.depth);
        let parsed = crate::par::map_chunks(&slots, |slots| {
            let mut out = Vec::with_capacity(slots.len());
            for &(start, end) in slots {
                let mut parser = Parser { chars: &chars[..end], at: start, depth };
                parser.skip_ws();
                let value = parser.value().and_then(|value| {
                    parser.skip_ws();
                    if parser.at == end {
                        Ok(value)
                    } else {
                        Err(ParseError)
                    }
                });
                let failed = value.is_err();
                out.push(value);
                if failed {
                    break;
                }
            }
            out
        });
        self.at = close + 1;
        Some(parsed.into_iter().collect())
    }

    fn object(&mut self) -> Result<Value, ParseError> {
        self.enter()?;
        self.at += 1;
        let mut map = Map::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.at += 1;
            self.depth -= 1;
            return Ok(Value::Dict(map));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(ParseError);
            }
            self.at += 1;
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(ParseError);
            }
            self.at += 1;
            self.skip_ws();
            let value = self.value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.at += 1,
                Some(b'}') => {
                    self.at += 1;
                    self.depth -= 1;
                    return Ok(Value::Dict(map));
                }
                _ => return Err(ParseError),
            }
        }
    }
}

// -- writing ------------------------------------------------------------------

/// `json.dumps(value)`, or with `sort_keys=True`.
pub fn dumps(value: &Value, sort_keys: bool) -> String {
    let mut out = String::new();
    write_value(&mut out, value, sort_keys);
    out
}

fn write_value(out: &mut String, value: &Value, sort_keys: bool) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Int(i) => {
            let _ = write!(out, "{}", i);
        }
        Value::BigInt(text) => out.push_str(text),
        Value::Float(f) => {
            if f.is_nan() {
                out.push_str("NaN");
            } else if f.is_infinite() {
                out.push_str(if *f > 0.0 { "Infinity" } else { "-Infinity" });
            } else {
                out.push_str(&float_repr(*f));
            }
        }
        Value::Str(s) => write_string(out, s),
        Value::List(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_value(out, item, sort_keys);
            }
            out.push(']');
        }
        Value::Dict(map) => {
            out.push('{');
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            if sort_keys {
                entries.sort_by(|a, b| a.0.cmp(b.0));
            }
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                write_string(out, key);
                out.push_str(": ");
                write_value(out, item, sort_keys);
            }
            out.push('}');
        }
    }
}

/// `json.dumps(value, indent=indent)`: one item per line, "," between them.
pub fn dumps_indent(value: &Value, indent: usize) -> String {
    let mut out = String::new();
    write_indented(&mut out, value, indent, 0);
    out
}

fn newline(out: &mut String, indent: usize, level: usize) {
    out.push('\n');
    for _ in 0..indent * level {
        out.push(' ');
    }
}

fn write_indented(out: &mut String, value: &Value, indent: usize, level: usize) {
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, indent, level + 1);
                write_indented(out, item, indent, level + 1);
            }
            newline(out, indent, level);
            out.push(']');
        }
        Value::Dict(map) if !map.is_empty() => {
            out.push('{');
            for (index, (key, item)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, indent, level + 1);
                write_string(out, key);
                out.push_str(": ");
                write_indented(out, item, indent, level + 1);
            }
            newline(out, indent, level);
            out.push('}');
        }
        other => write_value(out, other, false),
    }
}

/// A JSON string with `ensure_ascii=True`, as json.dumps writes it.
pub fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0c' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || (c as u32) > 0x7e => {
                let code = c as u32;
                if code > 0xFFFF {
                    let v = code - 0x10000;
                    let _ = write!(out, "\\u{:04x}\\u{:04x}", 0xD800 + (v >> 10), 0xDC00 + (v & 0x3FF));
                } else {
                    let _ = write!(out, "\\u{:04x}", code);
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Python's `repr(float)`: the shortest digits that round-trip, laid out the
/// way CPython lays them out.
pub fn float_repr(value: f64) -> String {
    if value.is_nan() {
        return "nan".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let negative = value.is_sign_negative();
    let mut scientific = format!("{:e}", value.abs());
    // Rust's shortest digits break an exact tie upwards, CPython's to even:
    // 2215847513417901.25 is ...901.3 to one and ...901.2 to the other. The
    // correctly rounded string of the same length settles it.
    let precision = scientific.split_once('e').map_or(0, |(m, _)| m.len().saturating_sub(2));
    let even = format!("{:.*e}", precision, value.abs());
    if even != scientific && even.parse::<f64>().ok() == Some(value.abs()) {
        scientific = even;
    }
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digits = if digits.chars().all(|c| c == '0') { "0".to_string() } else { digits };
    let decpt = if digits == "0" { 1 } else { exponent + 1 };
    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if digits != "0" && (decpt <= -4 || decpt > 16) {
        out.push_str(&digits[..1]);
        if digits.len() > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        let shown = decpt - 1;
        let _ = write!(out, "e{}{:02}", if shown < 0 { '-' } else { '+' }, shown.abs());
        return out;
    }
    let length = digits.len() as i32;
    if decpt <= 0 {
        out.push_str("0.");
        for _ in 0..(-decpt) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if decpt >= length {
        out.push_str(&digits);
        for _ in 0..(decpt - length) {
            out.push('0');
        }
        out.push_str(".0");
    } else {
        out.push_str(&digits[..decpt as usize]);
        out.push('.');
        out.push_str(&digits[decpt as usize..]);
    }
    out
}

/// `str(value)`.
pub fn py_str(value: &Value) -> String {
    match value {
        Value::Str(s) => s.clone(),
        other => py_repr(other),
    }
}

/// `repr(value)`.
pub fn py_repr(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Int(i) => i.to_string(),
        Value::BigInt(text) => text.clone(),
        Value::Float(f) => float_repr(*f),
        Value::Str(s) => str_repr(s),
        Value::List(items) => {
            let parts: Vec<String> = items.iter().map(py_repr).collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Dict(map) => {
            let parts: Vec<String> =
                map.iter().map(|(k, v)| format!("{}: {}", str_repr(k), py_repr(v))).collect();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

/// `repr(text)` for a str.
pub fn str_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if pystr::is_printable(c) => out.push(c),
            c => {
                let code = c as u32;
                if code < 0x100 {
                    let _ = write!(out, "\\x{:02x}", code);
                } else if code < 0x10000 {
                    let _ = write!(out, "\\u{:04x}", code);
                } else {
                    let _ = write!(out, "\\U{:08x}", code);
                }
            }
        }
    }
    out.push(quote);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One element per index, with every kind of value and the characters
    /// a boundary scan could trip on inside strings.
    fn element(index: usize) -> String {
        format!(
            r#"{{"id": "w1:p{i}", "n": {i}, "f": {i}.5e-1, "t": true, "z": null, "s": "a,b]c}}d\"e\\", "u": "é😀", "l": [1, [2, {{"k": "]"}}], -3], "d": {{}}, "e": []}}"#,
            i = index
        )
    }

    fn big_array(count: usize) -> String {
        let items: Vec<String> = (0..count).map(element).collect();
        format!("[\n {} \n]", items.join(" ,\n "))
    }

    #[test]
    fn split_arrays_parse_as_in_order() {
        let text = format!(r#"{{"result": {{"panes": {}, "x": 1}}}}"#, big_array(3000));
        assert!(text.len() > SPLIT_MIN_BYTES);
        let parsed = loads(&text).unwrap();
        let panes = parsed.get("result").get("panes").list_or_empty();
        assert_eq!(panes.len(), 3000);
        for (index, pane) in panes.iter().enumerate() {
            assert_eq!(dumps(pane, false), dumps(&loads(&element(index)).unwrap(), false));
        }
        assert_eq!(parsed.get("result").get("x").as_int(), Some(1));
        // Anything the sequential parser refuses is still refused.
        let good = big_array(3000);
        let broken = [
            good.replacen("\"n\": 1500,", "\"n\": 1500,,", 1),
            good.replacen("\"t\": true", "\"t\": tru", 1),
            good.replacen("} ,\n {", "}  {", 1),
            format!("{},]", &good[..good.len() - 1]),
            good[..good.len() - 2].to_string(),
            good.replacen("\"e\": []}", "\"e\": []} 7", 1),
            format!("{}}}", &good[..good.len() - 1]),
            good.replacen("\"d\": {}", "\"d\": {]", 1),
            good.replacen("\"e\": []}", "\"e\": [}}", 1),
        ];
        for text in broken {
            assert!(loads(&text).is_err());
        }
    }

    #[test]
    // 2215847513417901.25 is exact in an f64; the lint misjudges it.
    #[allow(clippy::excessive_precision)]
    fn floats() {
        assert_eq!(float_repr(1.0), "1.0");
        assert_eq!(float_repr(1e16), "1e+16");
        assert_eq!(float_repr(1234.5), "1234.5");
        assert_eq!(float_repr(0.0001), "0.0001");
        assert_eq!(float_repr(0.00001), "1e-05");
        assert_eq!(float_repr(-0.0), "-0.0");
        assert_eq!(float_repr(123456789012345680.0), "1.2345678901234568e+17");
        assert_eq!(float_repr(1e15), "1000000000000000.0");
        // An exact tie in the last digit rounds to even, as CPython does.
        assert_eq!(float_repr(2215847513417901.25), "2215847513417901.2");
    }

    #[test]
    fn roundtrip() {
        let text = r#"{"a": [1, 2.5, "é\n"], "b": {"c": null, "d": true}, "a": 3}"#;
        let value = loads(text).unwrap();
        assert_eq!(dumps(&value, false), r#"{"a": 3, "b": {"c": null, "d": true}}"#);
        assert_eq!(dumps(&Value::str("é😀"), false), r#""\u00e9\ud83d\ude00""#);
    }

    #[test]
    fn equality() {
        assert!(py_eq(&Value::Bool(true), &Value::Int(1)));
        assert!(py_eq(&Value::Int(1), &Value::Float(1.0)));
        assert!(!py_eq(&Value::Null, &Value::Bool(false)));
    }
}
