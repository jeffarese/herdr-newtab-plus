//! The machine every run sees (parity.py's Machine), and the plugin on a
//! pseudo-terminal (parity.py's Session).
//!
//! Differences from parity.py, all for machine independence:
//! * The machine's directory is `<canonical /tmp>/ntp-<random>`, with as many
//!   random characters as make the canonical path exactly `TMP_LENGTH` long
//!   on every OS (`/private/tmp/ntp-xxxxxxxx` on macOS, 16 random characters
//!   under `/tmp` on Linux), and every occurrence of it in a transcript is
//!   replaced by `MASK`, which is as long. Equal lengths keep padding,
//!   truncation and style-span columns the same everywhere.
//! * The agent-icons font is installed on every machine: macOS reads
//!   `/Library/Fonts` as well as `~/Library/Fonts`, so only "installed" can
//!   be the same everywhere. The fake home has
//!   `Library/Fonts/HerdrAgentIconsMax-parity.ttf` (found on macOS) and the
//!   fake `fc-match` names the family (found on Linux).
//! * The umask is pinned to 022 (see `parity.rs`).

use crate::herdr::Herdr;
use crate::pyjson::{self, Value};
use std::collections::BTreeMap;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// The length of every machine directory's canonical path.
pub const TMP_LENGTH: usize = 25;
/// What a machine directory reads as in a transcript: `TMP_LENGTH` long.
pub const MASK: &str = "<tmp>____________________";

/// Held while a pty is opened, a fake tool is written and any child is
/// spawned, so no child inherits another scenario's pty before it is marked
/// close-on-exec, nor a script still open for writing (ETXTBSY on Linux).
pub static SPAWN_LOCK: Mutex<()> = Mutex::new(());

pub fn spawn_lock() -> std::sync::MutexGuard<'static, ()> {
    SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// `ntp-<random>` under the canonical /tmp, `TMP_LENGTH` long in all.
fn mkdtemp() -> PathBuf {
    let base = std::fs::canonicalize("/tmp").expect("canonicalize /tmp");
    let base_length = base.as_os_str().len();
    let random = TMP_LENGTH.checked_sub(base_length + "/ntp-".len()).filter(|n| *n >= 6).unwrap_or_else(|| {
        panic!("the canonical /tmp ({}) is too long for a {}-character machine directory", base.display(), TMP_LENGTH)
    });
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
    let mut urandom = std::fs::File::open("/dev/urandom").expect("open /dev/urandom");
    loop {
        let mut bytes = vec![0u8; random];
        urandom.read_exact(&mut bytes).expect("read /dev/urandom");
        let name: String = bytes.iter().map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char).collect();
        let path = base.join(format!("ntp-{}", name));
        assert_eq!(path.as_os_str().len(), TMP_LENGTH);
        match std::fs::create_dir(&path) {
            Ok(()) => return path,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("create {}: {}", path.display(), e),
        }
    }
}

/// The machine directory, removed on drop (panics included).
struct TmpDir(PathBuf);

