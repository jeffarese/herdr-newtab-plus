//! The popup size in herdr-plugin.toml, held to what it was chosen for. Ported
//! from the Python release's CI step "Manifest parses".

use crate::render::{compute_layout, render_help, MAX_LIST, PREVIEW_AUTO};
use crate::session::SMART;
use crate::theme::Theme;

/// The pane's `width` and `height`, as Herdr reads them.
fn popup() -> (isize, isize) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("herdr-plugin.toml");
    let text = std::fs::read_to_string(path).unwrap();
    let pane: Vec<&str> =
        text.lines().skip_while(|l| l.trim() != "[[panes]]").take_while(|l| !l.starts_with("[[actions")).collect();
    let number = |key: &str| -> isize {
        pane.iter()
            .find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('=')?.trim().parse().ok())
            .unwrap_or_else(|| panic!("no {key} in [[panes]]"))
    };
    (number("width"), number("height"))
}

/// What Herdr hands the pty: `width - 3` columns and `height - 2` rows.
fn inner() -> (isize, isize) {
    let (width, height) = popup();
    (width - 3, height - 2)
}

#[test]
fn the_chrome_fits_the_shipped_popup() {
    let (width, height) = inner();
    let layout = compute_layout(width, height, false);
    assert!(layout.footer && layout.chips && layout.target, "chrome does not fit");
    assert!(layout.note, "no room for the line spelling the create mode out");
}

#[test]
fn the_list_stays_capped_however_tall_the_popup_is() {
    // The popup is sized for the help panel, so what the manifest has to keep
    // true is the other end: the picker's list stays capped.
    let (width, height) = inner();
    assert_eq!(compute_layout(width, height, false).list_height, MAX_LIST);
}

#[test]
fn the_help_panel_says_all_of_it() {
    let (width, height) = inner();
    let panel = render_help(&Theme::new(None, None, None), width - 2, height, SMART);
    assert!(!panel.last().unwrap().trim().is_empty(), "the help panel does not fit the popup it sized");
}

#[test]
fn the_preview_shows_by_itself_at_the_shipped_size() {
    let (width, height) = inner();
    assert!(width - 2 >= PREVIEW_AUTO, "too narrow for the preview to show by default");
    assert!(compute_layout(width, height, true).preview_width > 0, "no room for the preview");
}
