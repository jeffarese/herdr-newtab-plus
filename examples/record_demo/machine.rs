//! The machine under the camera: a synthetic home, a fake Herdr session and
//! the tools the picker looks for on PATH.
//!
//! The picker completes real directories, so a film made against the real
//! filesystem would show whatever the machine doing the recording happens to
//! hold. Instead the home is a scratch directory laid out with the folders
//! the film is about, the picker runs with `HOME` pointing at it, and every
//! path it shows is folded to `~`: where the scratch directory lives never
//! reaches the screen.
//!
//! What the binary reads, and where it comes from here:
//! * config -- `$HERDR_PLUGIN_CONFIG_DIR/config.json`: `~/workspace` as the
//!   one root, and the selection colour the film was always made with;
//! * recents -- `$HERDR_PLUGIN_STATE_DIR/recent.json`: which agent each folder
//!   last opened with, and the order of the resting list;
//! * zoxide -- a fake `zoxide` that knows one frecent folder;
//! * the session -- the fake Herdr on a Unix socket (`tests/parity/herdr.rs`);
//! * agents -- fake `claude`, `codex`, `opencode` and `pi` executables;
//! * the logo font -- a file named like it in the home's `Library/Fonts`
//!   (macOS) and a `fc-match` that names its family (elsewhere), as
//!   `tests/parity/machine.rs` does;
//! * the terminal's background -- answered by `pty.rs`.

use crate::herdr::Herdr;
use crate::pyjson::{self, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `~`'s top level.
const HOME_DIRS: &[&str] = &["Documents", "Downloads", "code", "workspace"];

/// `~/workspace`: the folders the film is about, and which are checkouts.
pub const FOLDERS: &[(&str, bool)] = &[
    ("atlas", false),
    ("classifier", false),
    ("herdr-bar", true),
    ("herdr-newtab-plus", true),
    ("log-sink", true),
    ("mobile-client", true),
    ("notes", true),
    ("webhook-relay", true),
];

/// What the preview finds inside the folders above: subfolders (and whether
/// each is a checkout of its own), files, and the branch checked out.
struct Inside {
    folder: &'static str,
    dirs: &'static [(&'static str, bool)],
    files: &'static [&'static str],
}

const INSIDE: &[Inside] = &[
    Inside {
        folder: "herdr-newtab-plus",
        dirs: &[("scripts", false), ("src", false), ("tests", false)],
        files: &["Cargo.lock", "Cargo.toml", "CHANGELOG.md", "herdr-plugin.toml", "LICENSE", "README.md"],
    },
    Inside {
        folder: "herdr-bar",
        dirs: &[("assets", false), ("scripts", false), ("src", false), ("tests", false)],
        files: &["Cargo.lock", "Cargo.toml", "CHANGELOG.md", "herdr-plugin.toml", "LICENSE", "README.md"],
    },
    Inside {
        folder: "atlas",
        dirs: &[("api", true), ("infra", false), ("web", true)],
        files: &["docker-compose.yml", "Makefile", "README.md"],
    },
    Inside {
        folder: "mobile-client",
        dirs: &[("android", false), ("ios", false)],
        files: &["app.json", "package.json", "README.md"],
    },
    Inside {
        folder: "webhook-relay",
        dirs: &[("cmd", false), ("internal", false)],
        files: &["go.mod", "go.sum", "main.go", "README.md"],
    },
    Inside {
        folder: "log-sink",
        dirs: &[("collector", false), ("dashboards", false)],
        files: &["config.yaml", "README.md"],
    },
];

const BRANCHES: &[(&str, &str)] = &[
    ("herdr-newtab-plus", "main"),
    ("herdr-bar", "preview-pane"),
    ("mobile-client", "release/2.4"),
    ("notes", "main"),
    ("log-sink", "main"),
    ("webhook-relay", "retry-backoff"),
];

/// What the picker remembers, most recent first: the folder and the agent it
/// last opened with. Atlas leads, so the row Enter lands on at the end is a
/// folder that already has a space.
const RECENTS: &[(&str, &str)] = &[("atlas", "claude"), ("herdr-bar", "codex"), ("herdr-newtab-plus", "claude")];

/// The one folder zoxide knows that nothing else offers.
const ZOXIDE: &[(&str, f64)] = &[("mobile-client", 48.0)];

const AGENTS: &[&str] = &["claude", "codex", "opencode", "pi"];

const CONFIG: &str = r#"{"roots": ["~/workspace"], "selection_background": "237"}"#;

/// Two spaces, each with an agent in it: one working, one waiting on you.
const SNAPSHOT: &str = r#"{
    "version": "0.7.5",
    "focused_workspace_id": "w1",
    "focused_pane_id": "w1:p5P",
    "workspaces": [
        {"workspace_id": "w1", "label": "Atlas", "number": 1, "focused": true},
        {"workspace_id": "w2", "label": "Herdr", "number": 2, "focused": false}
    ],
    "panes": [
        {"pane_id": "w1:p5P", "workspace_id": "w1", "cwd": "{home}/workspace/atlas"},
        {"pane_id": "w2:p1A", "workspace_id": "w2", "cwd": "{home}/workspace/herdr-bar"}
    ],
    "agents": [
        {"pane_id": "w1:p5P", "agent": "claude", "agent_status": "working"},
        {"pane_id": "w2:p1A", "agent": "codex", "agent_status": "blocked"}
    ]
}"#;

