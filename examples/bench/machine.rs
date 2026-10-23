//! A home on disk, a fake Herdr on a socket, and fake agents on PATH
//! (bench_process.py's Machine). The fake Herdr is the parity harness's.

use crate::herdr::Herdr;
use crate::pyjson::{self, Map, Value};
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub struct Machine {
    tmp: PathBuf,
    pub server: Herdr,
    env: Vec<(String, String)>,
}

fn s(path: &Path) -> String {
    path.to_str().expect("a UTF-8 temporary directory").to_string()
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {}", path.display(), e));
}

fn dict(entries: Vec<(&str, Value)>) -> Value {
    Value::Dict(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map>())
}

/// `tempfile.mkdtemp(prefix="ntb-")`: a new directory under the temporary
/// directory, with as many random characters as Python's.
fn mkdtemp() -> PathBuf {
    let template = std::env::temp_dir().join("ntb-XXXXXXXX");
    let mut bytes = template.into_os_string().into_vec();
    bytes.push(0);
    let made = unsafe { libc::mkdtemp(bytes.as_mut_ptr().cast()) };
    assert!(!made.is_null(), "mkdtemp: {}", std::io::Error::last_os_error());
    bytes.pop();
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

impl Machine {
    pub fn new(folders: usize, panes: usize, recents: usize, zoxide: usize) -> Machine {
        let tmp = mkdtemp();
        let base = std::fs::canonicalize(&tmp).unwrap();
        let home = base.join("home");
        let root = home.join("work");
        let bin = base.join("bin");
        let config_dir = base.join("config");
        let state_dir = base.join("state");
        let socket_path = base.join("herdr.sock");
        for path in [&root, &bin, &config_dir, &state_dir] {
            std::fs::create_dir_all(path).unwrap();
        }

        let names: Vec<String> = (0..folders).map(|index| format!("project-{:04}", index)).collect();
        for (index, name) in names.iter().enumerate() {
            let folder = root.join(name);
            std::fs::create_dir(&folder).unwrap();
            for child in &["src", "tests", "docs", "scripts"][..index % 4 + 1] {
                std::fs::create_dir(folder.join(child)).unwrap();
            }
            for child in &["README.md", "LICENSE", "Makefile", "pyproject.toml", "notes.txt"][..index % 5 + 1] {
                write(&folder.join(child), "");
            }
            if index % 2 == 1 {
                std::fs::create_dir(folder.join(".git")).unwrap();
                write(&folder.join(".git/HEAD"), "ref: refs/heads/main\n");
            }
        }
        let folder = |index: usize| s(&root.join(&names[index % names.len()]));

        // Fake agents, so "agents": "auto" finds the same two on every machine,
        // and a fake zoxide that prints a fixed frecency list.
        for agent in ["claude", "codex"] {
            script(&bin, agent, "exit 0\n");
        }
        let listing = base.join("zoxide.txt");
        let lines: String =
            (0..zoxide).map(|index| format!("{}.0 {}\n", 1000 - index as i64, folder(index * 7))).collect();
        write(&listing, &lines);
        if zoxide > 0 {
            script(&bin, "zoxide", &format!("cat \"{}\"\n", s(&listing)));
        }

        let config = dict(vec![("roots", Value::List(vec![Value::Str(s(&root))])), ("zoxide", Value::str("auto"))]);
        write(&config_dir.join("config.json"), &pyjson::dumps(&config, false));
        let entries = (0..recents)
            .map(|index| {
                dict(vec![
                    ("path", Value::Str(folder(index * 3))),
                    ("agent", Value::str(if index % 2 == 0 { "claude" } else { "" })),
                    ("ts", Value::Int(1_700_000_000 - index as i128)),
                ])
            })
            .collect();
        let recent = dict(vec![("version", Value::Int(1)), ("entries", Value::List(entries))]);
        write(&state_dir.join("recent.json"), &pyjson::dumps(&recent, false));

        let server = Herdr::start(&socket_path, snapshot(panes, &folder), Value::Null);
        let env = vec![
            ("PATH".into(), format!("{}:/usr/bin:/bin", s(&bin))),
            ("HOME".into(), s(&home)),
            ("TERM".into(), "xterm-256color".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("HERDR_SOCKET_PATH".into(), s(&socket_path)),
            ("HERDR_PLUGIN_CONFIG_DIR".into(), s(&config_dir)),
            ("HERDR_PLUGIN_STATE_DIR".into(), s(&state_dir)),
            // Never reach a real Herdr through the CLI fallback.
            ("HERDR_BIN_PATH".into(), "/nonexistent/herdr".into()),
        ];
        Machine { tmp, server, env }
    }

    /// The plugin's whole environment.
    pub fn env(&self) -> &[(String, String)] {
        &self.env
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.server.close();
        let _ = std::fs::remove_dir_all(&self.tmp);
    }
}

fn snapshot(panes: usize, folder: &dyn Fn(usize) -> String) -> Value {
    let spaces = (1..4)
        .map(|index| {
            dict(vec![
                ("workspace_id", Value::Str(format!("w{}", index))),
                ("label", Value::Str(format!("space-{}", index))),
                ("number", Value::Int(index)),
                ("focused", Value::Bool(index == 1)),
            ])
        })
        .collect();
    let mut pane_list = Vec::new();
    let mut agents = Vec::new();
    for index in 0..panes {
        let workspace = format!("w{}", index % 3 + 1);
        let pane_id = format!("{}:p{}", workspace, index);
        pane_list.push(dict(vec![
            ("pane_id", Value::Str(pane_id.clone())),
            ("workspace_id", Value::Str(workspace)),
            ("cwd", Value::Str(folder(index * 5))),
            ("focused", Value::Bool(index == 0)),
        ]));
        if index % 2 == 0 {
            agents.push(dict(vec![
                ("pane_id", Value::Str(pane_id)),
                ("agent", Value::str(if index % 4 != 0 { "claude" } else { "codex" })),
                ("agent_status", Value::str(["working", "blocked", "idle", "done"][index % 4])),
            ]));
        }
    }
    dict(vec![
        ("version", Value::str("0.9.4")),
        ("focused_workspace_id", Value::str("w1")),
        ("focused_pane_id", Value::str("w1:p0")),
        ("workspaces", Value::List(spaces)),
        ("panes", Value::List(pane_list)),
        ("agents", Value::List(agents)),
    ])
}

/// A `#!/bin/sh` tool on the machine's PATH.
fn script(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    write(&path, &format!("#!/bin/sh\n{}", body));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
