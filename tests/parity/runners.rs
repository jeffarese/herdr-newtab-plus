//! One scenario of each kind, recorded as parity.py's run_picker and run_cli
//! record it.

use crate::machine::{returncode, setting, spawn_lock, Guard, Machine, Session};
use crate::pyjson::{self, Map, Value};
use crate::screen::Screen;
use std::fmt::Write as _;
use std::io::Read;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn num(value: &Value) -> f64 {
    match value {
        Value::Int(i) => *i as f64,
        Value::Float(f) => *f,
        _ => panic!("not a number: {:?}", value),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value.list_or_empty().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map>())
}

fn sleep(seconds: f64) {
    std::thread::sleep(Duration::from_secs_f64(seconds));
}

/// `repr(value)` of a bytes object, as parity.py labels a frame.
fn bytes_repr(bytes: &[u8]) -> String {
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') { b'"' } else { b'\'' };
    let mut out = String::from("b");
    out.push(quote as char);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if b == quote => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7e => out.push(b as char),
            b => {
                let _ = write!(out, "\\x{:02x}", b);
            }
        }
    }
    out.push(quote as char);
    out
}

/// A scenario step, one write or pause each.
enum Step {
    Keys(Vec<u8>),
    Wait(Value),
    Resize(u16, u16),
}

/// The scenario's steps: `["type", text]` is one key per character (a str in
/// parity.py's `keys()`), `["keys", text]` one write (a bytes item).
fn steps(scenario: &Value) -> Vec<Step> {
    let mut out = Vec::new();
    for step in scenario.get("steps").list_or_empty() {
        let step = step.list_or_empty();
        let (kind, value) = (step[0].as_str().unwrap(), &step[1]);
        match kind {
            "type" => out.extend(value.as_str().unwrap().chars().map(|c| Step::Keys(c.to_string().into_bytes()))),
            "keys" => out.push(Step::Keys(value.as_str().unwrap().as_bytes().to_vec())),
            "wait" => out.push(Step::Wait(value.clone())),
            "resize" => {
                let size = value.list_or_empty();
                out.push(Step::Resize(num(&size[0]) as u16, num(&size[1]) as u16));
            }
            other => panic!("unknown step {}", other),
        }
    }
    out
}