pub struct Machine {
    dir: PathBuf,
    pub home: PathBuf,
    bin: PathBuf,
    config: PathBuf,
    state: PathBuf,
    socket: PathBuf,
    /// The workspace labels by id, for saying where a new tab went.
    pub labels: Vec<(String, String)>,
}

fn s(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {}", path.display(), e));
}

fn script(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    write(&path, &format!("#!/bin/sh\n{}", body));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn checkout(folder: &Path, branch: Option<&str>) {
    let git = folder.join(".git");
    std::fs::create_dir_all(&git).unwrap();
    write(&git.join("HEAD"), &format!("ref: refs/heads/{}\n", branch.unwrap_or("main")));
}

impl Machine {
    /// A short scratch directory under the canonical /tmp: macOS caps socket
    /// paths at 104 bytes, and the canonical form is what the picker folds
    /// to `~`, should anything resolve a path through the /tmp link.
    pub fn new() -> Machine {
        let base = std::fs::canonicalize("/tmp").expect("canonicalize /tmp");
        let dir = base.join(format!("newtab-demo-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let machine = Machine {
            home: dir.join("home"),
            bin: dir.join("bin"),
            config: dir.join("config"),
            state: dir.join("state"),
            socket: dir.join("herdr.sock"),
            dir,
            labels: Vec::new(),
        };
        machine.lay_out();
        machine
    }

    fn lay_out(&self) {
        let workspace = self.home.join("workspace");
        for name in HOME_DIRS {
            std::fs::create_dir_all(self.home.join(name)).unwrap();
        }
        for (name, git) in FOLDERS {
            let folder = workspace.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            if *git {
                checkout(&folder, BRANCHES.iter().find(|(f, _)| f == name).map(|(_, b)| *b));
            }
        }
        for inside in INSIDE {
            let folder = workspace.join(inside.folder);
            for (name, git) in inside.dirs {
                std::fs::create_dir_all(folder.join(name)).unwrap();
                if *git {
                    checkout(&folder.join(name), None);
                }
            }
            for name in inside.files {
                write(&folder.join(name), "");
            }
        }

        let font = self.home.join("Library/Fonts/HerdrAgentIconsMax-demo.ttf");
        write(&font, "");

        std::fs::create_dir_all(&self.bin).unwrap();
        for agent in AGENTS {
            script(&self.bin, agent, "exit 0\n");
        }
        script(&self.bin, "fc-match", "printf '%s' 'Herdr Agent Icons Max'\n");
        let listing: String =
            ZOXIDE.iter().map(|(name, score)| format!("{:>6.1} {}\n", score, s(&workspace.join(name)))).collect();
        script(&self.bin, "zoxide", &format!("/bin/cat <<'EOF'\n{}EOF\n", listing));
        // macOS assesses a new executable on its first launch, which can take
        // longer than the picker waits for zoxide. Pay that here, off camera.
        for entry in std::fs::read_dir(&self.bin).unwrap().flatten() {
            let _ =
                Command::new(entry.path()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }

        write(&self.config.join("config.json"), CONFIG);
        let entries: Vec<String> = RECENTS
            .iter()
            .enumerate()
            .map(|(index, (name, agent))| {
                format!(
                    r#"{{"path": "{}", "agent": "{}", "ts": {}}}"#,
                    s(&workspace.join(name)),
                    agent,
                    1_760_000_000 - 3600 * index as i64
                )
            })
            .collect();
        write(&self.state.join("recent.json"), &format!(r#"{{"version": 1, "entries": [{}]}}"#, entries.join(", ")));
    }

    /// The session the fake Herdr serves, with the home filled in.
    fn snapshot(&mut self) -> Value {
        let data = pyjson::loads(&SNAPSHOT.replace("{home}", &s(&self.home))).expect("SNAPSHOT is not JSON");
        self.labels = data
            .get("workspaces")
            .list_or_empty()
            .iter()
            .map(|w| {
                let field = |key: &str| w.get(key).as_str().unwrap_or("").to_string();
                (field("workspace_id"), field("label"))
            })
            .collect();
        data
    }

    pub fn herdr(&mut self) -> Herdr {
        let data = self.snapshot();
        Herdr::start(&self.socket, data, Value::Null)
    }

    pub fn command(&self, binary: &Path) -> Command {
        let mut command = Command::new(binary);
        command.env_clear().current_dir(&self.home).envs([
            ("PATH", s(&self.bin)),
            ("HOME", s(&self.home)),
            ("TERM", "xterm-256color".into()),
            ("LANG", "C.UTF-8".into()),
            ("LC_ALL", "C.UTF-8".into()),
            ("HERDR_SOCKET_PATH", s(&self.socket)),
            ("HERDR_PLUGIN_CONFIG_DIR", s(&self.config)),
            ("HERDR_PLUGIN_STATE_DIR", s(&self.state)),
            ("HERDR_BIN_PATH", "/nonexistent/herdr".into()),
        ]);
        command
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