impl Drop for TmpDir {
    fn drop(&mut self) {
        let locked = self.0.join("home/work/locked");
        if locked.exists() {
            let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn s(path: &Path) -> String {
    path.to_str().unwrap().to_string()
}

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap_or_else(|e| panic!("write {}: {}", path.display(), e));
}

fn strings(value: &Value) -> Vec<String> {
    value.list_or_empty().iter().map(|v| v.as_str().unwrap().to_string()).collect()
}

/// A scenario value, or the machine default when the scenario has no such key.
pub fn setting<'a>(spec: &'a Value, scenario: &'a Value, key: &str) -> &'a Value {
    match scenario.as_dict().and_then(|d| d.get(key)) {
        Some(value) => value,
        None => spec.get(key),
    }
}

/// `{root}` and `{home}` replaced in every string of a template.
fn fill(value: &Value, root: &str, home: &str) -> Value {
    match value {
        Value::Str(text) => Value::Str(text.replace("{root}", root).replace("{home}", home)),
        Value::List(items) => Value::List(items.iter().map(|v| fill(v, root, home)).collect()),
        Value::Dict(map) => Value::Dict(map.iter().map(|(k, v)| (k.clone(), fill(v, root, home))).collect()),
        other => other.clone(),
    }
}

/// A file body: a string as is, anything else as `json.dumps` writes it.
fn text_of(value: &Value) -> String {
    match value {
        Value::Str(text) => text.clone(),
        other => pyjson::dumps(other, false),
    }
}

pub struct Machine {
    pub tmp: String,
    pub root: String,
    state_dir: PathBuf,
    env: Vec<(String, String)>,
    pub server: Option<Herdr>,
    // Last: removed after the server has stopped.
    _dir: TmpDir,
}

impl Machine {
    pub fn new(spec: &Value, scenario: &Value) -> Machine {
        let dir = TmpDir(mkdtemp());
        let base = dir.0.clone();
        let home = base.join("home");
        let root = home.join("work");
        let bin = base.join("bin");
        let config_dir = base.join("config");
        let state_dir = base.join("state");
        let socket_path = base.join("herdr.sock");
        for path in [&root, &bin, &config_dir, &state_dir] {
            std::fs::create_dir_all(path).unwrap();
        }

        let git = strings(spec.get("git"));
        let children = strings(spec.get("children"));
        let files = strings(spec.get("files"));
        for (index, name) in strings(setting(spec, scenario, "folders")).iter().enumerate() {
            let folder = root.join(name);
            std::fs::create_dir_all(&folder).unwrap();
            for child in &children[..index % children.len() + 1] {
                std::fs::create_dir(folder.join(child)).unwrap();
            }
            for child in &files[..(index * 3) % files.len() + 1] {
                write(&folder.join(child), "x");
            }
            if git.contains(name) {
                std::fs::create_dir(folder.join(".git")).unwrap();
                let branch = if index % 3 != 0 { "main".to_string() } else { format!("feature/{}", name) };
                write(&folder.join(".git/HEAD"), &format!("ref: refs/heads/{}\n", branch));
            }
        }
        // A worktree, a detached head, an unreadable folder and an empty one.
        if !scenario.get("extras").is_false() {
            extras(&root);
        }
        let font = home.join("Library/Fonts");
        std::fs::create_dir_all(&font).unwrap();
        write(&font.join("HerdrAgentIconsMax-parity.ttf"), "x");

        {
            let _guard = spawn_lock();
            for agent in strings(setting(spec, scenario, "agents_on_path")) {
                script(&bin, &agent, "exit 0\n");
            }
            script(&bin, "fc-match", "printf '%s' 'Herdr Agent Icons Max'\n");
            if let Some(zoxide) = scenario.get("zoxide").as_str() {
                let listing = base.join("zoxide.txt");
                write(&listing, &zoxide.replace("{root}", &s(&root)).replace("{home}", &s(&home)));
                script(&bin, "zoxide", &format!("/bin/cat \"{}\"\n", s(&listing)));
            }
        }

        match setting(spec, scenario, "config") {
            Value::Null => {}
            config => write(&config_dir.join("config.json"), &text_of(config)),
        }
        if let Some(files) = scenario.get("files").as_dict() {
            for (relative, content) in files.iter() {
                let path = state_dir.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                write(&path, &text_of(content).replace("{root}", &s(&root)).replace("{home}", &s(&home)));
            }
        }

        // macOS assesses a new executable on its first launch, which can take
        // longer than the plugin waits for zoxide. Pay that here, not in a run.
        for entry in std::fs::read_dir(&bin).unwrap().flatten() {
            let mut command = Command::new(entry.path());
            command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
            let child = {
                let _guard = spawn_lock();
                command.spawn()
            };
            let _ = Guard(child.unwrap_or_else(|e| panic!("run {}: {}", entry.path().display(), e))).wait();
        }

        let data = match setting(spec, scenario, "data") {
            Value::Str(name) => spec.get("snapshots").get(name).clone(),
            Value::Null => spec.get("snapshots").get("default").clone(),
            template => template.clone(),
        };
        assert!(!data.is_null(), "unknown snapshot");
        let data = fill(&data, &s(&root), &s(&home));
        let server = if scenario.get("socket_up").is_false() {
            None
        } else {
            Some(Herdr::start(&socket_path, data, scenario.get("answers").clone()))
        };

        let mut env: Vec<(String, String)> = vec![
            ("PATH".into(), s(&bin)),
            ("HOME".into(), s(&home)),
            ("TERM".into(), "xterm-256color".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            ("PYTHONHASHSEED".into(), "0".into()),
            ("HERDR_SOCKET_PATH".into(), s(&socket_path)),
            ("HERDR_PLUGIN_CONFIG_DIR".into(), s(&config_dir)),
            ("HERDR_PLUGIN_STATE_DIR".into(), s(&state_dir)),
            ("HERDR_BIN_PATH".into(), "/nonexistent/herdr".into()),
        ];
        if let Some(extra) = scenario.get("env").as_dict() {
            for (key, value) in extra.iter() {
                env.retain(|(k, _)| k != key);
                env.push((key.clone(), value.as_str().unwrap().to_string()));
            }
        }
        Machine { tmp: s(&base), root: s(&root), state_dir, env, server, _dir: dir }
    }

    pub fn command(&self, argv: &[String]) -> Command {
        let mut command = Command::new(&argv[0]);
        command.args(&argv[1..]).env_clear().envs(self.env.iter().map(|(k, v)| (k, v))).current_dir(ROOT);
        command
    }

    /// Every file under the state directory, with volatile parts masked.
    pub fn state_files(&self) -> Value {
        let mut out = BTreeMap::new();
        let mut pending = vec![self.state_dir.clone()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let relative = s(path.strip_prefix(&self.state_dir).unwrap());
                let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
                let mut text = String::from_utf8_lossy(&raw).into_owned();
                if entry.file_name() == "recent.json" {
                    if let Ok(mut parsed) = pyjson::loads(&text) {
                        mask_timestamps(&mut parsed);
                        text = pyjson::dumps(&parsed, false);
                    }
                }
                let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
                out.insert(format!("{} (mode)", relative), Value::str(format!("0o{:o}", mode)));
                out.insert(relative, Value::Str(text));
            }
        }
        Value::Dict(out.into_iter().collect())
    }

    /// `text` with the machine directory masked.
    pub fn mask(&self, text: &str) -> String {
        text.replace(&self.tmp, MASK)
    }

    /// The machine directory masked in every string of a transcript.
    pub fn mask_all(&self, value: &Value) -> Value {
        match value {
            Value::Str(text) => Value::Str(self.mask(text)),
            Value::List(items) => Value::List(items.iter().map(|v| self.mask_all(v)).collect()),
            Value::Dict(map) => Value::Dict(map.iter().map(|(k, v)| (self.mask(k), self.mask_all(v))).collect()),
            other => other.clone(),
        }
    }
}

/// `ts` of recent entries written now (an int past 2001) as `<now>`.
fn mask_timestamps(parsed: &mut Value) {
    let Some(Value::List(entries)) = parsed.as_dict_mut().and_then(|d| d.get_mut("entries")) else { return };
    for entry in entries.iter_mut() {
        if let Some(entry) = entry.as_dict_mut() {
            let now = match entry.get("ts") {
                Some(Value::Int(ts)) => *ts > 1_000_000_000,
                Some(Value::BigInt(ts)) => !ts.starts_with('-'),
                _ => false,
            };
            if now {
                entry.insert("ts", Value::str("<now>"));
            }
        }
    }
}

fn extras(root: &Path) {
    let tree = root.join("harbor-review");
    std::fs::create_dir_all(tree.join("src")).unwrap();
    write(&tree.join(".git"), "gitdir: ../harbor/.git/worktrees/review\n");
    let gitdir = root.join("harbor/.git/worktrees/review");
    std::fs::create_dir_all(&gitdir).unwrap();
    write(&gitdir.join("HEAD"), "ref: refs/heads/review\n");
    let detached = root.join("detached");
    std::fs::create_dir_all(detached.join(".git")).unwrap();
    write(&detached.join(".git/HEAD"), "3f2a9c1d8e7b6a5f4e3d2c1b0a9f8e7d6c5b4a39\n");
    std::fs::create_dir_all(root.join("empty")).unwrap();
    let locked = root.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let many = root.join("many-files");
    std::fs::create_dir_all(&many).unwrap();
    for index in 0..40 {
        write(&many.join(format!("file-{:02}.txt", index)), "x");
    }
    std::os::unix::fs::symlink(root.join("atlas"), root.join("atlas-link")).unwrap();
    for name in ["日本語プロジェクト", "café-naïve", "emoji-🚀-rocket", "Zebra", "apple"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
}

/// A `#!/bin/sh` tool on the machine's PATH. Hold `spawn_lock()`.
fn script(bin: &Path, name: &str, body: &str) {
    let path = bin.join(name);
    write(&path, &format!("#!/bin/sh\n{}", body));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// `Popen.returncode`: the exit code, or minus the signal.
pub fn returncode(status: ExitStatus) -> i32 {
    status.code().unwrap_or_else(|| -status.signal().unwrap_or(0))
}

// -- the plugin on a pseudo-terminal -----------------------------------------

pub struct Session {
    master: OwnedFd,
    child: Child,
    pub status: Option<i32>,
    pub output: Vec<u8>,
    /// What the terminal answers the background colour query with, if at all.
    appearance: Option<Vec<u8>>,
    query: Vec<u8>,
    answered: usize,
}

fn cloexec(fd: i32) {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC);
    }
}

fn winsize(width: u16, height: u16) -> libc::winsize {
    libc::winsize { ws_row: height, ws_col: width, ws_xpixel: 0, ws_ypixel: 0 }
}

impl Session {
    pub fn spawn(mut command: Command, width: u16, height: u16, appearance: Option<Vec<u8>>, query: &[u8]) -> Session {
        let _guard = spawn_lock();
        let (mut master, mut slave) = (0, 0);
        let rc = unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut())
        };
        assert_eq!(rc, 0, "openpty: {}", std::io::Error::last_os_error());
        cloexec(master);
        cloexec(slave);
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        unsafe { libc::ioctl(slave.as_raw_fd(), libc::TIOCSWINSZ, &winsize(width, height)) };
        let io = |fd: &OwnedFd| Stdio::from(fd.try_clone().unwrap());
        command.stdin(io(&slave)).stdout(io(&slave)).stderr(io(&slave));
        // start_new_session=True: a session of its own, without a
        // controlling terminal, as parity.py's Popen started it.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap_or_else(|e| panic!("spawn {:?}: {}", command.get_program(), e));
        drop(command);
        drop(slave);
        Session { master, child, status: None, output: Vec::new(), appearance, query: query.to_vec(), answered: 0 }
    }