fn replace_bytes(data: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

pub fn run(cmd: &[String], spec: &Value, scenario: &Value) -> Value {
    match scenario.get("kind").as_str().unwrap() {
        "picker" => run_picker(cmd, spec, scenario),
        "cli" => run_cli(cmd, spec, scenario),
        kind => panic!("unknown kind {}", kind),
    }
}

fn run_picker(cmd: &[String], spec: &Value, scenario: &Value) -> Value {
    let machine = Machine::new(spec, scenario);
    let size = setting(spec, scenario, "size").list_or_empty();
    let (width, height) = (num(&size[0]) as u16, num(&size[1]) as u16);
    let settle = num(spec.get("settle"));
    let exit_timeout = num(spec.get("exit_timeout"));
    let appearance = match scenario.as_dict().and_then(|d| d.get("appearance")) {
        None => Some("dark"),
        Some(Value::Null) => None,
        Some(name) => Some(name.as_str().unwrap()),
    }
    .map(|name| spec.get("appearances").get(name).as_str().unwrap().as_bytes().to_vec());
    let argv: Vec<String> = cmd.iter().cloned().chain(strings(scenario.get("args"))).collect();
    let query = spec.get("query").as_str().unwrap().as_bytes();
    let mut session = Session::spawn(machine.command(&argv), width, height, appearance, query);
    let mut screen = Screen::new(width as usize, height as usize);
    let mut frames = Vec::new();
    let mut fed = 0;
    session.drain(num(spec.get("first_settle")));
    let mut steps: Vec<Option<Step>> = steps(scenario).into_iter().map(Some).collect();
    steps.insert(0, None);
    for step in steps {
        let label = match step {
            None => "open".to_string(),
            Some(Step::Keys(bytes)) => {
                session.send(&replace_bytes(&bytes, b"{root}", machine.root.as_bytes()));
                session.drain(settle);
                bytes_repr(&bytes)
            }
            Some(Step::Wait(seconds)) => {
                sleep(num(&seconds));
                session.drain(settle);
                format!("wait {}", pyjson::py_str(&seconds))
            }
            Some(Step::Resize(width, height)) => {
                session.resize(width, height);
                screen.resize(width as usize, height as usize);
                sleep(num(spec.get("resize_pause")));
                // Python's select() resumes after SIGWINCH and only redraws on
                // the next input; a key nothing is bound to wakes both builds.
                session.send(spec.get("nudge").as_str().unwrap().as_bytes());
                session.drain(settle);
                format!("resize {}x{}", width, height)
            }
        };
        screen.feed(&String::from_utf8_lossy(&session.output[fed..]));
        fed = session.output.len();
        frames.push(dict(vec![
            ("after", Value::Str(label)),
            ("screen", Value::List(screen.dump().into_iter().map(Value::Str).collect())),
        ]));
        if session.reaped() {
            break;
        }
    }
    let exit = if session.wait_reaped(exit_timeout) {
        Value::Int(session.status.unwrap() as i128)
    } else {
        session.send(b"\x03");
        if session.wait_reaped(exit_timeout) {
            Value::str(format!("did not exit; ctrl+c -> {}", session.status.unwrap()))
        } else {
            Value::str("hung")
        }
    };
    // The agent is started by a detached child after the popup has gone,
    // and the two builds' children boot at different speeds: wait for it
    // wherever something was created, so neither is cut off early.
    if let Some(server) = machine.server.as_ref() {
        let expect_agent = scenario.get("expect_agent").truthy();
        if expect_agent || server.has_create() {
            let timeout = num(spec.get(if expect_agent { "agent_timeout" } else { "create_timeout" }));
            server.wait_for("agent.start", timeout);
            sleep(num(spec.get("after_agent")));
        }
    }
    session.drain(num(spec.get("final_drain")));
    screen.feed(&String::from_utf8_lossy(&session.output[fed..]));
    let server = machine.server.as_ref();
    let transcript = dict(vec![
        ("frames", Value::List(frames)),
        ("exit", exit),
        ("printed", Value::Str(screen.main_text.clone())),
        ("modes", Value::List(screen.modes.iter().cloned().map(Value::Str).collect())),
        ("changes", Value::List(server.map(|s| s.changes()).unwrap_or_default())),
        ("snapshots", Value::Int(server.map(|s| s.reads()).unwrap_or(0) as i128)),
        ("files", machine.state_files()),
    ]);
    drop(session);
    machine.mask_all(&transcript)
}

fn run_cli(cmd: &[String], spec: &Value, scenario: &Value) -> Value {
    let machine = Machine::new(spec, scenario);
    let argv: Vec<String> = cmd.iter().cloned().chain(strings(scenario.get("args"))).collect();
    let mut command = machine.command(&argv);
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = {
        let _guard = spawn_lock();
        Guard(command.spawn().unwrap_or_else(|e| panic!("spawn {}: {}", argv[0], e)))
    };
    fn collect(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            out
        })
    }
    let readers = [collect(child.0.stdout.take().unwrap()), collect(child.0.stderr.take().unwrap())];
    let deadline = Instant::now() + Duration::from_secs_f64(num(spec.get("cli_timeout")));
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "cli scenario timed out");
        sleep(0.005);
    };
    let mut outputs = readers.into_iter().map(|r| String::from_utf8_lossy(&r.join().unwrap()).into_owned());
    let (stdout, stderr) = (machine.mask(&outputs.next().unwrap()), machine.mask(&outputs.next().unwrap()));
    // parity.py's `re.match` masks, all of them `^` and a literal prefix.
    let masks = strings(scenario.get("mask"));
    let stdout = stdout
        .split('\n')
        .map(|line| if masks.iter().any(|m| line.starts_with(m.as_str())) { "<masked>" } else { line })
        .collect::<Vec<_>>()
        .join("\n");
    let transcript = dict(vec![
        ("exit", Value::Int(returncode(status) as i128)),
        ("stdout", Value::Str(stdout)),
        ("stderr", Value::Str(stderr)),
        ("changes", Value::List(machine.server.as_ref().map(|s| s.changes()).unwrap_or_default())),
        ("files", machine.state_files()),
    ]);
    machine.mask_all(&transcript)
}
