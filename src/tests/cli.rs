//! Ported from the Python release's tests/test_cli.py.
#![allow(unused_imports)]

use crate::agents::Agent;
use crate::app::Choice;
use crate::cli::{create, transient};
use crate::client::{HerdrClient, HerdrError};
use crate::pyjson::{self, Value};
use crate::session::{Target, SPACE, TAB};
use crate::testutil::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// A Herdr socket that records each request and answers every one with the
/// same line. The Python faked the client object; this fakes the server, so
/// the real client's request is what gets checked. Stopped and removed on drop.
struct FakeHerdr {
    path: String,
    seen: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeHerdr {
    fn serve(reply: &str) -> FakeHerdr {
        let path = socket_path();
        let listener = UnixListener::bind(&path).expect("bind fake socket");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (seen_in, stop_in, reply) = (seen.clone(), stop.clone(), format!("{}\n", reply));
        let thread = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop_in.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut conn) = conn else { continue };
                let mut line = String::new();
                if BufReader::new(&conn).read_line(&mut line).is_err() {
                    continue;
                }
                if let Ok(request) = pyjson::loads(line.trim_end()) {
                    seen_in.lock().unwrap().push(request);
                }
                let _ = conn.write_all(reply.as_bytes());
            }
        });
        FakeHerdr { path, seen, stop, thread: Some(thread) }
    }

    /// A client that talks to this socket only: no herdr binary to fall back on.
    fn client(&self) -> HerdrClient {
        HerdrClient::new(Some(self.path.clone()), Some("/no/such/herdr".to_string()), 4.0)
    }

    /// Each request as (method, params).
    fn calls(&self) -> Vec<(String, Value)> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|request| (request.get("method").as_str().unwrap_or("").to_string(), request.get("params").clone()))
            .collect()
    }
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = UnixStream::connect(&self.path); // wake the accept loop
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

const CREATED: &str = r#"{"id": "newtab", "result": {"tab": {"tab_id": "w1:t9"}, "root_pane": {"pane_id": "w1:p9"}}}"#;

fn shell() -> Agent {
    Agent::new("", "shell")
}

fn choice(folder: &str, target: Target) -> Choice {
    Choice { folder: folder.to_string(), agent: shell(), target, skip_permissions: false }
}

fn target(kind: &'static str, folder: &str, workspace_id: Option<&str>, workspace_label: Option<&str>) -> Target {
    Target {
        kind,
        folder: folder.to_string(),
        workspace_id: workspace_id.map(str::to_string),
        workspace_label: workspace_label.map(str::to_string),
    }
}

fn assert_params(params: &Value, expected: &str) {
    assert!(pyjson::py_eq(params, &json(expected)), "{} != {}", pyjson::dumps(params, true), expected);
}

// -- CreateTest ---------------------------------------------------------------

#[test]
fn a_tab_target_creates_a_tab_in_that_space() {
    let herdr = FakeHerdr::serve(CREATED);
    let pane = create(&mut herdr.client(), &choice("/a/x", target(TAB, "/a/x", Some("w1"), Some("One")))).unwrap();
    assert_eq!(pane, "w1:p9");
    let calls = herdr.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "tab.create");
    // No label: a tab keeps the name Herdr gives it.
    assert_params(&calls[0].1, r#"{"workspace_id": "w1", "cwd": "/a/x", "focus": true}"#);
}

#[test]
fn a_space_target_creates_a_space_labelled_by_the_folder() {
    let herdr = FakeHerdr::serve(CREATED);
    create(&mut herdr.client(), &choice("/a/my-thing", target(SPACE, "/a/my-thing", None, None))).unwrap();
    let calls = herdr.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "workspace.create");
    assert_params(&calls[0].1, r#"{"cwd": "/a/my-thing", "focus": true, "label": "my-thing"}"#);
}

#[test]
fn a_tab_target_without_a_space_falls_back_to_creating_one() {
    let herdr = FakeHerdr::serve(CREATED);
    create(&mut herdr.client(), &choice("/a/x", target(TAB, "/a/x", None, None))).unwrap();
    assert_eq!(herdr.calls()[0].0, "workspace.create");
}

#[test]
fn a_reply_with_no_pane_is_an_error_rather_than_a_silent_pass() {
    let herdr = FakeHerdr::serve(r#"{"id": "newtab", "result": {"tab": {"tab_id": "w9:t1"}}}"#);
    let result: Result<String, HerdrError> =
        create(&mut herdr.client(), &choice("/a/x", target(SPACE, "/a/x", None, None)));
    assert!(result.is_err());
}

// -- StartAgentArgsTest -------------------------------------------------------
// Agent arguments have to survive both transports, spelled each one's way.

/// What one `start_agent` sends: the socket's params, and the CLI's argv.
/// The Python replaced `client.call` to see both from one call; here each
/// transport is faked for real -- a socket, and a `herdr` script that writes
/// down its arguments.
fn start(extra: &[&str]) -> (Value, Vec<String>) {
    use std::os::unix::fs::PermissionsExt;
    let extra: Vec<String> = extra.iter().map(|a| a.to_string()).collect();

    let herdr = FakeHerdr::serve(r#"{"id": "newtab", "result": {}}"#);
    herdr.client().start_agent("a", "claude", "w1:p1", 30000, &extra).unwrap();
    let params = herdr.calls().pop().expect("a socket request").1;

    let bin = TempDir::new();
    bin.touch(
        "herdr",
        "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$(dirname \"$0\")/argv\"\necho '{\"result\": {}}'\n",
    );
    std::fs::set_permissions(bin.join("herdr"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut client = HerdrClient::new(Some(bin.join("no-socket")), Some(bin.join("herdr")), 4.0);
    client.start_agent("a", "claude", "w1:p1", 30000, &extra).unwrap();
    let argv = std::fs::read_to_string(bin.join("argv")).unwrap();
    (params, argv.lines().map(str::to_string).collect())
}

#[test]
fn the_socket_gets_them_as_a_field() {
    let (params, _) = start(&["--dangerously-skip-permissions"]);
    assert_params(params.get("args"), r#"["--dangerously-skip-permissions"]"#);
}

#[test]
fn the_cli_gets_them_after_a_double_dash() {
    let (_, cli) = start(&["--dangerously-skip-permissions"]);
    assert_eq!(&cli[cli.len() - 2..], ["--", "--dangerously-skip-permissions"]);
}

#[test]
fn a_flag_is_never_mistaken_for_one_of_herdrs_own() {
    // Without the separator, `herdr` would try to parse these itself.
    let (_, cli) = start(&["--kind", "evil"]);
    let at = |arg: &str| cli.iter().position(|a| a == arg).unwrap();
    assert!(at("--") < at("evil"));
}

#[test]
fn no_args_means_no_separator_and_no_field() {
    let (params, cli) = start(&[]);
    assert!(!params.as_dict().unwrap().contains_key("args"));
    assert!(!cli.iter().any(|a| a == "--"));
}

// -- TransientTest ------------------------------------------------------------

#[test]
fn a_shell_that_is_still_starting_is_worth_waiting_for() {
    assert!(transient("agent target pane w3:p2 is not an available shell"));
}

#[test]
fn a_real_refusal_is_not_retried() {
    assert!(!transient("unknown agent kind: nope"));
    assert!(!transient("agent name already in use"));
}
