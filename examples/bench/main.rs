//! Time the picker as a user feels it: a real process, a real terminal, a fake Herdr.
//!
//! ```text
//! cargo build --release                                     # the binary under test
//! cargo run --release --example bench                       # the numbers, for target/release
//! cargo run --release --example bench -- --json             # machine-readable
//! cargo run --release --example bench -- --save NAME        # write bench/NAME.json
//! cargo run --release --example bench -- --compare NAME     # side by side with a saved run
//! cargo run --release --example bench -- --cmd "python3 run.py"   # bench another build
//! cargo run --release --example bench -- --runs 20          # more samples, tighter medians
//! ```
//!
//! A port of `scripts/bench_process.py` (in git history), measuring the same
//! things the same way, so `bench/python.json`, recorded with it from the
//! Python release, compares with any run of this one.
//!
//! The parity tests check what the plugin draws; this bench times it, as a
//! black box: interpreter or binary startup, the socket round trip, scanning
//! the roots, the terminal handshake, and the detached child that starts the
//! agent are where most of the felt cost lives. It starts the plugin on a
//! pseudo-terminal, serves it a synthetic Herdr session over a Unix socket and
//! a synthetic home on disk, types at it, and times what comes back. Because
//! nothing here links the plugin, any implementation that speaks the same
//! protocol is measured against the same baseline: `--cmd` names the command.
//! The default is this crate's release binary next to this example (build it
//! first), or `target/release/herdr-newtab-plus`.
//!
//! What is measured, per workload (12, 400 and 4,000 folders under the roots,
//! with a session, recents and a zoxide list that grow alongside):
//!
//! * first frame -- spawn to the moment the whole first frame is on screen
//!   (its footer, the last row, has ended in `help`). The number a user feels
//!   every time they press the chord.
//! * keystroke -- typing a query and deleting it, from writing the key to the
//!   last byte of the frame it produced (the plugin quiet for 20 ms after).
//! * navigation -- the down arrow, which moves the selection and refills the
//!   preview from a folder it has not read yet.
//! * path keystroke -- typing a path into the roots, where one keystroke lists
//!   a directory of every folder in the workload ("listing keystroke").
//! * exit -- esc to process exit.
//! * create -- Enter to Herdr being asked for the tab or space, and to exit.
//! * agent hand-off -- Enter to Herdr being asked to start the agent, which a
//!   detached child of the plugin does after the popup has gone.
//! * cpu, memory, bytes -- user+system CPU and peak RSS of the popup process
//!   (wait4's rusage), and every byte it wrote to the terminal.
//! * herdr calls -- requests the popup made of the server.
//!
//! And two command-line runs: `--version`, the floor of starting the program
//! at all, and `--list`, which builds the catalog and prints it. The pty
//! answers the background colour query (OSC 11) the way most terminals do.
//!
//! POSIX only: pseudo-terminals and Unix sockets are what is being timed.

#[cfg(not(unix))]
fn main() {
    eprintln!("bench needs a Unix pseudo-terminal");
    std::process::exit(1);
}

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../src/par.rs"]
mod par;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../src/pyjson.rs"]
mod pyjson;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../src/pystr.rs"]
mod pystr;
#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../src/unicode_tables.rs"]
#[rustfmt::skip]
mod unicode_tables;

#[cfg(unix)]
#[allow(dead_code)]
#[path = "../../tests/parity/herdr.rs"]
mod herdr;

#[cfg(unix)]
mod machine;
#[cfg(unix)]
mod pty;
#[cfg(unix)]
mod report;

#[cfg(unix)]
fn main() {
    std::process::exit(unix::main());
}

#[cfg(unix)]
mod unix {
    use crate::machine::Machine;
    use crate::pty::{ms, Session, TIMEOUT};
    use crate::pyjson::{self, Map, Value};
    use crate::report::{percentile, report, to_json, WORKLOADS};
    use std::os::unix::process::CommandExt as _;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Instant;

