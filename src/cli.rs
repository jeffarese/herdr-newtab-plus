//! Command line entry point.
//!
//! The popup's whole job is to answer two questions and get out of the way, so
//! the order matters: the tab or space is created while the user is still
//! looking at the picker's last frame, and only the agent -- which Herdr waits
//! on until it is genuinely ready for input, and that can take many seconds --
//! is handed to a detached child. The popup closes on the tab appearing, not
//! on the agent booting.

use crate::agents::{self, Agent};
use crate::app::{Choice, Picker};
use crate::client::{HerdrClient, HerdrError};
use crate::config::Config;
use crate::icons;
use crate::paths::{self, Row};
use crate::pyjson::{self, Map, Value};
use crate::pyos;
use crate::session::{agent_name, created_ids, focused_cwd, live_agent_names, TAB};
use crate::state::Recents;
use crate::term::{self, Terminal};
use crate::theme::{default_selection_background, Theme};
use std::io::Write;
use std::time::{Duration, Instant};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const USAGE: &str = "herdr-newtab-plus - new tab, with a folder and an optional agent

usage: herdr-newtab-plus [options]

options:
  --list      print the folders the picker would offer, as JSON
  --doctor    print environment diagnostics
  --version   print the plugin version
  --help      show this message

Normally Herdr launches this itself:
  herdr plugin action invoke herdr-newtab-plus.open
";

/// Hidden: how the detached child re-enters to start the agent.
pub const START_AGENT: &str = "--start-agent";

fn state_dir() -> Option<String> {
    std::env::var("HERDR_PLUGIN_STATE_DIR").ok().filter(|d| !d.is_empty())
}

pub fn state_path() -> Option<String> {
    state_dir().map(|dir| pyos::join(&dir, "recent.json"))
}

pub fn log_path() -> Option<String> {
    state_dir().map(|dir| pyos::join(&dir, "agent-start.log"))
}

fn fail(message: &str) -> i32 {
    let _ = writeln!(std::io::stderr(), "herdr-newtab-plus: {}", message);
    2
}

fn out(text: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(text.as_bytes());
    let _ = stdout.flush();
}

fn catalog(client: &mut HerdrClient, config: &Config, recents: &Recents) -> Result<(Value, Vec<Row>), HerdrError> {
    let snapshot = client.snapshot()?;
    let catalog = paths::build_catalog(
        &snapshot,
        &config.roots,
        &recents.folders(),
        config.hidden,
        config.wants_zoxide(paths::zoxide_installed()),
    );
    Ok((snapshot, catalog))
}

