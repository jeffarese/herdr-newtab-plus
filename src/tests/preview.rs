//! Ported from tests/test_preview.py.
//! The folder preview: what it reads, how it is laid out, and how it is toggled.
#![allow(unused_imports)]

use crate::app::Picker;
use crate::config::Config;
use crate::keys::MouseKind;
use crate::paths::{self, Contents, Row, SOURCE_ROOT};
use crate::pyjson::Value;
use crate::render::{self, Preview};
use crate::session::{folder_agents, FolderAgent};
use crate::testutil::*;
use crate::textutil::display_width;
use crate::theme::Theme;
use std::collections::HashMap;
use std::rc::Rc;

// What the manifest's 110x30 popup gives the picker.
const WIDE: (isize, isize) = (107, 28);

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| name.to_string()).collect()
}

fn agent(kind: &str, name: &str, status: &str) -> FolderAgent {
    FolderAgent { kind: kind.to_string(), name: name.to_string(), status: status.to_string() }
}

/// The character at a column, counted in characters as Python indexes a str.
fn at(line: &str, column: usize) -> char {
    line.chars().nth(column).unwrap_or_else(|| panic!("no column {column} in {line:?}"))
}

/// Clears the filesystem caches on the way in and on the way out.
struct Fresh;

impl Fresh {
    fn new() -> Self {
        paths::clear_cache();
        Fresh
    }
}

impl Drop for Fresh {
    fn drop(&mut self) {
        paths::clear_cache();
    }
}

// -- ContentsTest ------------------------------------------------------------

fn contents_tree() -> (Fresh, TempDir) {
    let fresh = Fresh::new();
    let tree = TempDir::with(&["app/src", "app/Tests", "app/.github"]);
    for name in ["README.md", "build.sh", ".env"] {
        tree.touch(&format!("app/{name}"), "");
    }
    (fresh, tree)
}

#[test]
fn directories_come_first_and_each_half_is_sorted_without_case() {
    let (_fresh, tree) = contents_tree();
    let listed = paths::contents(&tree.join("app")).unwrap();
    assert_eq!(listed.dirs, strings(&[".github", "src", "Tests"]));
    assert_eq!(listed.files, strings(&[".env", "build.sh", "README.md"]));
    assert!(listed.readable);
}

#[test]
fn a_missing_folder_has_no_contents() {
    let (_fresh, tree) = contents_tree();
    assert!(paths::contents(&tree.join("nope")).is_none());
    assert!(paths::contents(&tree.join("app/README.md")).is_none());
}

#[test]
fn a_folder_read_once_answers_whether_it_exists() {
    let (_fresh, tree) = contents_tree();
    let (folder, file) = (tree.join("app"), tree.join("app/README.md"));
    paths::contents(&folder);
    paths::contents(&file);
    // Python made `os.path.isdir` raise. Here the disk is made to say the
    // opposite instead: the answers can only come from what was read.
    std::fs::remove_dir_all(&folder).unwrap();
    std::fs::create_dir_all(&file).unwrap();
    assert!(paths::is_directory(&folder, false));
    assert!(!paths::is_directory(&file, false));
}

#[test]
fn fresh_asks_the_filesystem_whatever_was_read_before() {
    let (_fresh, tree) = contents_tree();
    let folder = tree.join("app/src");
    paths::contents(&folder);
    std::fs::remove_dir(&folder).unwrap();
    assert!(paths::is_directory(&folder, false));
    assert!(!paths::is_directory(&folder, true));
}

#[test]
fn the_rescan_key_forgets_what_was_read() {
    let (_fresh, tree) = contents_tree();
    let folder = tree.join("app");
    paths::contents(&folder);
    tree.touch("app/new.txt", "");
    assert!(!paths::contents(&folder).unwrap().files.contains(&"new.txt".to_string()));
    paths::clear_cache();
    assert!(paths::contents(&folder).unwrap().files.contains(&"new.txt".to_string()));
}