    /// Read whatever arrives within `timeout`; false once the pty is closed.
    pub fn pump(&mut self, timeout: f64) -> bool {
        let mut poll = libc::pollfd { fd: self.master.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        let ms = (timeout.max(0.0) * 1000.0).ceil() as i32;
        let ready = unsafe { libc::poll(&mut poll, 1, ms) };
        if ready <= 0 {
            return true;
        }
        let mut buffer = vec![0u8; 65536];
        let n = unsafe { libc::read(self.master.as_raw_fd(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) || error.raw_os_error() == Some(libc::EAGAIN) {
                return true;
            }
            return false; // EIO: the slave side is gone
        }
        if n == 0 {
            return false;
        }
        self.output.extend_from_slice(&buffer[..n as usize]);
        // Be the terminal: answer the background colour query.
        if let Some(appearance) = self.appearance.clone() {
            while let Some(found) = find(&self.output[self.answered..], &self.query) {
                self.answered += found + self.query.len();
                self.send(&appearance);
            }
        }
        true
    }

    pub fn send(&self, data: &[u8]) {
        let mut rest = data;
        while !rest.is_empty() {
            let n = unsafe { libc::write(self.master.as_raw_fd(), rest.as_ptr().cast(), rest.len()) };
            if n <= 0 {
                return;
            }
            rest = &rest[n as usize..];
        }
    }

    /// Read until nothing comes for `quiet` seconds, or the pty closes.
    pub fn drain(&mut self, quiet: f64) {
        loop {
            let before = self.output.len();
            if !self.pump(quiet) || self.output.len() == before {
                return;
            }
        }
    }

    pub fn resize(&self, width: u16, height: u16) {
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &winsize(width, height));
            libc::killpg(self.child.id() as i32, libc::SIGWINCH);
        }
    }

    /// Whether the plugin has exited, collecting its return code.
    pub fn reaped(&mut self) -> bool {
        if self.status.is_some() {
            return true;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.status = Some(returncode(status));
                true
            }
            _ => false,
        }
    }

    pub fn wait_reaped(&mut self, timeout: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(timeout);
        while Instant::now() < deadline {
            if self.reaped() {
                return true;
            }
            self.pump(0.01);
        }
        self.reaped()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.status.is_none() {
            unsafe { libc::killpg(self.child.id() as i32, libc::SIGKILL) };
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// A plain child that is killed and reaped if left running.
pub struct Guard(pub Child);

impl Guard {
    pub fn wait(mut self) -> std::io::Result<ExitStatus> {
        self.0.wait()
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Ok(None) = self.0.try_wait() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}
