//! A fake Herdr on a Unix socket that answers the slice of the protocol the
//! plugin uses and logs every request (parity.py's RecordingHerdr).

use crate::pyjson::{self, Map, Value};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct Shared {
    data: Value,
    /// Per method, from the scenario: `{"reply": {...}}` sent with the id,
    /// `{"raw": "..."}` sent as is, or `{"by_call": [answer, ...]}` by call
    /// number, the last repeating.
    answers: Value,
    log: Mutex<Vec<(String, Value)>>,
    closing: AtomicBool,
}

pub struct Herdr {
    path: PathBuf,
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

fn line(message: &Value) -> Vec<u8> {
    let mut out = pyjson::dumps(message, false).into_bytes();
    out.push(b'\n');
    out
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map>())
}

impl Herdr {
    pub fn start(path: &Path, data: Value, answers: Value) -> Herdr {
        let listener = UnixListener::bind(path).unwrap_or_else(|e| panic!("bind {}: {}", path.display(), e));
        let shared = Arc::new(Shared { data, answers, log: Mutex::new(Vec::new()), closing: AtomicBool::new(false) });
        let accept = {
            let shared = shared.clone();
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if shared.closing.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(conn) = conn else { continue };
                    let shared = shared.clone();
                    std::thread::spawn(move || serve(&shared, conn));
                }
            })
        };
        Herdr { path: path.to_path_buf(), shared, accept: Some(accept) }
    }

    fn log(&self) -> std::sync::MutexGuard<'_, Vec<(String, Value)>> {
        self.shared.log.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Every request but the snapshot reads, in order, as `[method, params]`.
    pub fn changes(&self) -> Vec<Value> {
        self.log()
            .iter()
            .filter(|(m, _)| m != "session.snapshot")
            .map(|(m, p)| Value::List(vec![Value::str(m.clone()), p.clone()]))
            .collect()
    }

    pub fn has_create(&self) -> bool {
        self.log().iter().any(|(m, _)| m == "tab.create" || m == "workspace.create")
    }

    pub fn reads(&self) -> usize {
        self.log().iter().filter(|(m, _)| m == "session.snapshot").count()
    }

    pub fn wait_for(&self, method: &str, timeout: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            if self.log().iter().any(|(m, _)| m == method) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub fn close(&mut self) {
        if self.shared.closing.swap(true, Ordering::SeqCst) {
            return;
        }
        // Wake the accept loop so it sees `closing`.
        let _ = UnixStream::connect(&self.path);
        if let Some(handle) = self.accept.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for Herdr {
    fn drop(&mut self) {
        self.close();
    }
}

fn serve(shared: &Shared, mut conn: UnixStream) {
    let mut buffered = Vec::new();
    let mut chunk = vec![0u8; 65536];
    while !buffered.contains(&b'\n') {
        match conn.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buffered.extend_from_slice(&chunk[..n]),
        }
    }
    let first = &buffered[..buffered.iter().position(|b| *b == b'\n').unwrap()];
    let Ok(text) = std::str::from_utf8(first) else { return };
    let Ok(request) = pyjson::loads(text) else { return };
    let method = match request.get("method") {
        Value::Null => String::new(),
        method => pyjson::py_str(method),
    };
    let params = request.get("params");
    let params = if params.truthy() { params.clone() } else { Value::Dict(Map::new()) };
    let count = {
        let mut log = shared.log.lock().unwrap_or_else(|e| e.into_inner());
        log.push((method.clone(), params));
        log.iter().filter(|(m, _)| *m == method).count()
    };
    let _ = conn.write_all(&answer(shared, &method, count));
}

fn answer(shared: &Shared, method: &str, count: usize) -> Vec<u8> {
    let mut answer = shared.answers.get(method);
    if let Value::List(calls) = answer.get("by_call") {
        answer = &calls[(count - 1).min(calls.len() - 1)];
    }
    if let Some(raw) = answer.get("raw").as_str() {
        return raw.as_bytes().to_vec();
    }
    if let Some(reply) = answer.get("reply").as_dict() {
        let mut message = Map::new();
        message.insert("id", Value::str("newtab"));
        for (key, value) in reply.iter() {
            message.insert(key.clone(), value.clone());
        }
        return line(&Value::Dict(message));
    }
    assert!(answer.is_null(), "unknown answer for {}: {:?}", method, answer);
    let result = match method {
        "session.snapshot" => dict(vec![("snapshot", shared.data.clone())]),
        "tab.create" | "workspace.create" => dict(vec![
            ("tab", dict(vec![("tab_id", Value::str("w9:t1"))])),
            ("workspace", dict(vec![("workspace_id", Value::str("w9"))])),
            ("root_pane", dict(vec![("pane_id", Value::str("w9:p1"))])),
        ]),
        _ => Value::Dict(Map::new()),
    };
    line(&dict(vec![("id", Value::str("newtab")), ("result", result)]))
}