#[test]
fn a_folder_that_is_not_ours_to_read_says_so() {
    if unsafe { libc::geteuid() } == 0 {
        return; // root reads everything
    }
    use std::os::unix::fs::PermissionsExt;
    let (_fresh, tree) = contents_tree();
    let folder = tree.join("app/src");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    let listed = paths::contents(&folder);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    let listed = listed.expect("a folder, even an unreadable one");
    assert!(!listed.readable);
}

// -- BranchTest --------------------------------------------------------------

fn branch_tree() -> (Fresh, TempDir) {
    let fresh = Fresh::new();
    (fresh, TempDir::with(&["repo/.git", "plain", "worktree", "gitdirs/wt"]))
}

#[test]
fn a_checkout_names_its_branch() {
    let (_fresh, tree) = branch_tree();
    tree.touch("repo/.git/HEAD", "ref: refs/heads/feature/login\n");
    assert_eq!(paths::git_branch(&tree.join("repo"), None).as_deref(), Some("feature/login"));
}

#[test]
fn a_detached_head_is_a_short_hash() {
    let (_fresh, tree) = branch_tree();
    tree.touch("repo/.git/HEAD", "0123456789abcdef0123456789abcdef01234567\n");
    assert_eq!(paths::git_branch(&tree.join("repo"), None).as_deref(), Some("0123456"));
}

#[test]
fn a_worktree_follows_its_pointer() {
    let (_fresh, tree) = branch_tree();
    tree.touch("gitdirs/wt/HEAD", "ref: refs/heads/wip\n");
    tree.touch("worktree/.git", "gitdir: ../gitdirs/wt\n");
    assert_eq!(paths::git_branch(&tree.join("worktree"), None).as_deref(), Some("wip"));
}

#[test]
fn a_folder_that_is_not_a_checkout_has_none_and_reads_nothing() {
    let (_fresh, tree) = branch_tree();
    let folder = tree.join("plain");
    let listed = paths::contents(&folder);
    // Python made `open` raise. Here a checkout appears after the listing was
    // taken: the branch comes from the listing alone, so it is not read.
    std::fs::create_dir_all(tree.join("plain/.git")).unwrap();
    tree.touch("plain/.git/HEAD", "ref: refs/heads/main\n");
    assert_eq!(paths::git_branch(&folder, listed.as_deref()), None);
    assert_eq!(paths::git_branch(&tree.join("missing"), None), None);
}

// -- FolderAgentsTest --------------------------------------------------------

#[test]
fn every_agent_in_a_folder_most_urgent_first() {
    let mut snap = snapshot(vec![pane("w1:p1", "/work/api", "w1"), pane("w1:p2", "/work/api", "w1")], None);
    set(
        &mut snap,
        "agents",
        json(
            r#"[
                {"pane_id": "w1:p1", "agent": "claude", "agent_status": "idle"},
                {"pane_id": "w1:p2", "agent": "codex", "name": "reviewer", "agent_status": "blocked"}
            ]"#,
        ),
    );
    let found = folder_agents(&snap).remove("/work/api").unwrap();
    assert_eq!(found, vec![agent("codex", "reviewer", "blocked"), agent("claude", "claude", "idle")]);
}