    const ROOT: &str = env!("CARGO_MANIFEST_DIR");
    const BINARY: &str = "herdr-newtab-plus";
    const QUERY: &[u8] = b"proj";
    // The footer is the last row of every frame, and this is the end of it.
    const FRAME_DONE: &[u8] = b"help";
    const DOWN: &[u8] = b"\x1b[B";
    const SHIFT_TAB: &[u8] = b"\x1b[Z";
    const ESC: &[u8] = b"\x1b";
    const ENTER: &[u8] = b"\r";
    const BACKSPACE: &[u8] = b"\x7f";
    const CTRL_U: &[u8] = b"\x15";
    const USAGE: &str = "usage: cargo run --release --example bench -- \
                         [--runs N] [--cmd \"COMMAND ...\"] [--save NAME] [--compare NAME] [--json]";

    /// The command under test, resolved once: what it was given as, for the
    /// record, and the program to run.
    struct Subject {
        shown: Vec<String>,
        program: PathBuf,
        argv: Vec<String>,
    }

    impl Subject {
        fn argv_with(&self, flag: &str) -> Vec<String> {
            let mut argv = self.argv.clone();
            argv.push(flag.into());
            argv
        }
    }

    type Outcome<T> = Result<T, String>;

    fn dict(entries: Vec<(&str, Value)>) -> Value {
        Value::Dict(entries.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Map>())
    }

    // -- scenarios ------------------------------------------------------------

    struct Browse {
        first_frame_ms: f64,
        keystroke_ms: Vec<f64>,
        nav_ms: Vec<f64>,
        path_ms: Vec<f64>,
        exit_ms: f64,
        cpu_ms: f64,
        peak_kb: i64,
        bytes: usize,
        calls: usize,
    }

    fn keys(session: &mut Session, keys: &[&[u8]]) -> Vec<f64> {
        keys.iter().filter_map(|key| session.key(key)).collect()
    }

    fn each_byte(text: &[u8]) -> Vec<&[u8]> {
        text.chunks(1).collect()
    }

    /// Open, type and delete a query, walk the list, complete a path, then esc.
    fn browse(subject: &Subject, machine: &Machine) -> Outcome<Browse> {
        machine.server.reset();
        let mut session = Session::spawn(&subject.program, &subject.argv, machine.env())?;
        let first_frame_ms = ms(session.wait_for(FRAME_DONE, 0)? - session.started);
        session.settle(0.15);
        let mut keystroke_ms = keys(&mut session, &each_byte(QUERY));
        keystroke_ms.extend(keys(&mut session, &[BACKSPACE; QUERY.len()]));
        let nav_ms = keys(&mut session, &[DOWN; 12]);
        // Into the root: the "/" after "work" is the keystroke that lists it.
        let mut path_ms = keys(&mut session, &each_byte(b"~/work/"));
        path_ms.extend(keys(&mut session, &each_byte(b"pro")));
        session.key(CTRL_U);
        let calls = machine.server.count();
        let sent = session.send(ESC);
        let exited = session.wait_exit()?;
        Ok(Browse {
            first_frame_ms,
            keystroke_ms,
            nav_ms,
            path_ms,
            exit_ms: ms(exited - sent),
            cpu_ms: session.cpu_ms(),
            peak_kb: session.peak_kb(),
            bytes: session.output.len(),
            calls,
        })
    }

    struct Create {
        create_ms: f64,
        exit_ms: f64,
        agent_ms: Option<f64>,
    }

    /// Open, pick the agent chip or the shell, press Enter.
    fn create(subject: &Subject, machine: &Machine, agent: bool) -> Outcome<Create> {
        machine.server.reset();
        let mut session = Session::spawn(&subject.program, &subject.argv, machine.env())?;
        session.wait_for(FRAME_DONE, 0)?;
        session.settle(0.15);
        // The first row is a recent opened with claude, so the chip row is on
        // claude already; shift+tab back to the shell for a create without one.
        if !agent {
            session.key(SHIFT_TAB);
        }
        let sent = session.send(ENTER);
        let exited = session.wait_exit()?;
        let created = machine.server.first("workspace.create").or_else(|| machine.server.first("tab.create"));
        let started = if agent { machine.server.wait_first("agent.start", TIMEOUT) } else { None };
        Ok(Create {
            create_ms: ms(created.unwrap_or(exited).saturating_duration_since(sent)),
            exit_ms: ms(exited - sent),
            agent_ms: started.map(|at| ms(at.saturating_duration_since(sent))),
        })
    }

