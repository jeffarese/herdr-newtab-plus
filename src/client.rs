//! Talk to the running Herdr server.
//!
//! Two transports, same surface:
//!
//! * the Unix socket at `HERDR_SOCKET_PATH` -- one request per connection,
//!   newline-delimited JSON. Used for the snapshot the picker reads when it
//!   opens.
//! * the `herdr` CLI at `HERDR_BIN_PATH` -- portable, works when the socket is
//!   missing. Used as the automatic fallback for everything else.

use crate::pyjson::{self, Map, Value};
use crate::pystr;
use std::fmt;
use std::io::{self, Read, Write};
use std::time::Duration;

/// A Herdr request failed, or no server could be reached.
#[derive(Clone, Debug, PartialEq)]
pub struct HerdrError(pub String);

impl fmt::Display for HerdrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl HerdrError {
    pub fn new(message: impl Into<String>) -> HerdrError {
        HerdrError(message.into())
    }
}

pub type Result<T> = std::result::Result<T, HerdrError>;

/// What went wrong below the protocol: Python's OSError and ValueError, which
/// send the request to the CLI instead, or a HerdrError, which does not.
enum Transport {
    Fallback,
    Herdr(HerdrError),
}

fn env_path(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty())
}

pub fn is_executable_file(path: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() {
        return false;
    }
    let Ok(c_path) = std::ffi::CString::new(std::ffi::OsStr::new(path).as_bytes()) else { return false };
    unsafe { libc::access(c_path.as_ptr(), libc::X_OK) == 0 }
}

/// The Herdr binary to shell out to.
///
/// HERDR_BIN_PATH can outlive the binary it names -- upgrading Herdr under a
/// running server leaves the old path dangling -- so an unusable value falls
/// back to whatever `herdr` resolves to on PATH.
pub fn resolve_bin() -> String {
    match env_path("HERDR_BIN_PATH") {
        Some(candidate) if is_executable_file(&candidate) => candidate,
        _ => "herdr".to_string(),
    }
}

pub fn dict(entries: Vec<(&str, Value)>) -> Value {
    let mut map = Map::new();
    for (key, value) in entries {
        map.insert(key, value);
    }
    Value::Dict(map)
}

/// `str(OSError)`, as the Python release would have printed one.
pub fn os_error_str(error: &io::Error) -> String {
    match error.raw_os_error() {
        Some(code) => {
            let text = error.to_string();
            let suffix = format!(" (os error {})", code);
            format!("[Errno {}] {}", code, text.strip_suffix(&suffix).unwrap_or(&text))
        }
        None => error.to_string(),
    }
}

pub struct HerdrClient {
    pub socket_path: Option<String>,
    pub bin_path: String,
    pub timeout: f64,
    socket_ok: bool,
}

impl HerdrClient {
    pub fn new(socket_path: Option<String>, bin_path: Option<String>, timeout: f64) -> HerdrClient {
        let socket_path = socket_path.or_else(|| env_path("HERDR_SOCKET_PATH"));
        let bin_path = bin_path.filter(|b| !b.is_empty()).unwrap_or_else(resolve_bin);
        let socket_ok = socket_path.as_deref().map(|p| std::fs::metadata(p).is_ok()).unwrap_or(false);
        HerdrClient { socket_path, bin_path, timeout, socket_ok }
    }

    pub fn from_env() -> HerdrClient {
        HerdrClient::new(None, None, 4.0)
    }

    pub fn socket_ok(&self) -> bool {
        self.socket_ok
    }

    // -- transports ---------------------------------------------------------

    fn socket_call(&self, method: &str, params: &Value) -> std::result::Result<Value, Transport> {
        let request =
            dict(vec![("id", Value::str("newtab")), ("method", Value::str(method)), ("params", params.clone())]);
        let mut payload = pyjson::dumps(&request, false).into_bytes();
        payload.push(b'\n');
        let path = self.socket_path.as_deref().unwrap_or("");
        let line = unix_request(path, &payload, self.timeout).map_err(|_| Transport::Fallback)?;
        if line.is_empty() {
            return Err(Transport::Herdr(HerdrError::new("herdr closed the connection without a response")));
        }
        pyjson::loads(&String::from_utf8_lossy(&line)).map_err(|_| Transport::Fallback)
    }

    fn cli(&self, args: &[String]) -> Result<Value> {
        let mut argv = vec![self.bin_path.clone()];
        argv.extend(args.iter().cloned());
        let joined = args.join(" ");
        let output = match crate::proc::run(&argv, self.timeout + 4.0) {
            Ok(output) => output,
            Err(crate::proc::RunError::Spawn(error)) if error.kind() == io::ErrorKind::NotFound => {
                return Err(HerdrError::new("herdr binary not found (set HERDR_BIN_PATH)"));
            }
            Err(crate::proc::RunError::Spawn(error)) => return Err(HerdrError::new(os_error_str(&error))),
            Err(crate::proc::RunError::Timeout) => return Err(HerdrError::new(format!("herdr {} timed out", joined))),
        };
        let out = String::from_utf8_lossy(&output.stdout);
        let out = pystr::strip(&out);
        if out.is_empty() {
            let err = String::from_utf8_lossy(&output.stderr);
            let err = pystr::strip(&err);
            if err.is_empty() {
                return Err(HerdrError::new(format!("herdr {} produced no output", joined)));
            }
            return Err(HerdrError::new(err));
        }
        let lines = pystr::splitlines(out);
        let last = lines.last().copied().unwrap_or("");
        pyjson::loads(last).map_err(|_| HerdrError::new(format!("herdr {} returned unparseable output", joined)))
    }

