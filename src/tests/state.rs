//! Ported from tests/test_state.py.
#![allow(unused_imports)]

use crate::state::{Recents, MAX_ENTRIES};
use crate::testutil::*;
use std::collections::HashMap;
use std::path::Path;

/// `tempfile.mkstemp` then unlink: a path to a state file that does not exist
/// yet. The directory, and so the file, goes when this does.
struct StateFile {
    _dir: TempDir,
    path: String,
}

impl StateFile {
    fn new() -> StateFile {
        let dir = TempDir::new();
        let path = dir.join("newtab-recent.json");
        StateFile { _dir: dir, path }
    }

    fn some(&self) -> Option<String> {
        Some(self.path.clone())
    }

    fn write(&self, text: &str) {
        std::fs::write(&self.path, text).unwrap();
    }
}

// -- RecentsTest -------------------------------------------------------------

#[test]
fn most_recent_leads() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "claude");
    recents.touch("/b", "codex");
    assert_eq!(recents.folders(), vec!["/b", "/a"]);
}

#[test]
fn touching_again_moves_it_up_without_duplicating() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "");
    recents.touch("/b", "");
    recents.touch("/a", "");
    assert_eq!(recents.folders(), vec!["/a", "/b"]);
}

#[test]
fn the_agent_is_remembered_per_folder() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "claude");
    recents.touch("/b", "codex");
    assert_eq!(recents.agent_for("/a").as_deref(), Some("claude"));
    assert_eq!(recents.agent_for("/b").as_deref(), Some("codex"));
    assert_eq!(recents.agent_for("/c"), None);
}

#[test]
fn a_shell_pick_is_remembered_as_no_agent() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "");
    assert_eq!(recents.agent_for("/a"), None);
}

#[test]
fn agents_maps_each_folder_to_its_latest_agent() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "codex");
    recents.touch("/b", "");
    recents.touch("/a", "claude");
    assert_eq!(recents.agents(), HashMap::from([("/a".to_string(), "claude".to_string())]));
}

#[test]
fn last_agent_skips_folders_opened_as_a_shell() {
    let mut recents = Recents::new(None);
    recents.touch("/a", "claude");
    recents.touch("/b", "");
    assert_eq!(recents.last_agent().as_deref(), Some("claude"));
}

#[test]
fn paths_are_normalized_on_the_way_in() {
    let mut recents = Recents::new(None);
    recents.touch("/a/b/", "");
    assert_eq!(recents.agent_for("/a/b"), None);
    assert_eq!(recents.folders(), vec!["/a/b"]);
}

#[test]
fn the_list_is_capped() {
    let mut recents = Recents::new(None);
    for index in 0..MAX_ENTRIES + 20 {
        recents.touch(&format!("/f{}", index), "");
    }
    assert_eq!(recents.entries.len(), MAX_ENTRIES);
}

#[test]
fn round_trips_through_a_file() {
    let file = StateFile::new();
    let mut recents = Recents::new(file.some());
    recents.touch("/a", "claude");
    recents.save();
    assert_eq!(Recents::new(file.some()).agent_for("/a").as_deref(), Some("claude"));
}

#[test]
fn a_broken_file_is_ignored_rather_than_fatal() {
    let file = StateFile::new();
    file.write("{not json");
    assert!(Recents::new(file.some()).folders().is_empty());
}

#[test]
fn junk_entries_are_dropped() {
    let file = StateFile::new();
    file.write(r#"{"entries": [{"path": "/a"}, {"nope": 1}, "string"]}"#);
    assert_eq!(Recents::new(file.some()).folders(), vec!["/a"]);
}

#[test]
fn saving_without_a_path_is_a_no_op() {
    Recents::new(None).save();
}

// -- CreateModeTest ----------------------------------------------------------

#[test]
fn nothing_is_remembered_until_the_toggle_moves() {
    assert_eq!(Recents::new(None).create_mode(), None);
}

#[test]
fn it_round_trips_through_a_file() {
    let file = StateFile::new();
    let mut recents = Recents::new(file.some());
    recents.remember_create_mode("space");
    recents.save();
    assert_eq!(Recents::new(file.some()).create_mode(), Some("space"));
}

#[test]
fn an_unknown_mode_is_ignored_rather_than_trusted() {
    let mut recents = Recents::new(None);
    recents.remember_create_mode("sideways");
    assert_eq!(recents.create_mode(), None);
}

#[test]
fn a_junk_mode_in_the_file_falls_back_to_nothing_remembered() {
    let file = StateFile::new();
    file.write(r#"{"create": 7, "entries": [{"path": "/a"}]}"#);
    let recents = Recents::new(file.some());
    assert_eq!(recents.create_mode(), None);
    assert_eq!(recents.folders(), vec!["/a"]); // the rest of the file still counts
}

#[test]
fn an_unchanged_state_is_not_written_back() {
    let file = StateFile::new();
    Recents::new(file.some()).save();
    assert!(!Path::new(&file.path).exists());
}

#[test]
fn the_mode_saves_on_its_own_without_a_folder_being_picked() {
    let file = StateFile::new();
    let mut recents = Recents::new(file.some());
    recents.remember_create_mode("tab");
    recents.save();
    assert_eq!(Recents::new(file.some()).create_mode(), Some("tab"));
    assert!(Recents::new(file.some()).folders().is_empty());
}