    /// A command-line run to completion, in milliseconds.
    fn command(subject: &Subject, machine: &Machine, flag: &str) -> f64 {
        let argv = subject.argv_with(flag);
        let start = Instant::now();
        let _ = Command::new(&subject.program)
            .arg0(&argv[0])
            .args(&argv[1..])
            .env_clear()
            .envs(machine.env().iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        ms(start.elapsed())
    }

    // -- the runs -------------------------------------------------------------

    fn run_workload(subject: &Subject, spec: (&str, usize, usize, usize, usize), runs: usize) -> Outcome<Value> {
        let (_, folders, panes, recents, zoxide) = spec;
        let machine = Machine::new(folders, panes, recents, zoxide);
        browse(subject, &machine)?; // warm the page cache and any bytecode caches
        let browses = (0..runs).map(|_| browse(subject, &machine)).collect::<Outcome<Vec<_>>>()?;
        let few = (runs / 2).max(3);
        let creates = (0..few).map(|_| create(subject, &machine, false)).collect::<Outcome<Vec<_>>>()?;
        let agents = (0..few).map(|_| create(subject, &machine, true)).collect::<Outcome<Vec<_>>>()?;
        let lists: Vec<f64> = (0..few).map(|_| command(subject, &machine, "--list")).collect();
        drop(machine);

        let every = |pick: fn(&Browse) -> &Vec<f64>| -> Vec<f64> { browses.iter().flat_map(pick).copied().collect() };
        let each = |pick: fn(&Browse) -> f64| -> Vec<f64> { browses.iter().map(pick).collect() };
        let first_frame = each(|b| b.first_frame_ms);
        let keystroke = every(|b| &b.keystroke_ms);
        let nav = every(|b| &b.nav_ms);
        let path_max: Vec<f64> = browses.iter().filter_map(|b| b.path_ms.iter().copied().reduce(f64::max)).collect();
        let create_ms: Vec<f64> = creates.iter().map(|c| c.create_ms).collect();
        let create_exit: Vec<f64> = creates.iter().map(|c| c.exit_ms).collect();
        let agent_ms: Vec<f64> = agents.iter().filter_map(|c| c.agent_ms).collect();
        let int = |values: Vec<f64>| Value::Int(percentile(&values, 0.5) as i128);
        Ok(dict(vec![
            ("folders", Value::Int(folders as i128)),
            ("panes", Value::Int(panes as i128)),
            ("first_frame_p50_ms", Value::Float(percentile(&first_frame, 0.5))),
            ("first_frame_p90_ms", Value::Float(percentile(&first_frame, 0.9))),
            ("keystroke_p50_ms", Value::Float(percentile(&keystroke, 0.5))),
            ("keystroke_p99_ms", Value::Float(percentile(&keystroke, 0.99))),
            ("nav_p50_ms", Value::Float(percentile(&nav, 0.5))),
            ("nav_p99_ms", Value::Float(percentile(&nav, 0.99))),
            ("path_p50_ms", Value::Float(percentile(&every(|b| &b.path_ms), 0.5))),
            ("path_max_p50_ms", Value::Float(percentile(&path_max, 0.5))),
            ("exit_p50_ms", Value::Float(percentile(&each(|b| b.exit_ms), 0.5))),
            ("create_p50_ms", Value::Float(percentile(&create_ms, 0.5))),
            ("create_exit_p50_ms", Value::Float(percentile(&create_exit, 0.5))),
            ("agent_p50_ms", Value::Float(percentile(&agent_ms, 0.5))),
            ("cpu_p50_ms", Value::Float(percentile(&each(|b| b.cpu_ms), 0.5))),
            ("peak_p50_kb", int(each(|b| b.peak_kb as f64))),
            ("bytes_p50", int(each(|b| b.bytes as f64))),
            ("herdr_calls_p50", int(each(|b| b.calls as f64))),
            ("list_p50_ms", Value::Float(percentile(&lists, 0.5))),
        ]))
    }

    fn run_all(subject: &Subject, runs: usize) -> Outcome<Value> {
        let mut results = Map::new();
        for spec in WORKLOADS {
            eprintln!("bench: {} ({} folders)", spec.0, spec.1);
            results.insert(spec.0, run_workload(subject, spec, runs)?);
        }
        let machine = Machine::new(1, 1, 0, 0);
        command(subject, &machine, "--version");
        let versions: Vec<f64> = (0..runs.max(5)).map(|_| command(subject, &machine, "--version")).collect();
        results.insert("version_p50_ms", Value::Float(percentile(&versions, 0.5)));
        Ok(Value::Dict(results))
    }

    // -- the command and the machine ------------------------------------------

    /// `shlex.split`: words split on whitespace, with POSIX shell quoting.
    fn split(text: &str) -> Outcome<Vec<String>> {
        let mut words = Vec::new();
        let mut word: Option<String> = None;
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            match c {
                c if c.is_whitespace() => words.extend(word.take()),
                '\'' => {
                    let word = word.get_or_insert_with(String::new);
                    loop {
                        match chars.next() {
                            Some('\'') => break,
                            Some(c) => word.push(c),
                            None => return Err("--cmd: no closing quotation".into()),
                        }
                    }
                }
                '"' => {
                    let word = word.get_or_insert_with(String::new);
                    loop {
                        match chars.next() {
                            Some('"') => break,
                            Some('\\') => match chars.next() {
                                Some(c @ ('\\' | '"' | '$' | '`' | '\n')) => word.push(c),
                                Some(c) => {
                                    word.push('\\');
                                    word.push(c);
                                }
                                None => return Err("--cmd: no closing quotation".into()),
                            },
                            Some(c) => word.push(c),
                            None => return Err("--cmd: no closing quotation".into()),
                        }
                    }
                }
                '\\' => match chars.next() {
                    Some(c) => word.get_or_insert_with(String::new).push(c),
                    None => return Err("--cmd: no escaped character".into()),
                },
                c => word.get_or_insert_with(String::new).push(c),
            }
        }
        words.extend(word);
        Ok(words)
    }

