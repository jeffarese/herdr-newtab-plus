//! Ported from tests/test_session.py.
#![allow(unused_imports)]

use crate::paths;
use crate::pyjson::Value;
use crate::pyos;
use crate::session::{
    agent_name, clear_resolved, created_ids, focused_cwd, folder_statuses, live_agent_names, next_mode, normalize,
    resolve_target, resolved, space_folders, spaces, RESOLVED, SMART, SPACE, TAB,
};
use crate::testutil::*;
use std::collections::HashSet;
use std::os::unix::fs::symlink;

fn keys(map: &crate::session::SpaceMap) -> Vec<String> {
    map.iter().map(|(key, _)| key.clone()).collect()
}

fn taken(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

// -- NormalizeTest -----------------------------------------------------------

#[test]
fn expands_and_strips() {
    let home = pyos::expanduser("~");
    assert_eq!(normalize("~/x/"), format!("{}/x", home));
    assert_eq!(normalize("  ~  "), home);
    assert_eq!(normalize("/"), "/");
}

#[test]
fn empty_stays_empty() {
    assert_eq!(normalize(""), "");
    assert_eq!(normalize("   "), "");
}

// -- SpacesTest --------------------------------------------------------------

#[test]
fn folders_come_from_panes() {
    let snap = snapshot(vec![pane("w1:p1", "/a", "w1"), pane("w1:p2", "/b", "w1")], None);
    let mut found = keys(&space_folders(&snap));
    found.sort();
    assert_eq!(found, vec!["/a", "/b"]);
    assert_eq!(space_folders(&snap).get("/a").unwrap().label, "Atlas");
}

#[test]
fn focused_space_wins_a_shared_folder() {
    let snap = snapshot(
        vec![pane("w1:p1", "/a", "w1"), pane("w2:p1", "/a", "w2")],
        Some(vec![space("w1", "one", 1, false), space("w2", "two", 2, true)]),
    );
    assert_eq!(space_folders(&snap).get("/a").unwrap().label, "two");
}

#[test]
fn trailing_separator_does_not_split_a_folder() {
    let snap = snapshot(vec![pane("w1:p1", "/a/", "w1"), pane("w1:p2", "/a", "w1")], None);
    assert_eq!(keys(&space_folders(&snap)), vec!["/a"]);
}

#[test]
fn panes_without_a_known_space_are_skipped() {
    let snap = snapshot(vec![pane("w9:p1", "/a", "w9")], None);
    assert!(space_folders(&snap).is_empty());
}

#[test]
fn focused_cwd_prefers_the_focused_pane() {
    let mut snap = snapshot(vec![pane("w1:p1", "/a", "w1"), pane("w1:p2", "/b", "w1")], None);
    set(&mut snap, "focused_pane_id", Value::str("w1:p2"));
    assert_eq!(focused_cwd(&snap).as_deref(), Some("/b"));
}

#[test]
fn spaces_survive_a_missing_label() {
    let snap = snapshot(vec![], Some(vec![json(r#"{"workspace_id": "w1", "number": 1, "focused": true}"#)]));
    assert_eq!(spaces(&snap)[0].label, "w1");
}

// -- ResolveTargetTest -------------------------------------------------------

fn open_snap() -> Value {
    snapshot(vec![pane("w1:p1", "/open", "w1")], None)
}

#[test]
fn smart_reuses_a_space_standing_in_the_folder() {
    let target = resolve_target("/open", &open_snap(), "smart");
    assert_eq!(target.kind, TAB);
    assert_eq!(target.workspace_id.as_deref(), Some("w1"));
    assert_eq!(target.workspace_label.as_deref(), Some("Atlas"));
}

#[test]
fn smart_creates_a_space_for_anything_else() {
    let target = resolve_target("/elsewhere", &open_snap(), "smart");
    assert_eq!(target.kind, SPACE);
    assert_eq!(target.workspace_id, None);
    assert_eq!(target.workspace_label.as_deref(), Some("elsewhere"));
}

#[test]
fn tab_mode_falls_back_to_the_focused_space() {
    let target = resolve_target("/elsewhere", &open_snap(), "tab");
    assert_eq!(target.kind, TAB);
    assert_eq!(target.workspace_id.as_deref(), Some("w1"));
}

#[test]
fn tab_mode_creates_a_space_when_there_are_none() {
    let mut empty = snapshot(vec![], Some(vec![]));
    set(&mut empty, "focused_workspace_id", Value::Null);
    assert_eq!(resolve_target("/x", &empty, "tab").kind, SPACE);
}

#[test]
fn space_mode_never_reuses() {
    assert_eq!(resolve_target("/open", &open_snap(), "space").kind, SPACE);
}

#[test]
fn space_mode_still_names_the_space_it_would_make() {
    // The line above the hints reads off this label, so an insisted-on space
    // has to carry one just as an inferred one does.
    assert_eq!(resolve_target("/open", &open_snap(), "space").workspace_label.as_deref(), Some("open"));
}

#[test]
fn an_unknown_mode_behaves_as_smart_does() {
    assert_eq!(resolve_target("/open", &open_snap(), "sideways").kind, TAB);
}

#[test]
fn folder_is_normalized_on_the_way_in() {
    assert_eq!(resolve_target("/open/", &open_snap(), "smart").kind, TAB);
}

#[test]
fn a_symlinked_folder_finds_the_space_it_already_has() {
    // A shell resolves its own cwd, so the pane reports the real path while
    // the picker offers the link. Both have to land on the same space, or
    // picking the folder twice silently opens two.
    let tree = TempDir::with(&["real"]);
    let link = tree.join("link");
    symlink(tree.join("real"), &link).unwrap();
    let snap = snapshot(vec![pane("w1:p1", &tree.join("real"), "w1")], None);
    assert_eq!(resolve_target(&link, &snap, "smart").kind, TAB);

    // ...and the other way round, when the pane is the one on the link.
    let snap = snapshot(vec![pane("w1:p1", &link, "w1")], None);
    assert_eq!(resolve_target(&tree.join("real"), &snap, "smart").kind, TAB);
}

#[test]
fn the_picked_spelling_is_what_gets_created() {
    let tree = TempDir::with(&["real"]);
    let link = tree.join("link");
    symlink(tree.join("real"), &link).unwrap();
    let target = resolve_target(&link, &snapshot(vec![], None), "smart");
    assert_eq!(target.folder, link);
}

// -- NextModeTest ------------------------------------------------------------

#[test]
fn it_cycles_decide_for_me_then_each_override() {
    assert_eq!(next_mode(SMART), TAB);
    assert_eq!(next_mode(TAB), SPACE);
    assert_eq!(next_mode(SPACE), SMART);
}

#[test]
fn an_unrecognised_mode_lands_where_smart_would() {
    assert_eq!(next_mode("sideways"), TAB);
}

// -- AgentNameTest -----------------------------------------------------------

#[test]
fn derives_from_the_folder() {
    assert_eq!(agent_name("/a/my-project", "claude", &[]), "my-project");
}

#[test]
fn illegal_characters_become_hyphens() {
    assert_eq!(agent_name("/a/My Project!", "claude", &[]), "my-project");
}

#[test]
fn a_leading_non_letter_is_repaired() {
    let name = agent_name("/a/2024-notes", "claude", &[]);
    assert!(name.chars().next().unwrap().is_alphabetic(), "{}", name);
}

#[test]
fn avoids_taken_names() {
    assert_eq!(agent_name("/a/api", "claude", &taken(&["api"])), "api-2");
    assert_eq!(agent_name("/a/api", "claude", &taken(&["api", "api-2"])), "api-3");
}

#[test]
fn stays_within_herdrs_limit() {
    let name = agent_name(&format!("/a/{}", "x".repeat(80)), "claude", &[("x".repeat(32))]);
    assert!(name.chars().count() <= 32);
}

#[test]
fn a_nameless_folder_still_produces_something_legal() {
    assert!(!agent_name("/", "codex", &[]).is_empty());
}

// -- SnapshotFieldsTest ------------------------------------------------------

#[test]
fn live_agent_names_reads_only_named_agents() {
    let mut snap = snapshot(vec![], None);
    set(&mut snap, "agents", json(r#"[{"agent": "claude"}, {"agent": "codex", "name": "api"}]"#));
    assert_eq!(live_agent_names(&snap), HashSet::from(["api".to_string()]));
}

#[test]
fn created_ids_reads_both_shapes() {
    let result = json(r#"{"tab": {"tab_id": "w1:t1"}, "root_pane": {"pane_id": "w1:p1"}}"#);
    assert_eq!(created_ids(&result), (Some("w1:t1".to_string()), Some("w1:p1".to_string())));
    assert_eq!(created_ids(&json("{}")), (None, None));
}

// -- FolderStatusTest --------------------------------------------------------

fn status_snap(agents: &str, panes: Option<Vec<Value>>) -> Value {
    let panes = panes.unwrap_or_else(|| vec![pane("w1:p1", "/work/api", "w1"), pane("w1:p2", "/work/web", "w1")]);
    let mut snap = snapshot(panes, None);
    set(&mut snap, "agents", json(agents));
    snap
}

#[test]
fn an_agent_marks_the_folder_its_pane_is_standing_in() {
    let found = folder_statuses(&status_snap(r#"[{"pane_id": "w1:p1", "agent_status": "working"}]"#, None));
    assert_eq!(found.get("/work/api").map(String::as_str), Some("working"));
    assert!(!found.contains_key("/work/web"));
}

#[test]
fn the_most_urgent_status_wins() {
    let found = folder_statuses(&status_snap(
        r#"[
            {"pane_id": "w1:p1", "agent_status": "idle"},
            {"pane_id": "w1:p2", "agent_status": "working", "cwd": "/work/api"},
            {"pane_id": "w1:p1", "agent_status": "blocked"}
        ]"#,
        None,
    ));
    assert_eq!(found.get("/work/api").map(String::as_str), Some("blocked"));
}

#[test]
fn an_agents_own_cwd_beats_its_panes() {
    let found =
        folder_statuses(&status_snap(r#"[{"pane_id": "w1:p1", "agent_status": "done", "cwd": "/work/web/"}]"#, None));
    assert_eq!(found.get("/work/web").map(String::as_str), Some("done"));
    assert!(!found.contains_key("/work/api"));
}

#[test]
fn a_status_herdr_does_not_report_is_left_out() {
    let found = folder_statuses(&status_snap(
        r#"[
            {"pane_id": "w1:p1", "agent_status": "none"},
            {"pane_id": "w1:p2"},
            {"pane_id": "w1:p9", "agent_status": "working"},
            "junk"
        ]"#,
        None,
    ));
    // (w1:p9 is no such pane.)
    assert!(found.is_empty(), "{:?}", found);
}

#[test]
fn a_pane_that_reports_an_agent_status_counts_too() {
    let mut blocked = pane("w1:p1", "/work/api", "w1");
    set(&mut blocked, "agent_status", Value::str("blocked"));
    let found = folder_statuses(&status_snap("[]", Some(vec![blocked])));
    assert_eq!(found.get("/work/api").map(String::as_str), Some("blocked"));
}

// -- ResolvedTest ------------------------------------------------------------
// Following a symlink is remembered, and the rescan key is how you forget.
//
// Python counted calls to a patched os.path.realpath. Rust cannot patch it, so
// these count the entries in the resolved cache instead: every miss follows
// the path once and stores exactly one entry, so the two counts are the same.

struct Resolved {
    tree: TempDir,
    link: String,
}

impl Resolved {
    fn set_up() -> Resolved {
        let tree = TempDir::with(&["real"]);
        let link = tree.join("link");
        symlink(tree.join("real"), &link).unwrap();
        paths::clear_cache();
        Resolved { tree, link }
    }
}

impl Drop for Resolved {
    fn drop(&mut self) {
        paths::clear_cache();
    }
}

/// How many paths `action` followed afresh.
fn realpath_calls(action: impl FnOnce()) -> usize {
    let before = RESOLVED.with(|cache| cache.borrow().len());
    action();
    RESOLVED.with(|cache| cache.borrow().len()) - before
}

#[test]
fn follows_a_symlink() {
    let t = Resolved::set_up();
    assert_eq!(resolved(&t.link), t.tree.join("real"));
}

#[test]
fn the_same_folder_is_only_followed_once() {
    let t = Resolved::set_up();
    let seen = realpath_calls(|| {
        for _ in 0..5 {
            resolved(&t.link);
        }
    });
    assert_eq!(seen, 1);
}

#[test]
fn a_remembered_answer_is_the_same_answer() {
    let t = Resolved::set_up();
    let first = resolved(&t.link);
    assert_eq!(resolved(&t.link), first);
}

#[test]
fn different_folders_are_followed_separately() {
    let t = Resolved::set_up();
    let seen = realpath_calls(|| {
        resolved(&t.link);
        resolved(&t.tree.join("real"));
    });
    assert_eq!(seen, 2);
}

#[test]
fn the_rescan_key_makes_it_ask_again() {
    // ctrl+r goes through paths.clear_cache, so that has to reach this too
    // or a repointed symlink would stay wrong for the life of the popup.
    let t = Resolved::set_up();
    resolved(&t.link);
    paths::clear_cache();
    let seen = realpath_calls(|| {
        resolved(&t.link);
    });
    assert_eq!(seen, 1);
}

#[test]
fn clearing_picks_up_a_symlink_that_moved() {
    let t = Resolved::set_up();
    std::fs::create_dir_all(t.tree.join("other")).unwrap();
    assert_eq!(resolved(&t.link), t.tree.join("real"));
    std::fs::remove_file(&t.link).unwrap();
    symlink(t.tree.join("other"), &t.link).unwrap();
    clear_resolved();
    assert_eq!(resolved(&t.link), t.tree.join("other"));
}

#[test]
fn an_empty_path_is_never_cached_as_an_answer() {
    let _t = Resolved::set_up();
    assert_eq!(resolved(""), "");
    assert_eq!(resolved("   "), "");
}
