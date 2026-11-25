//! Which agents this machine can actually start.
//!
//! Herdr's `agent start --kind` accepts a fixed set of kinds and launches its
//! own canonical executable for each. There is no API that reports which of
//! them are installed, so the picker looks for the executable itself and
//! offers only the ones it finds.
//!
//! Order is by how commonly the agent is reached for, not alphabetically,
//! because the row is scanned rather than searched.

use crate::config::Agents;
use crate::pyos;

/// The always-present first chip: open a plain shell.
pub const NONE: &str = "";

#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub kind: String,  // what `agent start --kind` wants; "" for a plain shell
    pub label: String, // what the chip row shows
}

impl Agent {
    pub fn new(kind: &str, label: &str) -> Agent {
        Agent { kind: kind.to_string(), label: label.to_string() }
    }

    pub fn is_none(&self) -> bool {
        self.kind == NONE
    }
}

/// kind -> the executables that mean it is installed. Herdr picks the binary
/// itself at start time; these only decide whether to offer the chip.
pub const KNOWN: &[(&str, &[&str])] = &[
    ("claude", &["claude"]),
    ("codex", &["codex"]),
    ("gemini", &["gemini"]),
    ("cursor", &["cursor-agent", "cursor"]),
    ("opencode", &["opencode"]),
    ("copilot", &["copilot"]),
    ("amp", &["amp"]),
    ("droid", &["droid"]),
    ("grok", &["grok"]),
    ("kimi", &["kimi"]),
    ("pi", &["pi"]),
    ("cline", &["cline"]),
    ("kilo", &["kilo"]),
    ("kiro", &["kiro"]),
    ("devin", &["devin"]),
    ("agy", &["agy"]),
    ("omp", &["omp"]),
    ("mastracode", &["mastracode"]),
    ("hermes", &["hermes"]),
    ("qodercli", &["qodercli", "qoder"]),
    ("maki", &["maki"]),
];

/// The order chips appear in. Anything in KNOWN but missing here follows, sorted.
pub const PREFERRED: &[&str] =
    &["claude", "codex", "gemini", "cursor", "opencode", "copilot", "amp", "droid", "grok", "kimi", "pi"];

pub const NONE_LABEL: &str = "shell";

/// kind -> the arguments that start it with its permission prompts turned off.
///
/// Every CLI spells this differently, and most do not spell it at all, so
/// this is a table of what was checked rather than a pattern. An agent
/// missing from it is not assumed to have an undiscovered spelling: the toggle
/// refuses on that chip and says so.
pub const SKIP_PERMISSION_FLAGS: &[(&str, &[&str])] =
    &[("claude", &["--dangerously-skip-permissions"]), ("codex", &["--dangerously-bypass-approvals-and-sandbox"])];

/// What the chip row appends to say the flag is on.
pub const SKIP_MARK: &str = "!";

pub fn is_known(kind: &str) -> bool {
    KNOWN.iter().any(|(k, _)| *k == kind)
}

/// The arguments that disable permission prompts for one agent kind.
///
/// Empty means this kind has no known way to ask for it, which callers must
/// treat as a refusal rather than as "pass nothing and hope".
pub fn skip_permission_flags(kind: &str, overrides: &[(String, Vec<String>)]) -> Vec<String> {
    if kind.is_empty() {
        return Vec::new();
    }
    if let Some((_, argv)) = overrides.iter().find(|(k, _)| k == kind) {
        return argv.clone();
    }
    SKIP_PERMISSION_FLAGS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, argv)| argv.iter().map(|a| a.to_string()).collect())
        .unwrap_or_default()
}

pub fn can_skip_permissions(kind: &str, overrides: &[(String, Vec<String>)]) -> bool {
    !skip_permission_flags(kind, overrides).is_empty()
}

/// Where agent installers put their executables, under the home folder.
///
/// Looked in after PATH, because the PATH the picker gets is Herdr's server's,
/// not your shell's. A server started over ssh, by `herdr --remote` or as a
/// service never read the startup files that add `~/.local/bin` or
/// `~/.npm-global/bin`, so an agent the new tab's shell starts fine would
/// otherwise get no chip. Herdr starts the agent in that shell, not here, so
/// finding it in one of these is enough.
pub const HOME_BIN_DIRS: &[&str] = &[
    ".local/bin",
    ".claude/local",
    ".npm-global/bin",
    ".npm/bin",
    ".bun/bin",
    ".volta/bin",
    ".deno/bin",
    ".cargo/bin",
    ".opencode/bin",
    ".yarn/bin",
    ".local/share/pnpm",
    "Library/pnpm",
    ".local/share/mise/shims",
    ".asdf/shims",
    "go/bin",
    "bin",
];

