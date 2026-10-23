//! Transcripts as text: the canonical dump and a line diff.

use crate::pyjson::{self, Value};
use std::fmt::Write as _;

/// `json.dumps(value, indent=1, ensure_ascii=False, sort_keys=True)`, as
/// parity.py's `dump` (its temp-name mask is the machine's job here).
pub fn dump(value: &Value) -> String {
    let mut out = String::new();
    write(&mut out, value, 0);
    out
}

fn newline(out: &mut String, level: usize) {
    out.push('\n');
    for _ in 0..level {
        out.push(' ');
    }
}

fn write(out: &mut String, value: &Value, level: usize) {
    match value {
        Value::List(items) if !items.is_empty() => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                write(out, item, level + 1);
            }
            newline(out, level);
            out.push(']');
        }
        Value::Dict(map) if !map.is_empty() => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            out.push('{');
            for (index, (key, item)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                newline(out, level + 1);
                write_str(out, key);
                out.push_str(": ");
                write(out, item, level + 1);
            }
            newline(out, level);
            out.push('}');
        }
        Value::List(_) => out.push_str("[]"),
        Value::Dict(_) => out.push_str("{}"),
        Value::Str(text) => write_str(out, text),
        other => out.push_str(&pyjson::dumps(other, true)),
    }
}

/// A JSON string with `ensure_ascii=False`.
fn write_str(out: &mut String, text: &str) {
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
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

/// A unified diff of two dumps, two lines of context.
pub fn diff(left_name: &str, right_name: &str, left: &str, right: &str) -> Vec<String> {
    let a: Vec<&str> = left.lines().collect();
    let b: Vec<&str> = right.lines().collect();
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix && suffix < b.len() - prefix && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    // Longest common subsequence over the differing middle.
    let (n, m) = (am.len(), bm.len());
    let mut table = vec![0u32; (n + 1) * (m + 1)];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i * (m + 1) + j] = if am[i] == bm[j] {
                table[(i + 1) * (m + 1) + j + 1] + 1
            } else {
                table[(i + 1) * (m + 1) + j].max(table[i * (m + 1) + j + 1])
            };
        }
    }
    // ops: (kind, line) with ' ', '-', '+'
    let mut ops: Vec<(char, &str)> = a[..prefix].iter().map(|l| (' ', *l)).collect();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            ops.push((' ', am[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || table[(i + 1) * (m + 1) + j] >= table[i * (m + 1) + j + 1]) {
            ops.push(('-', am[i]));
            i += 1;
        } else {
            ops.push(('+', bm[j]));
            j += 1;
        }
    }
    ops.extend(a[a.len() - suffix..].iter().map(|l| (' ', *l)));

    let mut out = Vec::new();
    if ops.iter().all(|(k, _)| *k == ' ') {
        return out;
    }
    out.push(format!("--- {}", left_name));
    out.push(format!("+++ {}", right_name));
    let context = 2;
    let mut index = 0;
    while index < ops.len() {
        if ops[index].0 == ' ' {
            index += 1;
            continue;
        }
        let start = index.saturating_sub(context);
        let mut end = index;
        let mut last_change = index;
        while end < ops.len() && end <= last_change + 2 * context {
            if ops[end].0 != ' ' {
                last_change = end;
            }
            end += 1;
        }
        let end = (last_change + context + 1).min(ops.len());
        let line_a = ops[..start].iter().filter(|(k, _)| *k != '+').count() + 1;
        let line_b = ops[..start].iter().filter(|(k, _)| *k != '-').count() + 1;
        out.push(format!("@@ -{} +{} @@", line_a, line_b));
        for (kind, line) in &ops[start..end] {
            out.push(format!("{}{}", kind, line));
        }
        index = end;
    }
    out
}