#[test]
fn an_agent_without_a_status_is_still_listed() {
    let mut snap = snapshot(vec![pane("w1:p1", "/work/api", "w1")], None);
    set(&mut snap, "agents", json(r#"[{"pane_id": "w1:p1", "agent": "pi"}]"#));
    assert_eq!(folder_agents(&snap).remove("/work/api").unwrap(), vec![agent("pi", "pi", "")]);
}

#[test]
fn a_folder_with_nobody_in_it_is_absent() {
    let snap = snapshot(vec![pane("w1:p1", "/work/api", "w1")], None);
    assert_eq!(folder_agents(&snap), HashMap::new());
}

// -- LayoutTest --------------------------------------------------------------

#[test]
fn off_unless_asked_for() {
    let layout = render::compute_layout(WIDE.0, WIDE.1, false);
    assert_eq!(layout.preview_width, 0);
    assert_eq!(layout.list_width, layout.inner_width);
}

#[test]
fn a_third_of_the_popup_beside_the_list() {
    let layout = render::compute_layout(WIDE.0, WIDE.1, true);
    assert_eq!(layout.preview_width, 105 / 3);
    assert_eq!(
        layout.list_width + render::SEPARATOR.chars().count() as isize + layout.preview_width,
        layout.inner_width
    );
    assert!(layout.list_width >= render::MIN_LIST);
}

#[test]
fn the_shipped_popup_shows_it_by_itself() {
    assert!(WIDE.0 - 2 >= render::PREVIEW_AUTO);
}

#[test]
fn a_popup_too_narrow_or_too_short_keeps_the_list_whole() {
    let narrow = render::compute_layout(render::PREVIEW_FITS + 1, 28, true);
    assert_eq!(narrow.preview_width, 0);
    let short = render::compute_layout(107, 5, true);
    assert_eq!(short.preview_width, 0);
}

#[test]
fn it_never_changes_the_rows() {
    for height in [6, 12, 28, 40] {
        let mut off = render::compute_layout(107, height, false);
        let mut on = render::compute_layout(107, height, true);
        for layout in [&mut off, &mut on] {
            layout.list_width = 0;
            layout.preview_width = 0;
        }
        assert_eq!(off, on, "height {height}");
    }
}

// -- RenderPreviewTest -------------------------------------------------------

fn base_preview() -> Preview {
    Preview {
        name: "api".to_string(),
        path: "~/work/api".to_string(),
        contents: Some(Rc::new(Contents {
            dirs: strings(&["src", "tests", ".github"]),
            files: strings(&["README.md", ".env"]),
            readable: true,
        })),
        branch: None,
        space: None,
        agents: Vec::new(),
        hidden: false,
    }
}

fn listing(dirs: Vec<String>, files: Vec<String>, readable: bool) -> Option<Rc<Contents>> {
    Some(Rc::new(Contents { dirs, files, readable }))
}

fn lines_with(preview: Option<&Preview>, width: isize, height: isize, theme: Option<&Theme>) -> Vec<String> {
    let default = Theme::new(None, None, None);
    let rows = render::render_preview(theme.unwrap_or(&default), width, height, preview);
    assert_eq!(rows.len(), height as usize);
    for row in &rows {
        assert!(display_width(&plain_text(row)) as isize <= width, "{row:?}");
    }
    rows.iter().map(|row| plain_text(row).trim_end().to_string()).collect()
}

fn lines(preview: &Preview) -> Vec<String> {
    lines_with(Some(preview), 35, 20, None)
}

#[test]
fn heading_path_rule_then_what_is_inside() {
    let rows = lines(&base_preview());
    assert_eq!(rows[0], "api");
    assert_eq!(rows[1], "~/work/api");
    assert!(rows[2].starts_with("───"));
    assert_eq!(rows[3..6], strings(&["src/", "tests/", "README.md"]));
}

#[test]
fn dot_files_only_when_hidden_ones_are_offered() {
    assert!(!lines(&base_preview()).contains(&".env".to_string()));
    let rows = lines(&Preview { hidden: true, ..base_preview() });
    assert!(rows.contains(&".github/".to_string()));
    assert!(rows.contains(&".env".to_string()));
}

#[test]
fn the_facts_come_before_the_listing() {
    let rows = lines(&Preview {
        branch: Some("main".to_string()),
        space: Some("Atlas".to_string()),
        agents: vec![agent("claude", "claude", "working"), agent("codex", "api", "blocked")],
        ..base_preview()
    });
    assert_eq!(rows[3], "branch  main");
    assert_eq!(rows[4], "space   Atlas");
    assert_eq!(rows[5], "agents  ◐ claude  working");
    assert_eq!(rows[6], "        ◉ api  needs you");
    assert_eq!(rows[7], "");
    assert_eq!(rows[8], "src/");
}

#[test]
fn what_does_not_fit_is_counted() {
    let many = listing((0..30).map(|i| format!("d{i:02}")).collect(), Vec::new(), true);
    let rows = lines_with(Some(&Preview { contents: many, ..base_preview() }), 35, 10, None);
    assert_eq!(rows[3..9], (0..6).map(|i| format!("d{i:02}/")).collect::<Vec<_>>()[..]);
    assert_eq!(rows[9], "… 24 more");
}

#[test]
fn empty_missing_and_unreadable_say_which() {
    let empty = Preview { contents: listing(Vec::new(), Vec::new(), true), ..base_preview() };
    assert_eq!(lines(&empty)[3], "empty");
    assert_eq!(lines(&Preview { contents: None, ..base_preview() })[3], "not a folder yet");
    let unreadable = Preview { contents: listing(Vec::new(), Vec::new(), false), ..base_preview() };
    assert_eq!(lines(&unreadable)[3], "not readable");
}

#[test]
fn nothing_to_preview_is_blank() {
    assert_eq!(lines_with(None, 35, 20, None), vec![String::new(); 20]);
}

#[test]
fn an_agent_logo_leads_its_name_with_the_font() {
    let mut theme = Theme::new(None, None, None);
    theme.agent_icons = true;
    let preview = Preview { agents: vec![agent("claude", "claude", "idle")], ..base_preview() };
    let rows = lines_with(Some(&preview), 35, 20, Some(&theme));
    assert!(rows[3].contains(&format!("{}  claude", theme.logo(Some("claude")))), "{:?}", rows[3]);
}

// -- RuleJunctionTest --------------------------------------------------------

#[test]
fn a_junction_lands_on_its_column() {
    let theme = Theme::new(None, None, None);
    let line = plain_text(&render::render_rule(&theme, 10, 4, "┬"));
    assert_eq!(line, "────┬─────");
    // Python's `render_rule(theme, 10)`: no junction.
    assert_eq!(plain_text(&render::render_rule(&theme, 10, -1, "┼")), "─".repeat(10));
}

// -- PickerPreviewTest -------------------------------------------------------

fn picker_tree() -> (Fresh, TempDir, Vec<Row>) {
    let fresh = Fresh::new();
    let tree = TempDir::with(&["api/src", "api/tests", "web"]);
    tree.touch("api/README.md", "");
    let items = ["api", "web"].iter().map(|name| suggestion(&tree.join(name), name, SOURCE_ROOT)).collect();
    (fresh, tree, items)
}

fn frame_at(app: &mut Picker, size: (isize, isize)) -> Vec<String> {
    app.frame(size.0, size.1).iter().map(|line| plain_text(line)).collect()
}

fn frame(app: &mut Picker) -> Vec<String> {
    frame_at(app, WIDE)
}

/// The column of the separator between the list and the preview.
fn separator_column() -> usize {
    (1 + render::compute_layout(WIDE.0, WIDE.1, true).list_width + 1) as usize
}

#[test]
fn on_by_default_at_the_shipped_size() {
    let (_fresh, _tree, items) = picker_tree();
    let mut app = picker(items);
    let lines = frame(&mut app);
    assert!(app.preview_showing);
    let body = lines.join("\n");
    assert!(body.contains("src/"));
    assert!(body.contains("README.md"));
    for line in &lines {
        assert!(display_width(line) as isize <= WIDE.0, "{line:?}");
    }
}

#[test]
fn it_follows_the_selection() {
    let (_fresh, _tree, items) = picker_tree();
    let mut app = picker(items);
    frame(&mut app);
    app.handle_key("down");
    let body = frame(&mut app).join("\n");
    assert!(body.contains("empty"));
    assert!(!body.contains("README.md"));
}

#[test]
fn the_separator_meets_both_rules() {
    let (_fresh, _tree, items) = picker_tree();
    let mut app = picker(items);
    let lines = frame(&mut app);
    let column = separator_column();
    assert_eq!(at(&lines[1], column), '┬');
    assert_eq!(at(&lines[2], column), '│');
    let bottom = (2..lines.len()).rev().find(|&i| lines[i].contains('┴')).expect("a bottom junction");
    assert_eq!(at(&lines[bottom], column), '┴');
}

#[test]
fn false_in_the_config_hides_it_until_ctrl_o() {
    let (_fresh, _tree, items) = picker_tree();
    let mut app =
        PickerSpec { catalog: items, config: Some(config(r#"{"roots": [], "preview": false}"#)), ..Default::default() }
            .build();
    assert!(!frame(&mut app)[1].contains('┬'));
    app.handle_key("ctrl+o");
    assert!(frame(&mut app)[1].contains('┬'));
    app.handle_key("ctrl+o");
    assert!(!frame(&mut app)[1].contains('┬'));
}

#[test]
fn ctrl_o_brings_it_up_where_auto_would_not() {
    let (_fresh, _tree, items) = picker_tree();
    let size = (render::PREVIEW_AUTO - 1, 28);
    let mut app = picker(items);
    assert!(!frame_at(&mut app, size)[1].contains('┬'));
    app.handle_key("ctrl+o");
    assert!(frame_at(&mut app, size)[1].contains('┬'));
}

#[test]
fn the_separator_is_the_scrollbar_beside_a_long_list() {
    let (_fresh, tree, _items) = picker_tree();
    let many =
        (0..30).map(|i| format!("x{i:02}")).map(|name| suggestion(&tree.join(&name), &name, SOURCE_ROOT)).collect();
    let mut app = picker(many);
    let lines = frame(&mut app);
    let column = separator_column();
    let marks: Vec<String> = lines[2..12].iter().map(|line| at(line, column).to_string()).collect();
    assert!(marks.contains(&render::SCROLL_THUMB.to_string()));
    // One vertical line, not a scrollbar and a separator side by side.
    assert!(!lines[11].contains(&format!("{} {}", render::SCROLL_TRACK, render::SCROLL_TRACK)));
}

#[test]
fn a_click_on_the_preview_or_the_air_under_the_list_selects_nothing() {
    let (_fresh, _tree, items) = picker_tree();
    let mut app = picker(items);
    frame(&mut app);
    let right = (app.list_right + 3) as i128;
    assert!(app.handle_mouse(MouseKind::Press, right, (app.row_top + 2) as i128).is_none());
    assert_eq!(app.selected, 0);
    let below = (app.row_top + app.list_height + 2) as i128;
    assert!(app.handle_mouse(MouseKind::Press, 4, below).is_none());
    assert_eq!(app.selected, 0);
}

#[test]
fn it_shows_the_space_and_the_agents_in_the_folder() {
    let (_fresh, tree, items) = picker_tree();
    let folder = tree.join("api");
    let mut snap = snapshot(vec![pane("w1:p1", &folder, "w1")], None);
    set(&mut snap, "agents", json(r#"[{"pane_id": "w1:p1", "agent": "claude", "agent_status": "blocked"}]"#));
    let mut app = PickerSpec { catalog: items, snap: Some(snap), ..Default::default() }.build();
    let body = frame(&mut app).join("\n");
    assert!(body.contains("space   Atlas"));
    assert!(body.contains("◉ claude  needs you"));
}

#[test]
fn creating_asks_the_filesystem_afresh() {
    let (_fresh, tree, items) = picker_tree();
    let mut app = picker(items);
    frame(&mut app);
    std::fs::remove_dir(tree.join("web")).unwrap();
    app.handle_key("down");
    frame(&mut app);
    assert!(app.choice().is_none());
}

// -- ConfigTest --------------------------------------------------------------

#[test]
fn auto_true_and_false() {
    assert_eq!(Config::default().preview, Value::str("auto"));
    assert!(config(r#"{"preview": true}"#).preview.is_true());
    assert!(config(r#"{"preview": false}"#).preview.is_false());
    assert_eq!(config(r#"{"preview": "sometimes"}"#).preview, Value::str("auto"));
}