/// nvm gives each Node version its own `bin`, where that version's `npm i -g`
/// lands, so every one of them is looked in.
const NVM_VERSIONS: &str = ".nvm/versions/node";

/// HOME_BIN_DIRS and nvm's per-version folders, for this home. Empty without
/// a HOME.
pub fn home_bin_dirs() -> Vec<String> {
    let home = match std::env::var("HOME") {
        Ok(home) if !home.is_empty() => home,
        _ => return Vec::new(),
    };
    let mut dirs: Vec<String> = HOME_BIN_DIRS.iter().map(|dir| pyos::join(&home, dir)).collect();
    if let Ok(entries) = std::fs::read_dir(pyos::join(&home, NVM_VERSIONS)) {
        let mut versions: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        versions.sort();
        for version in versions {
            dirs.push(pyos::join(&pyos::join(&home, NVM_VERSIONS), &format!("{}/bin", version)));
        }
    }
    dirs
}

fn binaries_of(kind: &str) -> Vec<&str> {
    match KNOWN.iter().find(|(k, _)| *k == kind) {
        Some((_, binaries)) => binaries.to_vec(),
        None => vec![kind],
    }
}

/// Where this kind's executable is: on PATH, or else in one of `fallback`.
pub fn locate(kind: &str, fallback: &[String]) -> Option<String> {
    let binaries = binaries_of(kind);
    binaries.iter().find_map(|binary| pyos::which(binary)).or_else(|| {
        fallback.iter().find_map(|dir| {
            binaries.iter().map(|binary| pyos::join(dir, binary)).find(|candidate| pyos::is_executable_file(candidate))
        })
    })
}

/// Whether this kind's executable is on PATH itself, not only in a folder
/// `home_bin_dirs` adds.
pub fn on_path(kind: &str) -> bool {
    binaries_of(kind).iter().any(|binary| pyos::which(binary).is_some())
}

fn ordered(mut kinds: Vec<&'static str>) -> Vec<&'static str> {
    let rank = |kind: &str| PREFERRED.iter().position(|k| *k == kind).unwrap_or(PREFERRED.len());
    kinds.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.cmp(b)));
    kinds
}

/// The chip row: a plain shell first, then every agent that can be started.
///
/// A configured list is taken as written -- names Herdr does not know are
/// dropped, but an installed check is not applied, so an agent behind a shim
/// or a shell function still gets a chip.
pub fn available(configured: &Agents) -> Vec<Agent> {
    let mut chips = vec![Agent::new(NONE, NONE_LABEL)];
    let kinds: Vec<String> = match configured {
        Agents::Listed(listed) => listed.iter().filter(|kind| is_known(kind)).cloned().collect(),
        Agents::Auto => {
            // One PATH walk per kind, and they do not depend on each other.
            let fallback = home_bin_dirs();
            let fallback = &fallback;
            let found: Vec<&'static str> = std::thread::scope(|scope| {
                let handles: Vec<_> = KNOWN
                    .iter()
                    .map(|(kind, _)| {
                        scope.spawn(move || if locate(kind, fallback).is_some() { Some(*kind) } else { None })
                    })
                    .collect();
                handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect()
            });
            ordered(found).into_iter().map(|k| k.to_string()).collect()
        }
    };
    for kind in kinds {
        chips.push(Agent::new(&kind, &kind));
    }
    chips
}

pub fn index_of(chips: &[Agent], kind: Option<&str>) -> usize {
    let Some(kind) = kind.filter(|k| !k.is_empty()) else { return 0 };
    chips.iter().position(|chip| chip.kind == kind).unwrap_or(0)
}

/// Which chip starts selected.
///
/// `last` reuses whatever was started in that folder before, which is the
/// setting that earns its keep: the same project almost always wants the same
/// agent, and the first keystroke is then never about the agent row.
pub fn initial_index(chips: &[Agent], default_agent: &str, remembered: Option<&str>) -> usize {
    if default_agent == "last" {
        return index_of(chips, remembered);
    }
    if matches!(default_agent, "none" | "" | "shell") {
        return 0;
    }
    index_of(chips, Some(default_agent))
}

/// Herdr injects its own plugin variables into this process; passing them on
/// to a background child would make that child look like a plugin invocation
/// it is not. Everything else is inherited untouched.
pub const PLUGIN_VARIABLES: &[&str] = &[
    "HERDR_PLUGIN_ID",
    "HERDR_PLUGIN_ROOT",
    "HERDR_PLUGIN_CONFIG_DIR",
    "HERDR_PLUGIN_STATE_DIR",
    "HERDR_PLUGIN_CONTEXT_JSON",
    "HERDR_PLUGIN_ACTION_ID",
    "HERDR_PLUGIN_ENTRYPOINT_ID",
    "HERDR_PLUGIN_EVENT",
    "HERDR_PLUGIN_EVENT_JSON",
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
];
