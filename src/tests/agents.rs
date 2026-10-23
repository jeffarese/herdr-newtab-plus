//! Ported from the Python release's tests/test_agents.py.
#![allow(unused_imports)]

use crate::agents::{self, available, initial_index, Agent, KNOWN, PLUGIN_VARIABLES, PREFERRED};
use crate::config::Agents;
use crate::testutil::*;

fn kinds(chips: &[Agent]) -> Vec<String> {
    chips.iter().map(|chip| chip.kind.clone()).collect()
}

fn listed(kinds: &[&str]) -> Agents {
    Agents::Listed(kinds.iter().map(|k| k.to_string()).collect())
}

/// `available("auto")` on a PATH that holds only these executables. The
/// Python swapped `agents.installed` for a lambda; here the PATH is real.
fn detected(binaries: &[&str]) -> Vec<String> {
    use std::os::unix::fs::PermissionsExt;
    let _lock = env_lock();
    let bin = TempDir::new();
    for binary in binaries {
        bin.touch(binary, "#!/bin/sh\n");
        std::fs::set_permissions(bin.join(binary), std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = EnvVars::new();
    env.set("PATH", bin.path());
    kinds(&available(&Agents::Auto))
}

// -- AvailableTest ------------------------------------------------------------

#[test]
fn a_plain_shell_always_leads() {
    let chips = available(&listed(&[]));
    assert_eq!(chips.len(), 1);
    assert!(chips[0].is_none());
}

#[test]
fn a_configured_list_is_taken_as_written() {
    assert_eq!(kinds(&available(&listed(&["codex", "claude"]))), vec!["", "codex", "claude"]);
}

#[test]
fn kinds_herdr_does_not_know_are_dropped() {
    assert_eq!(kinds(&available(&listed(&["nope", "claude"]))), vec!["", "claude"]);
}

#[test]
fn detection_only_offers_what_is_installed() {
    assert_eq!(detected(&["codex", "claude"]), vec!["", "claude", "codex"]);
}

#[test]
fn preferred_kinds_lead_and_the_rest_sort() {
    assert_eq!(detected(&["maki", "claude", "agy"]), vec!["", "claude", "agy", "maki"]);
}

#[test]
fn every_preferred_kind_is_one_herdr_knows() {
    for kind in PREFERRED {
        assert!(KNOWN.iter().any(|(known, _)| known == kind), "{}", kind);
    }
}

// -- InitialIndexTest ---------------------------------------------------------

fn index_chips() -> Vec<Agent> {
    vec![Agent::new("", "shell"), Agent::new("claude", "claude"), Agent::new("codex", "codex")]
}

#[test]
fn last_uses_what_the_folder_remembers() {
    assert_eq!(initial_index(&index_chips(), "last", Some("codex")), 2);
}

#[test]
fn last_with_nothing_remembered_falls_back_to_the_shell() {
    assert_eq!(initial_index(&index_chips(), "last", None), 0);
}

#[test]
fn a_remembered_agent_that_is_gone_falls_back() {
    assert_eq!(initial_index(&index_chips(), "last", Some("gemini")), 0);
}

#[test]
fn none_pins_the_shell() {
    assert_eq!(initial_index(&index_chips(), "none", Some("codex")), 0);
}

#[test]
fn a_named_default_wins() {
    assert_eq!(initial_index(&index_chips(), "claude", Some("codex")), 1);
}

// -- SkipPermissionFlagsTest --------------------------------------------------

fn overrides(kind: &str, argv: &[&str]) -> Vec<(String, Vec<String>)> {
    vec![(kind.to_string(), argv.iter().map(|a| a.to_string()).collect())]
}

#[test]
fn a_known_agent_reports_its_own_spelling() {
    assert_eq!(agents::skip_permission_flags("claude", &[]), vec!["--dangerously-skip-permissions"]);
    assert_eq!(agents::skip_permission_flags("codex", &[]), vec!["--dangerously-bypass-approvals-and-sandbox"]);
}

#[test]
fn an_agent_with_no_known_flag_reports_nothing() {
    assert!(agents::skip_permission_flags("opencode", &[]).is_empty());
    assert!(!agents::can_skip_permissions("opencode", &[]));
}

#[test]
fn a_plain_shell_is_never_skippable() {
    assert!(agents::skip_permission_flags("", &[]).is_empty());
    assert!(!agents::can_skip_permissions("", &[]));
}

#[test]
fn config_teaches_the_toggle_an_agent_it_did_not_know() {
    let taught = overrides("gemini", &["--yolo"]);
    assert!(agents::can_skip_permissions("gemini", &taught));
    assert_eq!(agents::skip_permission_flags("gemini", &taught), vec!["--yolo"]);
}

#[test]
fn config_overrides_a_built_in_spelling() {
    assert_eq!(
        agents::skip_permission_flags("claude", &overrides("claude", &["--permission-mode", "bypass"])),
        vec!["--permission-mode", "bypass"]
    );
}

#[test]
fn an_empty_override_turns_the_toggle_off_for_that_agent() {
    assert!(!agents::can_skip_permissions("claude", &overrides("claude", &[])));
}

// -- AgentEnvTest -------------------------------------------------------------

// Python's `agent_env()` built the child's environment without the plugin
// variables. Rust's `spawn_agent` removes `PLUGIN_VARIABLES` from the child's
// inherited environment instead, so the list is what is tested.
#[test]
fn plugin_variables_do_not_leak_into_the_child() {
    assert!(PLUGIN_VARIABLES.contains(&"HERDR_PLUGIN_ID"));
    assert!(PLUGIN_VARIABLES.contains(&"HERDR_PANE_ID"));
    // The socket is how the child reaches Herdr, so it has to survive.
    assert!(!PLUGIN_VARIABLES.contains(&"HERDR_SOCKET_PATH"));
}