    pub fn unwrap(response: Value) -> Result<Map> {
        let Value::Dict(mut response) = response else {
            return Err(HerdrError::new("herdr returned an unexpected response shape"));
        };
        if let Some(error) = response.get("error") {
            if error.truthy() {
                let message = match error {
                    Value::Dict(map) => map.get("message").cloned().unwrap_or(Value::Null),
                    other => Value::str(pyjson::py_str(other)),
                };
                if message.truthy() {
                    return Err(HerdrError::new(pyjson::py_str(&message)));
                }
                return Err(HerdrError::new("herdr request failed"));
            }
        }
        match response.remove("result") {
            Some(Value::Dict(result)) => Ok(result),
            _ => Err(HerdrError::new("herdr returned an unexpected response shape")),
        }
    }

    /// Run a request over the socket, falling back to the CLI.
    pub fn call(&mut self, method: &str, params: Value, cli_args: Vec<String>) -> Result<Map> {
        if self.socket_ok {
            match self.socket_call(method, &params) {
                Ok(response) => return HerdrClient::unwrap(response),
                Err(Transport::Herdr(error)) => return Err(error),
                // Transport-level failure only: the server may have restarted
                // or handed off. Stop trusting the socket and use the CLI.
                Err(Transport::Fallback) => self.socket_ok = false,
            }
        }
        HerdrClient::unwrap(self.cli(&cli_args)?)
    }

    // -- operations ---------------------------------------------------------

    pub fn snapshot(&mut self) -> Result<Value> {
        let mut result = self.call("session.snapshot", dict(vec![]), args(&["api", "snapshot"]))?;
        match result.remove("snapshot") {
            Some(snapshot @ Value::Dict(_)) => Ok(snapshot),
            _ => Err(HerdrError::new("session snapshot missing from response")),
        }
    }

    pub fn create_tab(&mut self, workspace_id: &str, cwd: &str, label: Option<&str>) -> Result<Map> {
        let mut params =
            vec![("workspace_id", Value::str(workspace_id)), ("cwd", Value::str(cwd)), ("focus", Value::Bool(true))];
        let mut cli = args(&["tab", "create", "--workspace", workspace_id, "--cwd", cwd, "--focus"]);
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            params.push(("label", Value::str(label)));
            cli.extend(args(&["--label", label]));
        }
        self.call("tab.create", dict(params), cli)
    }

    pub fn create_workspace(&mut self, cwd: &str, label: Option<&str>) -> Result<Map> {
        let mut params = vec![("cwd", Value::str(cwd)), ("focus", Value::Bool(true))];
        let mut cli = args(&["workspace", "create", "--cwd", cwd, "--focus"]);
        if let Some(label) = label.filter(|l| !l.is_empty()) {
            params.push(("label", Value::str(label)));
            cli.extend(args(&["--label", label]));
        }
        self.call("workspace.create", dict(params), cli)
    }

    /// Launch an agent into a pane that is still an idle shell.
    ///
    /// Herdr only returns once the agent owns the terminal and is ready for
    /// input, so this can sit for many seconds. Callers that want the popup
    /// gone first should run it in a detached child.
    ///
    /// `extra` is handed to the agent's own executable rather than to Herdr
    /// -- `args` over the socket, everything after `--` on the command line.
    pub fn start_agent(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        timeout_ms: i64,
        extra: &[String],
    ) -> Result<()> {
        let mut params = vec![
            ("name", Value::str(name)),
            ("kind", Value::str(kind)),
            ("pane_id", Value::str(pane_id)),
            ("timeout_ms", Value::Int(timeout_ms as i128)),
        ];
        let timeout = timeout_ms.to_string();
        let mut cli = args(&["agent", "start", name, "--kind", kind, "--pane", pane_id, "--timeout", &timeout]);
        if !extra.is_empty() {
            params.push(("args", Value::List(extra.iter().map(|a| Value::str(a.clone())).collect())));
            cli.push("--".to_string());
            cli.extend(extra.iter().cloned());
        }
        self.call("agent.start", dict(params), cli).map(|_| ())
    }
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn unix_request(path: &str, payload: &[u8], timeout: f64) -> io::Result<Vec<u8>> {
    use std::os::unix::net::UnixStream;
    let mut conn = UnixStream::connect(path)?;
    let limit = Some(Duration::from_secs_f64(timeout.max(0.001)));
    conn.set_read_timeout(limit)?;
    conn.set_write_timeout(limit)?;
    conn.write_all(payload)?;
    // Read straight into the answer, in reads that grow with it: a big
    // snapshot arrives in a handful of reads instead of one per 64 KiB.
    let mut buffered: Vec<u8> = Vec::new();
    let mut scanned = 0;
    loop {
        if buffered[scanned..].contains(&b'\n') {
            break;
        }
        scanned = buffered.len();
        let room = scanned.clamp(65536, 4 << 20);
        buffered.resize(scanned + room, 0);
        let count = match conn.read(&mut buffered[scanned..]) {
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                buffered.truncate(scanned);
                continue;
            }
            Err(error) => return Err(error),
        };
        buffered.truncate(scanned + count);
        if count == 0 {
            break;
        }
    }
    let end = buffered.iter().position(|&b| b == b'\n').unwrap_or(buffered.len());
    buffered.truncate(end);
    Ok(buffered)
}
