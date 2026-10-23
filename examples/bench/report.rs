//! Statistics, the report and the saved results, laid out exactly as
//! bench_process.py laid them out, so its saved files compare with these.

use crate::pyjson::{self, Map, Value};

/// The workloads: name, folders under the root, panes in the session,
/// recents, zoxide entries.
pub const WORKLOADS: [(&str, usize, usize, usize, usize); 3] =
    [("small", 12, 2, 6, 0), ("medium", 400, 32, 40, 120), ("large", 4000, 200, 80, 400)];

const ROWS: [(&str, &str, &str); 17] = [
    ("first_frame_p50_ms", "first frame p50", "ms"),
    ("first_frame_p90_ms", "first frame p90", "ms"),
    ("keystroke_p50_ms", "keystroke p50", "ms"),
    ("keystroke_p99_ms", "keystroke p99", "ms"),
    ("nav_p50_ms", "navigation p50", "ms"),
    ("nav_p99_ms", "navigation p99", "ms"),
    ("path_p50_ms", "path keystroke p50", "ms"),
    ("path_max_p50_ms", "listing keystroke", "ms"),
    ("exit_p50_ms", "esc to exit", "ms"),
    ("create_p50_ms", "enter to create", "ms"),
    ("create_exit_p50_ms", "enter to exit", "ms"),
    ("agent_p50_ms", "enter to agent start", "ms"),
    ("cpu_p50_ms", "cpu per session", "ms"),
    ("peak_p50_kb", "peak memory", "KB"),
    ("bytes_p50", "bytes per session", "B"),
    ("herdr_calls_p50", "herdr calls", ""),
    ("list_p50_ms", "--list", "ms"),
];

/// The value at `fraction` of the sorted values (Python's `round`, which
/// breaks ties to even, picks the index), or 0 when there are none.
pub fn percentile(values: &[f64], fraction: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut ordered = values.to_vec();
    ordered.sort_by(|a, b| a.total_cmp(b));
    let index = ((fraction * (ordered.len() - 1) as f64).round_ties_even() as usize).min(ordered.len() - 1);
    ordered[index]
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Int(i) => Some(*i as f64),
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(*b as u8 as f64),
        _ => None,
    }
}

/// Python's truth of a number from the results.
fn truthy(value: &Value) -> bool {
    number(value).is_some_and(|n| n != 0.0)
}

fn fmt(value: &Value, unit: &str) -> String {
    let text = match value {
        Value::Null => return "-".into(),
        Value::Float(f) if *f < 100.0 => format!("{:.2}", f),
        Value::Float(f) => format!("{:.0}", f),
        other => pyjson::py_str(other),
    };
    format!("{} {}", text, unit).trim().to_string()
}

fn delta(now: &Value, was: &Value) -> String {
    let (Some(now), Some(was)) = (number(now), number(was)) else { return String::new() };
    if was == 0.0 {
        return String::new();
    }
    if now <= was {
        return if now != 0.0 { format!("({:.1}x)", was / now) } else { "(inf)".into() };
    }
    format!("(+{:.0}%)", 100.0 * (now - was) / was)
}

/// The table: one column per workload, a delta against `baseline` if given.
pub fn report(data: &Value, baseline: Option<&Value>) -> String {
    let results = data.get("results");
    let baseline = baseline.filter(|b| b.truthy());
    let width = if baseline.is_some() { 26 } else { 14 };
    let cmd: Vec<String> = data.get("cmd").list_or_empty().iter().map(pyjson::py_str).collect();
    let mut lines =
        vec![String::new(), format!("  herdr-newtab-plus process bench   {}", cmd.join(" ")), String::new()];
    let mut header = format!("  {:22}", "");
    for (name, ..) in WORKLOADS {
        let column = format!("{} ({})", name, pyjson::py_str(results.get(name).get("folders")));
        header.push_str(&format!("{:>width$}", column, width = width));
    }
    let rule = format!("  {}", "-".repeat(header.chars().count() - 2));
    lines.push(header);
    lines.push(rule);
    for (key, label, unit) in ROWS {
        let mut row = format!("  {:22}", label);
        for (name, ..) in WORKLOADS {
            let value = results.get(name).get(key);
            let mut cell = fmt(value, unit);
            if let Some(baseline) = baseline {
                let was = baseline.get("results").get(name).get(key);
                if truthy(was) && !value.is_null() {
                    cell = format!("{} {}", cell, delta(value, was));
                }
            }
            row.push_str(&format!("{:>width$}", cell, width = width));
        }
        lines.push(row);
    }
    lines.push(String::new());
    let version = results.get("version_p50_ms");
    let mut cell = fmt(version, "ms");
    if let Some(baseline) = baseline {
        let was = baseline.get("results").get("version_p50_ms");
        if truthy(was) {
            cell = format!("{} {}", cell, delta(version, was));
        }
    }
    lines.push(format!("  {:22}{}", "--version", cell));
    lines.push(String::new());
    lines.join("\n") + "\n"
}

/// Every dict's keys in order, as `sort_keys=True` writes them.
fn sorted(value: &Value) -> Value {
    match value {
        Value::Dict(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| a.0.cmp(b.0));
            Value::Dict(entries.into_iter().map(|(k, v)| (k.clone(), sorted(v))).collect::<Map>())
        }
        Value::List(items) => Value::List(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

/// `json.dumps(data, indent=2, sort_keys=True)`.
pub fn to_json(data: &Value) -> String {
    pyjson::dumps_indent(&sorted(data), 2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentile_rounds_ties_to_even() {
        let values: Vec<f64> = (0..12).map(|v| v as f64).collect();
        assert_eq!(percentile(&values, 0.5), 6.0); // round(5.5) == 6
        let values: Vec<f64> = (0..4).map(|v| v as f64).collect();
        assert_eq!(percentile(&values, 0.5), 2.0); // round(1.5) == 2
        assert_eq!(percentile(&[1.0, 2.0], 0.5), 1.0); // round(0.5) == 0
        assert_eq!(percentile(&[], 0.5), 0.0);
    }

    #[test]
    fn cells_and_deltas_read_as_the_python_ones() {
        assert_eq!(fmt(&Value::Float(3.14159), "ms"), "3.14 ms");
        assert_eq!(fmt(&Value::Float(428.5), "ms"), "428 ms");
        assert_eq!(fmt(&Value::Int(1), ""), "1");
        assert_eq!(fmt(&Value::Null, "ms"), "-");
        assert_eq!(delta(&Value::Float(1.0), &Value::Float(4.0)), "(4.0x)");
        assert_eq!(delta(&Value::Float(0.0), &Value::Float(4.0)), "(inf)");
        assert_eq!(delta(&Value::Int(150), &Value::Int(100)), "(+50%)");
    }
}