/// Which offered agents ctrl+Y can actually arm, and with what.
fn skip_summary(chips: &[Agent], config: &Config) -> String {
    chips
        .iter()
        .filter_map(|chip| {
            let flags = agents::skip_permission_flags(&chip.kind, &config.skip_permission_flags);
            if flags.is_empty() {
                None
            } else {
                Some(format!("{} ({})", chip.label, flags.join(" ")))
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The mode in force, and whether the toggle or the config file set it.
fn create_mode_line(config: &Config, recents: &Recents) -> String {
    match recents.create_mode() {
        None => format!("{} (config)", config.create),
        Some(remembered) => format!("{} (remembered; config says {})", remembered, config.create),
    }
}

fn icons_summary(config: &Config) -> String {
    let found = icons::font_available();
    if config.agent_icons == "none" {
        return "off (agent_icons is \"none\")".to_string();
    }
    if config.agent_icons == "font" {
        return format!("on, forced ({} {} here)", icons::FAMILY, if found { "found" } else { "not found" });
    }
    if found {
        return format!("on ({} found)", icons::FAMILY);
    }
    format!("off ({} not installed; see the README)", icons::FAMILY)
}

fn preview_summary(config: &Config) -> &'static str {
    if pyjson::py_eq(&config.preview, &Value::Bool(true)) {
        "on"
    } else if pyjson::py_eq(&config.preview, &Value::Bool(false)) {
        "off until ctrl+o"
    } else {
        "auto (when the popup is wide enough)"
    }
}

fn doctor(client: &mut HerdrClient, config: &Config) -> i32 {
    let recents = Recents::new(state_path());
    let chips = agents::available(&config.agents);
    let skip = skip_summary(&chips, config);
    let mut lines = vec![
        format!("herdr-newtab-plus {}", VERSION),
        format!("runtime          rust ({}-{})", std::env::consts::ARCH, std::env::consts::OS),
        format!("herdr bin        {}", client.bin_path),
        format!("socket           {}", client.socket_path.clone().unwrap_or_else(|| "(unset)".to_string())),
        format!("socket usable    {}", if client.socket_ok() { "yes" } else { "no, using the CLI" }),
        format!("config           {}", Config::path(None).unwrap_or_else(|| "(no HERDR_PLUGIN_CONFIG_DIR)".to_string())),
        format!("state            {}", state_path().unwrap_or_else(|| "(no HERDR_PLUGIN_STATE_DIR)".to_string())),
        format!("create mode      {}", create_mode_line(config, &recents)),
        format!("roots            {}", config.roots.join(", ")),
        format!("zoxide           {}", if config.wants_zoxide(paths::zoxide_installed()) { "yes" } else { "no" }),
        format!("agents           {}", chips.iter().map(|chip| chip.label.as_str()).collect::<Vec<_>>().join(", ")),
        format!("skip permissions {}", if skip.is_empty() { "(none of the above)".to_string() } else { skip }),
        format!("agent icons      {}", icons_summary(config)),
        format!("preview          {}", preview_summary(config)),
        format!("stdin is a tty   {}", if term::isatty(0) { "yes" } else { "no" }),
    ];
    match catalog(client, config, &recents) {
        Err(error) => {
            lines.push(format!("herdr            unreachable: {}", error));
            out(&(lines.join("\n") + "\n"));
            1
        }
        Ok((snapshot, catalog)) => {
            let version =
                snapshot.as_dict().and_then(|m| m.get("version")).map(pyjson::py_str).unwrap_or_else(|| "?".to_string());
            lines.push(format!("herdr            {}", version));
            lines.push(format!("base cwd         {}", focused_cwd(&snapshot).unwrap_or_else(|| "(unknown)".to_string())));
            let owned = catalog.iter().filter(|item| item.source == paths::SOURCE_SPACE).count();
            lines.push(format!("catalog          {} folders ({} already have a space)", catalog.len(), owned));
            out(&(lines.join("\n") + "\n"));
            0
        }
    }
}

fn list(client: &mut HerdrClient, config: &Config) -> Result<i32, HerdrError> {
    let recents = Recents::new(state_path());
    let (_, catalog) = catalog(client, config, &recents)?;
    let payload: Vec<Value> = catalog
        .iter()
        .map(|item| {
            let mut map = Map::new();
            map.insert("path", Value::str(item.path.clone()));
            map.insert("name", Value::str(item.name.clone()));
            map.insert("source", Value::str(item.source));
            map.insert("detail", Value::str(item.detail.clone()));
            map.insert("git", Value::Bool(item.is_git));
            Value::Dict(map)
        })
        .collect();
    out(&(pyjson::dumps_indent(&Value::List(payload), 2) + "\n"));
    Ok(0)
}

// -- creating --------------------------------------------------------------

/// Make the tab or space and focus it. Returns the new root pane's id.
pub fn create(client: &mut HerdrClient, choice: &Choice) -> Result<String, HerdrError> {
    let result = match choice.target.workspace_id.as_deref() {
        Some(workspace_id) if choice.target.kind == TAB && !workspace_id.is_empty() => {
            client.create_tab(workspace_id, &choice.folder, None)?
        }
        _ => {
            let label = pyos::basename(&choice.folder);
            client.create_workspace(&choice.folder, if label.is_empty() { None } else { Some(label) })?
        }
    };
    let (_, pane_id) = created_ids(&Value::Dict(result));
    match pane_id.filter(|p| !p.is_empty()) {
        Some(pane_id) => Ok(pane_id),
        None => Err(HerdrError::new(format!("herdr created the {} but reported no pane to use", choice.target.kind))),
    }
}

/// Where the plugin lives: the folder above `bin/`, where Herdr runs it from.
fn plugin_root(exe: &std::path::Path) -> Option<std::path::PathBuf> {
    let dir = exe.parent()?;
    if dir.file_name().map(|name| name == "bin").unwrap_or(false) {
        return dir.parent().map(|p| p.to_path_buf());
    }
    Some(dir.to_path_buf())
}

/// Hand the agent start to a detached child so the popup can close now.
pub fn spawn_agent(choice: &Choice, pane_id: &str, name: &str, extra: &[String]) {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut request = Map::new();
    request.insert("name", Value::str(name));
    request.insert("kind", Value::str(choice.agent.kind.clone()));
    request.insert("pane", Value::str(pane_id));
    request.insert("args", Value::List(extra.iter().map(|a| Value::str(a.clone())).collect()));
    let payload = pyjson::dumps(&Value::Dict(request), false);

    let exe = std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("herdr-newtab-plus"));
    let mut command = Command::new(&exe);
    command.arg(START_AGENT).arg(&payload).stdin(Stdio::null());

    let log = log_path().and_then(|target| {
        let directory = pyos::dirname(&target);
        if !directory.is_empty() {
            std::fs::create_dir_all(&directory).ok()?;
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&target).ok()
    });
    match log.as_ref().and_then(|file| Some((file.try_clone().ok()?, file.try_clone().ok()?))) {
        Some((stdout, stderr)) => {
            command.stdout(stdout).stderr(stderr);
        }
        None => {
            command.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    for variable in agents::PLUGIN_VARIABLES {
        command.env_remove(variable);
    }
    if let Some(root) = plugin_root(&exe) {
        command.current_dir(root);
    }
    // A session of its own, so closing the popup's pty cannot take it down.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    if let Err(error) = command.spawn() {
        let _ = writeln!(
            std::io::stderr(),
            "herdr-newtab-plus: could not start {}: {}",
            choice.agent.kind,
            crate::client::os_error_str(&error)
        );
    }
}

// A pane is born running a shell's startup files, and Herdr refuses to hand it
// to an agent until that shell owns the foreground and is idle. A heavy zshrc
// takes a moment, so the detached child waits it out rather than giving up on
// the first no.
pub const SHELL_WAIT_SECONDS: f64 = 25.0;
pub const SHELL_POLL_SECONDS: f64 = 0.35;
pub const NAME_ATTEMPTS: u32 = 4;

/// Whether the pane is merely not ready yet, as opposed to unusable.
fn transient(message: &str) -> bool {
    let text = crate::pystr::lower(message);
    text.contains("available shell") || text.contains("foreground") || text.contains("not ready")
}

struct StartRequest {
    name: String,
    kind: String,
    pane: String,
    extra: Vec<String>,
}

fn parse_request(payload: &str) -> Result<StartRequest, String> {
    let request = pyjson::loads(payload).map_err(|_| "invalid JSON".to_string())?;
    let Value::Dict(map) = &request else {
        return Err("payload must be an object".to_string());
    };
    let field = |key: &str| -> Result<String, String> {
        map.get(key).map(pyjson::py_str).ok_or_else(|| pyjson::str_repr(key))
    };
    let name = field("name")?;
    let kind = field("kind")?;
    let pane = field("pane")?;
    let extra = match map.get("args") {
        None => Vec::new(),
        Some(value) if !value.truthy() => Vec::new(),
        Some(Value::List(items)) => items.iter().map(pyjson::py_str).collect(),
        Some(_) => return Err("args must be a list".to_string()),
    };
    Ok(StartRequest { name, kind, pane, extra })
}

/// The detached child: wait for the shell, then start the agent.
///
/// Runs after the popup is gone, so there is nobody to report to but the log.
/// Both things that realistically go wrong here -- the shell still starting
/// up, and a name another agent took first -- are worth retrying.
fn start_agent(payload: &str) -> i32 {
    let request = match parse_request(payload) {
        Ok(request) => request,
        Err(error) => return fail(&format!("bad --start-agent payload: {}", error)),
    };
    let StartRequest { mut name, kind, pane, extra } = request;

    let mut client = HerdrClient::new(None, None, 90.0);
    let deadline = Instant::now() + Duration::from_secs_f64(SHELL_WAIT_SECONDS);
    let mut renames = 0;

    let last = loop {
        let Err(error) = client.start_agent(&name, &kind, &pane, 60000, &extra) else { return 0 };
        let message = error.to_string();
        if transient(&message) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_secs_f64(SHELL_POLL_SECONDS));
            continue;
        }
        if crate::pystr::lower(&message).contains("name") && renames < NAME_ATTEMPTS {
            renames += 1;
            let mut taken = client.snapshot().map(|s| live_agent_names(&s)).unwrap_or_default();
            taken.insert(name.clone());
            let mut sorted: Vec<String> = taken.into_iter().collect();
            sorted.sort();
            name = agent_name(&name, &kind, &sorted);
            continue;
        }
        break error;
    };
    fail(&format!("could not start {} in {}: {}", kind, pane, last))
}

// -- entry -----------------------------------------------------------------

pub fn main(args: Vec<String>) -> i32 {
    if let Some(index) = args.iter().position(|a| a == START_AGENT) {
        return match args.get(index + 1) {
            None => fail(&format!("{} needs a payload", START_AGENT)),
            Some(payload) => start_agent(payload),
        };
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        out(USAGE);
        return 0;
    }
    if args.iter().any(|a| a == "--version") {
        out(&format!("herdr-newtab-plus {}\n", VERSION));
        return 0;
    }

    let config = Config::load(None);
    let mut client = HerdrClient::from_env();

    if args.iter().any(|a| a == "--doctor") {
        return doctor(&mut client, &config);
    }
    if args.iter().any(|a| a == "--list") {
        return match list(&mut client, &config) {
            Ok(code) => code,
            Err(error) => fail(&error.to_string()),
        };
    }

    if !term::isatty(0) || !term::isatty(1) {
        return fail("needs a terminal; run it from Herdr with `plugin pane open`");
    }

    let recents = Recents::new(state_path());
    if config.agent_icons == "auto" {
        icons::prefetch(); // overlaps the snapshot request rather than following it
    }
    let (snapshot, catalog) = match catalog(&mut client, &config, &recents) {
        Ok(found) => found,
        Err(error) => return fail(&error.to_string()),
    };

    // One appearance query, and only when something still depends on it: it
    // costs a round trip to the terminal on every open.
    let mut selection_background = config.selection_background.clone();
    let auto = pyjson::py_eq(&selection_background, &Value::str("auto"));
    let appearance = if auto || !config.colors_has("muted") { term::detect_appearance(0.12) } else { None };
    if auto {
        selection_background = match default_selection_background(appearance) {
            Some(spec) => Value::str(spec),
            None => Value::Null,
        };
    }
    let mut theme = Theme::new(Some(&config.colors), Some(&selection_background), appearance);
    theme.agent_icons = icons::enabled(config.agent_icons);

    let chips = agents::available(&config.agents);
    let base = focused_cwd(&snapshot).filter(|b| !b.is_empty()).unwrap_or_else(paths::home);
    let mouse = config.mouse;
    let skip_flags = config.skip_permission_flags.clone();
    let mut picker = Picker::new(snapshot, catalog, chips, config, recents, theme, base);

    let choice = match Terminal::enter(mouse) {
        Ok(mut terminal) => picker.run(&mut terminal),
        Err(error) => return fail(&crate::client::os_error_str(&error)),
    };

    let Some(choice) = choice else {
        // Cancelling abandons the pick, not the preference: the create mode is
        // a setting, and a toggle that quietly reverted on escape would be one
        // you could never trust. A no-op when nothing was touched.
        picker.recents.save();
        return 0;
    };

    let pane_id = match create(&mut client, &choice) {
        Ok(pane_id) => pane_id,
        Err(error) => return fail(&error.to_string()),
    };

    if !choice.agent.is_none() {
        let mut taken: Vec<String> = live_agent_names(&picker.snapshot).into_iter().collect();
        taken.sort();
        let name = agent_name(&choice.folder, &choice.agent.kind, &taken);
        let extra = if choice.skip_permissions {
            agents::skip_permission_flags(&choice.agent.kind, &skip_flags)
        } else {
            Vec::new()
        };
        spawn_agent(&choice, &pane_id, &name, &extra);
    }

    picker.recents.touch(&choice.folder, &choice.agent.kind);
    picker.recents.save();
    0
}
