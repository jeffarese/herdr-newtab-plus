//! Film the picker choosing a folder and an agent, then the same picker
//! over 4,000 folders, captioned with the bench's medians for that size.
//!
//! ```text
//! cargo run --release --features demo --example record_demo             # demo/herdr-newtab-plus.mp4 + .gif
//! cargo run --release --features demo --example record_demo -- --no-gif
//! cargo run --release --features demo --example record_demo -- --stills # one PNG per scene, no video
//! cargo run --release --features demo --example record_demo -- --frame 120 # one PNG, for tweaking
//! ```
//!
//! Nothing is simulated on the picker's side: the release binary runs on a
//! pseudo-terminal in a synthetic home, against a fake Herdr on a Unix socket
//! (`machine.rs`), the script types real keys with real pauses, and a sampler
//! photographs the screen model 20 times a second. What the picker takes to
//! draw, it takes on film too, and its flash messages expire on its own
//! clock. The frames go to ffmpeg as raw rgb24.
//!
//! The popup is filmed at the size Herdr actually gives it -- 107x28, which is
//! `height - 2` rows and `width - 3` columns of the manifest's 110x30 -- blank
//! rows included.

#[cfg(not(unix))]
fn main() {
    eprintln!("record_demo needs a Unix pseudo-terminal");
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
#[allow(dead_code)]
#[path = "../../tests/parity/screen.rs"]
mod screen;

#[cfg(unix)]
mod machine;
#[cfg(unix)]
mod paint;
#[cfg(unix)]
mod pty;

#[cfg(unix)]
fn main() {
    unix::main();
}

#[cfg(unix)]
mod unix {
    use crate::herdr::Herdr;
    use crate::machine::{Machine, CROWD_FOLDERS};
    use crate::paint::{sgr_color, Cell, Frame, Painter, CHIP_TTL, COLS, DEFAULT_FG, PANEL_BG, ROWS};
    use crate::pty::Session;
    use crate::pyjson::{self, Value};
    use crate::screen::Screen;
    use std::io::Write;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    const ROOT: &str = env!("CARGO_MANIFEST_DIR");
    const NAME: &str = "herdr-newtab-plus";
    const FPS: u32 = 20;
    const GIF_WIDTH: u32 = 1080;
    const GIF_FPS: u32 = 12;

    fn keycap(name: &str) -> &str {
        match name {
            "up" => "↑",
            "down" => "↓",
            "left" => "←",
            "right" => "→",
            "tab" => "⇥",
            "shift+tab" => "⇧⇥",
            "enter" => "⏎",
            "backspace" => "⌫",
            "delete" => "⌦",
            "ctrl+o" => "^O",
            "ctrl+u" => "^U",
            "ctrl+w" => "^W",
            "ctrl+r" => "^R",
            "ctrl+t" => "^T",
            "ctrl+y" => "^Y",
            "pgdn" => "⇟",
            "pgup" => "⇞",
            other => other,
        }
    }

    /// The bytes a terminal sends for a key, as src/keys.rs decodes them.
    fn key_bytes(name: &str) -> &'static [u8] {
        match name {
            "up" => b"\x1b[A",
            "down" => b"\x1b[B",
            "right" => b"\x1b[C",
            "left" => b"\x1b[D",
            "tab" => b"\t",
            "shift+tab" => b"\x1b[Z",
            "enter" => b"\r",
            "backspace" => b"\x7f",
            "delete" => b"\x1b[3~",
            "esc" => b"\x1b",
            "ctrl+o" => b"\x0f",
            "ctrl+u" => b"\x15",
            "ctrl+w" => b"\x17",
            "ctrl+r" => b"\x12",
            "ctrl+t" => b"\x14",
            "ctrl+y" => b"\x19",
            "pgup" => b"\x1b[5~",
            "pgdn" => b"\x1b[6~",
            other => panic!("no bytes for key {}", other),
        }
    }

    fn binary() -> PathBuf {
        let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
        let status = Command::new(cargo)
            .args(["build", "--release", "--bin", NAME])
            .current_dir(ROOT)
            .status()
            .expect("run cargo build");
        let path = Path::new(ROOT).join("target/release").join(NAME);
        if !status.success() || !path.exists() {
            panic!("could not build {}; run `cargo build --release` first", path.display());
        }
        path
    }

    /// What the camera films: the screen the current picker draws on, and
    /// the Herdr whose requests say what Enter did.
    struct Stage {
        screen: Arc<Mutex<Screen>>,
        herdr: Arc<Herdr>,
        labels: Vec<(String, String)>,
    }

    /// What the camera sees between frames.
    struct Live {
        caption: String,
        chip: Option<(String, Instant)>,
        /// Where each caption starts, by frame index.
        scenes: Vec<(usize, String)>,
    }

    struct Recorder {
        start: Instant,
        /// Script time: every pause is measured from `start`, so the pauses
        /// add up to the script's however long each key takes to send.
        at: f64,
        live: Arc<Mutex<Live>>,
        frames: Arc<Mutex<Vec<Frame>>>,
        stage: Arc<Mutex<Stage>>,
        session: Option<Session>,
        /// Pickers that have closed, kept so the agent each handed off can
        /// still finish talking to its Herdr.
        closed: Vec<Session>,
    }

    impl Recorder {
        fn say(&self, caption: &str) {
            let index = self.frames.lock().unwrap().len();
            let mut live = self.live.lock().unwrap();
            live.caption = caption.to_string();
            live.scenes.push((index, caption.to_string()));
        }

        fn hold(&mut self, seconds: f64) {
            self.at += seconds;
            let target = self.start + Duration::from_secs_f64(self.at);
            let now = Instant::now();
            if target > now {
                std::thread::sleep(target - now);
            }
        }

        /// Start a picker on a blank screen of its own.
        fn open(&mut self, command: Command) {
            let screen = Arc::new(Mutex::new(Screen::new(COLS, ROWS)));
            self.stage.lock().unwrap().screen = screen.clone();
            self.closed.extend(self.session.take());
            self.session = Some(Session::spawn(command, COLS as u16, ROWS as u16, screen));
        }

        /// Cut to an empty panel in front of another Herdr.
        fn cut(&mut self, herdr: Arc<Herdr>, labels: Vec<(String, String)>) {
            self.clear();
            let mut stage = self.stage.lock().unwrap();
            stage.herdr = herdr;
            stage.labels = labels;
        }

        /// Cut to an empty panel, as Herdr does when a popup closes.
        fn clear(&mut self) {
            self.stage.lock().unwrap().screen = Arc::new(Mutex::new(Screen::new(COLS, ROWS)));
        }

        fn send(&mut self, bytes: &[u8], chip: &str, dwell: f64) {
            if let Some(session) = &self.session {
                session.send(bytes);
            }
            self.live.lock().unwrap().chip = Some((chip.to_string(), Instant::now()));
            self.hold(dwell);
        }

        fn key(&mut self, name: &str, dwell: f64) {
            self.send(key_bytes(name), keycap(name), dwell);
        }

        fn repeat(&mut self, name: &str, times: usize, dwell: f64) {
            for _ in 0..times {
                self.key(name, dwell);
            }
        }

        fn type_text(&mut self, text: &str, dwell: f64) {
            for c in text.chars() {
                let mut buffer = [0u8; 4];
                self.send(c.encode_utf8(&mut buffer).as_bytes(), &c.to_string(), dwell);
            }
        }
    }

    /// The bench's medians for the large workload, from the saved run the
    /// README's table quotes. A recording shares the machine with whatever
    /// else runs on it, so timings taken while filming say more about the
    /// load than about the picker.
    struct Timings {
        runs: i128,
        first_frame: f64,
        keystroke: f64,
        listing: f64,
    }

    fn timings() -> Timings {
        let path = Path::new(ROOT).join("bench/rust.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {}", path.display(), e));
        let data = pyjson::loads(&text).unwrap_or_else(|_| panic!("{} is not JSON", path.display()));
        let large = data.get("results").get("large");
        let value = |key: &str| match large.get(key) {
            Value::Float(value) => *value,
            Value::Int(value) => *value as f64,
            _ => panic!("{} has no results.large.{}", path.display(), key),
        };
        let Value::Int(runs) = data.get("runs") else { panic!("{} has no runs", path.display()) };
        Timings {
            runs: *runs,
            first_frame: value("first_frame_p50_ms"),
            keystroke: value("keystroke_p50_ms"),
            listing: value("path_max_p50_ms"),
        }
    }

    /// The second act: what the finale films, and in front of which Herdr.
    struct Act {
        command: Command,
        herdr: Arc<Herdr>,
        labels: Vec<(String, String)>,
    }

    /// The script: the tour of what the picker does, then the finale.
    fn perform(r: &mut Recorder, tour: Command, crowd: Act) {
        r.say("one keybinding, and a new tab starts by asking where");
        // The empty panel first, so the popup is seen opening.
        r.hold(0.4);
        r.open(tour);
        r.hold(1.4);

        r.say("before you type: folders you were just in, and the ones already open");
        r.hold(2.4);

        r.say("each row shows the agent it last opened with · ◐ ◉ say one is in there now");
        r.hold(2.6);

        r.say("on the right, the folder itself: its space, who is in it, what is inside");
        r.hold(2.6);

        r.say("↑↓ moves · the agent and the preview follow · the → line says what ⏎ will do");
        r.repeat("down", 4, 0.7);
        r.hold(1.1);

        r.say("^O hides the preview, and brings it back");
        r.key("ctrl+o", 1.4);
        r.key("ctrl+o", 1.0);

        r.say("type a few letters · fuzzy, over every folder it knows");
        r.type_text("relay", 0.15);
        r.hold(2.0);

        r.say("a ~ or a / switches to completing real paths");
        r.key("ctrl+u", 0.3);
        r.type_text("~/workspace/herdr-n", 0.11);
        r.hold(1.9);

        r.say("the guess after the cursor is the picker's · ⇥ takes it");
        r.key("tab", 1.9);

        r.say("a folder taken whole offers itself first, then what is inside it");
        r.hold(1.8);
        r.type_text("s", 0.3);
        r.hold(1.9);

        r.say("^W backs out one segment at a time");
        r.repeat("ctrl+w", 2, 0.75);
        r.hold(1.0);

        // Back to the resting list for the rows that follow: the agent, the
        // skip toggle and the create mode all live under the list, and
        // explaining them over a list filtered down to one folder would waste
        // most of the popup.
        r.say("⇧⇥ picks a different agent for the new tab");
        r.key("ctrl+u", 0.6);
        r.repeat("shift+tab", 4, 0.7);

        r.say("^Y arms it to start with its permission prompts off");
        r.key("ctrl+y", 2.2);

        r.say("...and says so, in the chip and in the line above the hints");
        r.hold(1.4);
        r.key("ctrl+y", 1.0);

        r.say("^T decides where ⏎ lands · a tab, a new space, or you decide");
        r.repeat("ctrl+t", 2, 1.9);
        r.key("ctrl+t", 1.6);

        r.say("? explains all of it without leaving the popup");
        r.type_text("?", 0.2);
        r.hold(5.0);
        r.key("esc", 0.9);

        r.say("⏎ creates it · a tab, because that folder already has a space");
        r.hold(1.4);
        r.key("enter", 0.1);
        r.hold(2.4);

        finale(r, crowd);
    }

    /// The same picker over the bench's large workload, captioned with what
    /// the bench measured at that size.
    fn finale(r: &mut Recorder, crowd: Act) {
        let bench = timings();
        let folders = thousands(CROWD_FOLDERS);
        r.cut(crowd.herdr, crowd.labels);
        r.say(&format!("and at scale: {} folders, 200 panes open, 80 recents, 400 in zoxide", folders));
        r.hold(2.0);
        r.open(crowd.command);
        r.hold(1.2);
        r.say(&format!(
            "its first frame takes {:.1} ms at this size · the median of {} bench runs",
            bench.first_frame, bench.runs
        ));
        r.hold(2.8);

        r.say(&format!("fuzzy over all {} · {:.2} ms a keystroke, the median", folders, bench.keystroke));
        r.type_text("payw", 0.32);
        r.hold(2.4);

        r.say(&format!("a / lists the folder as you type: all {} in {:.2} ms, the median", folders, bench.listing));
        r.key("ctrl+u", 0.4);
        r.type_text("~/code/", 0.14);
        r.hold(0.8);
        r.type_text("aur", 0.28);
        r.hold(2.4);

        r.key("esc", 0.3);
        r.clear();
        r.hold(0.3);
        r.say("herdr plugin install jeffarese/herdr-newtab-plus");
        r.hold(3.0);
    }

    /// 4000 as 4,000.
    fn thousands(value: usize) -> String {
        let digits = value.to_string();
        let mut out = String::new();
        for (index, c) in digits.chars().enumerate() {
            if index > 0 && (digits.len() - index).is_multiple_of(3) {
                out.push(',');
            }
            out.push(c);
        }
        out
    }

    /// What Enter resolved to, in the words the popup would have used: the
    /// create request the fake Herdr received, and the agent started in it.
    fn outcome(changes: &[Value], labels: &[(String, String)]) -> Option<String> {
        let method = |change: &Value| change.list_or_empty().first().and_then(|m| m.as_str()).unwrap_or("").to_string();
        let params = |change: &Value| change.list_or_empty().get(1).cloned().unwrap_or(Value::Null);
        let created = changes.iter().find(|c| matches!(method(c).as_str(), "tab.create" | "workspace.create"))?;
        let params_of = params(created);
        let field = |key: &str| params_of.get(key).as_str().unwrap_or("").to_string();
        let mut text = if method(created) == "tab.create" {
            let id = field("workspace_id");
            let label = labels.iter().find(|(i, _)| *i == id).map(|(_, l)| l.clone()).unwrap_or(id);
            format!("new tab in {}", label)
        } else {
            let cwd = field("cwd");
            let label = field("label");
            format!(
                "new space {}",
                if label.is_empty() { cwd.rsplit('/').next().unwrap_or("").to_string() } else { label }
            )
        };
        if let Some(started) = changes.iter().find(|c| method(c) == "agent.start") {
            text.push_str(&format!("  ·  {}", params(started).get("kind").as_str().unwrap_or("")));
        }
        Some(text)
    }

    /// The screen model, the caption and the chip, frozen into one frame.
    fn snapshot(screen: &Screen, live: &Live, outcome: &str) -> Frame {
        let mut cells = Vec::with_capacity(ROWS * COLS);
        for y in 0..ROWS {
            for x in 0..COLS {
                let (c, fg, bg, bold, underline, reverse) = screen.cell(y, x);
                let mut front = sgr_color(fg).unwrap_or(DEFAULT_FG);
                let mut back = sgr_color(bg);
                if reverse {
                    (front, back) = (back.unwrap_or(PANEL_BG), Some(front));
                }
                cells.push(Cell { ch: c.unwrap_or('\0'), fg: front, bg: back, bold, underline });
            }
        }
        let (chip, chip_age) = match &live.chip {
            Some((label, at)) => {
                let age = at.elapsed().as_secs_f32();
                ((age < CHIP_TTL).then(|| label.clone()), age)
            }
            None => (None, 99.0),
        };
        Frame { cells, caption: live.caption.clone(), chip, chip_age, outcome: outcome.to_string() }
    }

    struct Options {
        out: PathBuf,
        gif: bool,
        stills: bool,
        frame: Option<usize>,
        compact: bool,
    }

    fn options() -> Options {
        let mut options =
            Options { out: Path::new(ROOT).join("demo"), gif: true, stills: false, frame: None, compact: true };
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-gif" => options.gif = false,
                "--stills" => options.stills = true,
                "--frame" => {
                    let index = args.next().and_then(|n| n.parse().ok()).expect("--frame needs a frame number");
                    options.frame = Some(index);
                }
                // The README GIF is compact; this is the roomier framing.
                "--roomy" => options.compact = false,
                "--out" => options.out = PathBuf::from(args.next().expect("--out needs a directory")),
                "-h" | "--help" => {
                    eprintln!("usage: record_demo [--out DIR] [--no-gif] [--stills] [--frame N] [--roomy]");
                    std::process::exit(0);
                }
                other => panic!("unknown argument {}", other),
            }
        }
        options
    }

    fn ffmpeg(args: &[&str]) {
        let status =
            Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).status().expect("ffmpeg not found");
        assert!(status.success(), "ffmpeg {:?} failed", args);
    }

    /// Pipe frames into one ffmpeg as raw rgb24.
    fn pipe(painter: &mut Painter, frames: &[&Frame], output: &[&str]) {
        let size = format!("{}x{}", painter.width, painter.height);
        let mut child = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", &size])
            .args(["-r", &FPS.to_string(), "-i", "-"])
            .args(output)
            .stdin(Stdio::piped())
            .spawn()
            .expect("ffmpeg not found");
        let mut stdin = child.stdin.take().unwrap();
        for (index, frame) in frames.iter().enumerate() {
            stdin.write_all(painter.render(frame)).expect("ffmpeg stopped reading");
            if frames.len() > 1 && index % 50 == 0 {
                eprint!("\r  rendering {}/{}", index, frames.len());
            }
        }
        drop(stdin);
        if frames.len() > 1 {
            eprintln!("\r  rendering {}/{}", frames.len(), frames.len());
        }
        assert!(child.wait().unwrap().success(), "ffmpeg failed");
    }

    fn slug(caption: &str) -> String {
        let mut out = String::new();
        for c in caption.to_lowercase().chars() {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                out.push(c);
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').chars().take(44).collect::<String>().trim_end_matches('-').to_string()
    }

    fn s(path: &Path) -> String {
        path.to_str().unwrap().to_string()
    }

    pub fn main() {
        let options = options();
        let binary = binary();
        // Fail before filming, not after: the logos are drawn in this font.
        if crate::paint::icon_font_path().is_none() {
            panic!("install herdr-radar's Herdr Agent Icons Max font to film the vendor logos");
        }

        // -- the machines -----------------------------------------------------
        let mut machine = Machine::new();
        let herdr: Arc<Herdr> = Arc::new(machine.herdr());
        let labels = machine.labels.clone();
        let command = machine.command(&binary);
        let mut crowd = Machine::crowd();
        let crowd_herdr: Arc<Herdr> = Arc::new(crowd.herdr());
        let act = Act { command: crowd.command(&binary), herdr: crowd_herdr.clone(), labels: crowd.labels.clone() };

        // -- the camera -------------------------------------------------------
        let stage = Arc::new(Mutex::new(Stage {
            screen: Arc::new(Mutex::new(Screen::new(COLS, ROWS))),
            herdr: herdr.clone(),
            labels: labels.clone(),
        }));
        let live = Arc::new(Mutex::new(Live { caption: String::new(), chip: None, scenes: Vec::new() }));
        let frames: Arc<Mutex<Vec<Frame>>> = Arc::new(Mutex::new(Vec::new()));
        let rolling = Arc::new(AtomicBool::new(true));
        let start = Instant::now();
        let camera = {
            let (stage, live, frames, rolling) = (stage.clone(), live.clone(), frames.clone(), rolling.clone());
            std::thread::spawn(move || {
                let mut tick = 0u32;
                while rolling.load(Ordering::SeqCst) {
                    let target = start + Duration::from_secs_f64(tick as f64 / FPS as f64);
                    let now = Instant::now();
                    if target > now {
                        std::thread::sleep(target - now);
                    }
                    let (screen, herdr, labels) = {
                        let stage = stage.lock().unwrap();
                        (stage.screen.clone(), stage.herdr.clone(), stage.labels.clone())
                    };
                    let outcome = outcome(&herdr.changes(), &labels).unwrap_or_default();
                    let frame = {
                        let screen = screen.lock().unwrap();
                        let live = live.lock().unwrap();
                        snapshot(&screen, &live, &outcome)
                    };
                    frames.lock().unwrap().push(frame);
                    tick += 1;
                }
            })
        };

        let mut recorder = Recorder {
            start,
            at: 0.0,
            live: live.clone(),
            frames: frames.clone(),
            stage: stage.clone(),
            session: None,
            closed: Vec::new(),
        };
        perform(&mut recorder, command, act);
        rolling.store(false, Ordering::SeqCst);
        camera.join().unwrap();

        // The agent is started by a detached child after the popup has gone;
        // let it finish talking to the fake Herdr before that goes away.
        herdr.wait_for("agent.start", 5.0);
        let mut tour = recorder.closed.drain(..).next().expect("the script never opened the picker");
        let mut finale = recorder.session.take().expect("the script never opened the finale");
        let exited = (tour.exited(), finale.exited());
        drop((tour, finale));
        let changes = herdr.changes();
        let crowd_changes = crowd_herdr.changes();
        // Each Herdr closes by connecting to its own socket, so the last
        // hold on it goes before the machine that holds the socket.
        drop((recorder, stage));
        drop((herdr, crowd_herdr));
        drop((machine, crowd));
        match (exited, outcome(&changes, &[])) {
            ((Some(status), Some(_)), Some(_)) if status.success() => {}
            (status, outcome) => {
                eprintln!(
                    "warning: the script ended without the picker creating and both exiting ({:?}, {:?})",
                    status, outcome
                )
            }
        }
        if outcome(&crowd_changes, &[]).is_some() {
            eprintln!("warning: the finale created something; it should only look");
        }

        // -- the film ---------------------------------------------------------
        let frames = std::mem::take(&mut *frames.lock().unwrap());
        let scenes = std::mem::take(&mut live.lock().unwrap().scenes);
        let mut painter = Painter::new(options.compact);
        std::fs::create_dir_all(&options.out).unwrap();
        eprintln!(
            "{} frames · {:.1}s · {}x{}",
            frames.len(),
            frames.len() as f64 / FPS as f64,
            painter.width,
            painter.height
        );

        if let Some(index) = options.frame {
            let index = index.min(frames.len() - 1);
            let path = options.out.join(format!("frame-{:05}.png", index));
            pipe(&mut painter, &[&frames[index]], &["-frames:v", "1", &s(&path)]);
            eprintln!("{}", path.display());
            return;
        }

        if options.stills {
            for (order, (start, caption)) in scenes.iter().enumerate() {
                let index = (start + (FPS as f64 * 1.1) as usize).min(frames.len() - 1);
                let path = options.out.join(format!("scene-{:02}-{}.png", order + 1, slug(caption)));
                pipe(&mut painter, &[&frames[index]], &["-frames:v", "1", &s(&path)]);
                eprintln!("{}", path.display());
            }
            return;
        }

        // Lossless first: the GIF palette is built from it, not from h264,
        // whose chroma artefacts turn into palette noise once quantised.
        let work = options.out.join(".frames");
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let master = s(&work.join("master.mkv"));
        let mp4 = s(&options.out.join(format!("{}.mp4", NAME)));
        let gif = s(&options.out.join(format!("{}.gif", NAME)));
        let all: Vec<&Frame> = frames.iter().collect();
        pipe(&mut painter, &all, &["-c:v", "ffv1", &master]);
        ffmpeg(&[
            "-i",
            &master,
            "-c:v",
            "libx264",
            "-preset",
            "slow",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            "-movflags",
            "+faststart",
            &mp4,
        ]);
        eprintln!("{}", mp4);
        if options.gif {
            let palette = s(&work.join("palette.png"));
            let chain = format!("fps={},scale={}:-1:flags=lanczos", GIF_FPS, GIF_WIDTH);
            ffmpeg(&["-i", &master, "-vf", &format!("{},palettegen=stats_mode=diff:max_colors=160", chain), &palette]);
            let use_ = "paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle";
            ffmpeg(&[
                "-i",
                &master,
                "-i",
                &palette,
                "-lavfi",
                &format!("{} [x]; [x][1:v] {}", chain, use_),
                "-loop",
                "0",
                &gif,
            ]);
            eprintln!("{}", gif);
        }
        let _ = std::fs::remove_dir_all(&work);
    }
}