    /// `shutil.which`: a name without a slash, looked up on this process's PATH.
    fn which(name: &str) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        if name.contains('/') {
            return Some(PathBuf::from(name));
        }
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path).map(|dir| dir.join(name)).find(|candidate| {
            std::fs::metadata(candidate).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    }

    /// This crate's release binary: beside this example's own target
    /// directory (so `CARGO_TARGET_DIR` is honoured), else under `target/`.
    fn default_binary() -> PathBuf {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent()?.parent().map(Path::to_path_buf))
            .filter(|dir| dir.file_name().is_some_and(|name| name == "release"))
            .map(|dir| dir.join(BINARY));
        beside.filter(|path| path.exists()).unwrap_or_else(|| Path::new(ROOT).join("target/release").join(BINARY))
    }

    /// A relative part of the command made absolute: against the working
    /// directory if it names something there, else against the checkout (as
    /// `python3 run.py` was run from it).
    fn anchor(part: &str) -> String {
        let path = Path::new(part);
        if path.is_absolute() || part.starts_with('-') {
            return part.into();
        }
        if let Ok(cwd) = std::env::current_dir() {
            if part.contains('/') && cwd.join(path).exists() {
                return cwd.join(path).to_string_lossy().into_owned();
            }
        }
        let inside = Path::new(ROOT).join(path);
        if inside.exists() {
            return inside.to_string_lossy().into_owned();
        }
        part.into()
    }

    fn subject(cmd: Option<&str>) -> Outcome<Subject> {
        let argv: Vec<String> = match cmd {
            Some(text) => split(text)?.iter().map(|part| anchor(part)).collect(),
            None => vec![default_binary().to_string_lossy().into_owned()],
        };
        let Some(first) = argv.first() else { return Err("--cmd is empty".into()) };
        let program = which(first).ok_or_else(|| format!("{}: not found on PATH", first))?;
        if !program.exists() {
            let hint = if cmd.is_none() { " (build it first: cargo build --release)" } else { "" };
            return Err(format!("{}: no such file{}", program.display(), hint));
        }
        let shown = argv
            .iter()
            .map(|part| match Path::new(part).strip_prefix(ROOT) {
                Ok(relative) if Path::new(part).is_absolute() => relative.to_string_lossy().into_owned(),
                _ => part.clone(),
            })
            .collect();
        Ok(Subject { shown, program, argv })
    }

    fn machine_info(subject: &Subject) -> Value {
        let mut uts: libc::utsname = unsafe { std::mem::zeroed() };
        unsafe { libc::uname(&mut uts) };
        let arch = unsafe { std::ffi::CStr::from_ptr(uts.machine.as_ptr()) }.to_string_lossy().into_owned();
        let cpus = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
        let platform = match std::env::consts::OS {
            "macos" => "darwin", // sys.platform's name, as the Python bench recorded it
            other => other,
        };
        let mut info = Map::new();
        info.insert("platform", Value::str(platform));
        info.insert("machine", Value::Str(arch));
        info.insert("cpus", Value::Int(cpus as i128));
        info.insert("bench", Value::str("rust"));
        // As the Python bench noted the interpreter it ran the plugin with.
        let name = subject.program.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if name.starts_with("python") {
            let version = Command::new(&subject.program)
                .args(["-c", "import sys; print(sys.version.split()[0])"])
                .stderr(Stdio::null())
                .output();
            if let Ok(out) = version {
                let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
                info.insert("plugin_python", Value::Str(text));
            }
        }
        Value::Dict(info)
    }

    /// Where `--save NAME` writes and `--compare NAME` reads; a name ending
    /// in `.json` is taken as a path.
    fn saved_path(name: &str) -> PathBuf {
        if name.ends_with(".json") {
            return PathBuf::from(name);
        }
        Path::new(ROOT).join("bench").join(format!("{}.json", name))
    }

    fn shown_path(path: &Path) -> String {
        path.strip_prefix(ROOT).unwrap_or(path).to_string_lossy().into_owned()
    }

    fn option<'a>(args: &'a [String], flag: &str) -> Outcome<Option<&'a str>> {
        match args.iter().position(|a| a == flag) {
            None => Ok(None),
            Some(index) => {
                args.get(index + 1).map(|v| Some(v.as_str())).ok_or_else(|| format!("{} needs a value", flag))
            }
        }
    }

    fn run(args: &[String]) -> Outcome<()> {
        const FLAGS: [&str; 4] = ["--cmd", "--runs", "--save", "--compare"];
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                flag if FLAGS.contains(&flag) => index += 2,
                "--json" => index += 1,
                "-h" | "--help" => {
                    println!("{}", USAGE);
                    return Ok(());
                }
                other => return Err(format!("unknown argument {:?}\n{}", other, USAGE)),
            }
        }
        let subject = subject(option(args, "--cmd")?)?;
        let runs = match option(args, "--runs")? {
            Some(text) => text.parse::<usize>().map_err(|_| format!("--runs: not a count: {}", text))?,
            None => 12,
        };
        // Read before the run, so a missing baseline costs nothing.
        let baseline = match option(args, "--compare")? {
            Some(name) => {
                let path = saved_path(name);
                let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {}", path.display(), e))?;
                Some(pyjson::loads(&text).map_err(|_| format!("{}: not JSON", path.display()))?)
            }
            None => None,
        };

        let data = dict(vec![
            ("cmd", Value::List(subject.shown.iter().map(|p| Value::Str(p.clone())).collect())),
            ("runs", Value::Int(runs as i128)),
            ("machine", machine_info(&subject)),
            ("results", run_all(&subject, runs)?),
        ]);

        if args.iter().any(|a| a == "--json") {
            println!("{}", to_json(&data));
            return Ok(());
        }
        print!("{}", report(&data, baseline.as_ref()));
        if let Some(name) = option(args, "--save")? {
            let target = saved_path(name);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {}", parent.display(), e))?;
            }
            std::fs::write(&target, to_json(&data) + "\n").map_err(|e| format!("{}: {}", target.display(), e))?;
            print!("  saved {}\n\n", shown_path(&target));
        }
        Ok(())
    }

    pub fn main() -> i32 {
        let args: Vec<String> = std::env::args().skip(1).collect();
        match run(&args) {
            Ok(()) => 0,
            Err(message) => {
                eprintln!("bench: {}", message);
                1
            }
        }
    }
}
