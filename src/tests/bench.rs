//! The counts the Python release's `scripts/bench.py --check` gated in CI:
//! bytes per frame and questions asked of the filesystem per frame. Unlike
//! timings they are properties of the code, the same on every machine, so a
//! frame that stopped diffing or started walking the filesystem fails here
//! however fast the machine running it is.
//!
//! The workload is bench.py's: a deterministic catalog of the shape a real
//! one has, a session with spaces and panes, and a tree of folders made in a
//! temp directory and folded to `~` the way a home is.

use crate::agents::Agent;
use crate::app::Picker;
use crate::lookups::counted;
use crate::paint::Painter;
use crate::paths::{self, Row, SOURCE_RECENT, SOURCE_ROOT, SOURCE_SPACE, SOURCE_ZOXIDE};
use crate::pyjson::Value;
use crate::state::Recents;
use crate::testutil::*;
use crate::theme::Theme;

const WIDTH: usize = 107;
const HEIGHT: usize = 28;

/// What a keystroke workload looks like: a name typed one character at a
/// time, narrowing from the whole catalog down to a couple of rows.
const TYPED: &str = "herdr-newtab";

/// How much of the catalog gets a folder on disk, so every row the scenarios
/// land on previews something real.
const STAGED: usize = 400;

const DIR_POOL: [&str; 5] = ["src", "tests", "docs", "scripts", "assets"];
const FILE_POOL: [&str; 6] = ["README.md", "LICENSE", "pyproject.toml", "Makefile", "CHANGELOG.md", ".gitignore"];

/// Names past ASCII, so the width cache's per-character path is exercised.
const WIDE_NAMES: [&str; 3] = ["日本語プロジェクト", "café-ledger", "emoji-🚀-lab"];
const STEMS: [&str; 9] = [
    "herdr-bar",
    "herdr-newtab-plus",
    "webhook-relay",
    "mobile-client",
    "log-sink",
    "classifier",
    "notes",
    "atlas",
    "sandbox",
];

// Hard ceilings, as generous as bench.py's: they catch a frame that stopped
// diffing or started statting per row, not a layout somebody meant to change.
const KEYSTROKE_BYTES: usize = 800;
const NAV_BYTES: usize = 1400;
const FRAME_CALLS: usize = 4;

/// A home made of fixture folders, folded to `~`, put back on drop.
struct Home {
    _dir: TempDir,
    base: String,
    previous: String,
}

impl Home {
    /// The folders `catalog(STAGED)` names, each with contents of its own,
    /// varying folder to folder so moving between two has something to redraw.
    fn staged() -> Home {
        let dir = TempDir::new();
        let previous = paths::home();
        paths::set_home(dir.path());
        let home = Home { base: dir.join("workspace"), _dir: dir, previous };
        for (index, item) in home.catalog(STAGED).iter().enumerate() {
            let folder = &item.path;
            std::fs::create_dir_all(folder).unwrap();
            for name in &DIR_POOL[..index % DIR_POOL.len() + 1] {
                std::fs::create_dir(format!("{folder}/{name}")).unwrap();
            }
            for name in &FILE_POOL[..(index * 3) % FILE_POOL.len() + 1] {
                std::fs::write(format!("{folder}/{name}"), "").unwrap();
            }
            if item.is_git {
                std::fs::create_dir(format!("{folder}/.git")).unwrap();
                let branch = if index % 3 != 0 { "main".to_string() } else { format!("feature-{index}") };
                std::fs::write(format!("{folder}/.git/HEAD"), format!("ref: refs/heads/{branch}\n")).unwrap();
            }
        }
        paths::clear_cache();
        home
    }

    /// Sources mixed the way a working machine has them: a few open spaces,
    /// a handful of recents, some zoxide frecency, the rest out of the roots.
    /// Depth varies, so the parent column's middle truncation is reached.
    fn catalog(&self, count: usize) -> Vec<Row> {
        let mut sources = vec![SOURCE_SPACE; 3];
        sources.extend([SOURCE_RECENT; 8]);
        sources.extend([SOURCE_ZOXIDE; 20]);
        (0..count)
            .map(|index| {
                let name = if index < WIDE_NAMES.len() {
                    WIDE_NAMES[index].to_string()
                } else if index < WIDE_NAMES.len() + STEMS.len() {
                    STEMS[index - WIDE_NAMES.len()].to_string()
                } else {
                    format!("{}-{index:03}", STEMS[index % STEMS.len()])
                };
                let source = sources.get(index).copied().unwrap_or(SOURCE_ROOT);
                let parent =
                    if index % 3 != 0 { self.base.clone() } else { format!("{}/archive/{:02}", self.base, index % 7) };
                let detail = if source == SOURCE_SPACE { "Atlas" } else { "" };
                suggestion_full(&format!("{parent}/{name}"), &name, source, detail, index % 2 == 1)
            })
            .collect()
    }

    /// A session with three spaces and `panes` panes standing somewhere: the
    /// tab-versus-space decision walks every pane, so it is the other thing
    /// that could make a frame cost more than the last.
    fn snapshot(&self, panes: usize) -> Value {
        let spaces = (1..4).map(|index| space(&format!("w{index}"), &format!("space-{index}"), index, index == 1));
        let panes = (0..panes).map(|index| {
            let workspace = format!("w{}", index % 3 + 1);
            pane(&format!("{workspace}:p{index}"), &format!("{}/project-{index:02}", self.base), &workspace)
        });
        let mut snap = snapshot(panes.collect(), Some(spaces.collect()));
        set(&mut snap, "focused_pane_id", Value::str("w1:p1"));
        snap
    }

    fn picker(&self, items: &[Row], session: &Value) -> Picker {
        Picker::new(
            session.clone(),
            items.to_vec(),
            vec![
                Agent::new("", "shell"),
                Agent::new("claude", "claude"),
                Agent::new("codex", "codex"),
                Agent::new("opencode", "opencode"),
                Agent::new("pi", "pi"),
            ],
            config(r#"{"roots": []}"#),
            Recents::new(None),
            Theme::new(None, None, None),
            self.base.clone(),
        )
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        paths::set_home(&self.previous);
        paths::clear_cache();
    }
}

fn draw(app: &mut Picker, painter: &mut Painter) -> usize {
    let rows = app.frame(WIDTH as isize, HEIGHT as isize);
    painter.frame(&rows, WIDTH, HEIGHT).len()
}

fn median(mut values: Vec<usize>) -> usize {
    values.sort_unstable();
    values[values.len() / 2]
}

#[derive(Debug, PartialEq)]
struct Calls {
    /// Landing on a row for the first time, which is what moving down does.
    nav: usize,
    /// Coming back to one, which is what holding a key does.
    nav_repeat: usize,
    keystroke: usize,
}

/// Questions per frame, each scenario from a cleared cache and a fresh
/// picker, as the popup they model is opened fresh each time.
fn calls(home: &Home, items: &[Row], session: &Value) -> Calls {
    let fresh = || {
        paths::clear_cache();
        let mut app = home.picker(items, session);
        draw(&mut app, &mut Painter::new());
        app
    };
    let frame = |app: &mut Picker| {
        app.frame(WIDTH as isize, HEIGHT as isize);
    };

    let mut app = fresh();
    let ((), nav) = counted(|| {
        app.move_by(1, true);
        frame(&mut app);
    });

    let mut app = fresh();
    app.move_by(1, true);
    frame(&mut app);
    app.move_by(-1, true);
    frame(&mut app);
    let ((), nav_repeat) = counted(|| {
        app.move_by(1, true);
        frame(&mut app);
    });

    paths::clear_cache();
    let mut app = home.picker(items, session);
    let typed: Vec<char> = TYPED.chars().collect();
    app.set_query(typed[..typed.len() - 1].iter().collect(), None);
    draw(&mut app, &mut Painter::new());
    let last = typed[typed.len() - 1].to_string();
    let ((), keystroke) = counted(|| {
        app.insert(&last, false);
        frame(&mut app);
    });

    paths::clear_cache();
    Calls { nav, nav_repeat, keystroke }
}

/// Landing on a folder asks three things: what is in it, for the preview,
/// which also says whether it exists; which branch it is on; and what it is
/// behind its symlinks, which says whether it has a space already. Standing
/// still, or typing, asks nothing: the session is a snapshot taken when the
/// popup opened. The same counts the Python release had.
#[test]
fn a_frame_asks_the_filesystem_no_more_than_its_budget() {
    let home = Home::staged();
    let found = calls(&home, &home.catalog(400), &home.snapshot(8));
    assert_eq!(found, Calls { nav: 3, nav_repeat: 0, keystroke: 0 });
    assert!(found.nav.max(found.keystroke) <= FRAME_CALLS);
}

/// A frame only draws the rows that fit, so its cost must not grow with the
/// catalog; nor with the panes, or the snapshot is being re-derived per frame.
#[test]
fn lookups_do_not_scale_with_the_catalog_or_the_panes() {
    let home = Home::staged();
    let small = calls(&home, &home.catalog(50), &home.snapshot(8));
    let large = calls(&home, &home.catalog(1600), &home.snapshot(8));
    assert_eq!((small.nav, small.keystroke), (large.nav, large.keystroke), "50 folders vs 1600");
    let few = calls(&home, &home.catalog(400), &home.snapshot(2));
    let many = calls(&home, &home.catalog(400), &home.snapshot(32));
    assert_eq!(few.nav, many.nav, "2 panes vs 32");
}

#[test]
fn returning_to_a_folder_costs_less_than_arriving_at_it() {
    let home = Home::staged();
    let steady = calls(&home, &home.catalog(400), &home.snapshot(8));
    assert!(steady.nav_repeat < steady.nav, "{steady:?}");
}

/// One character typed changes the input and re-sorts the rows under it: a
/// few hundred bytes. Measured at every length of the word, from the first
/// keystroke, which filters the whole catalog, to the last, which filters
/// almost nothing.
#[test]
fn a_keystroke_costs_a_few_hundred_bytes() {
    let home = Home::staged();
    let (items, session) = (home.catalog(400), home.snapshot(8));
    let typed: Vec<char> = TYPED.chars().collect();
    let sizes: Vec<usize> = (1..=typed.len())
        .map(|length| {
            let mut app = home.picker(&items, &session);
            let mut painter = Painter::new();
            app.set_query(typed[..length - 1].iter().collect(), None);
            draw(&mut app, &mut painter);
            app.insert(&typed[length - 1].to_string(), false);
            draw(&mut app, &mut painter)
        })
        .collect();
    let typical = median(sizes.clone());
    assert!(typical <= KEYSTROKE_BYTES, "median {typical} bytes per keystroke: {sizes:?}");
}

/// A navigation moves a full-bleed selection band, two whole rows, and
/// redraws the lines of the preview that differ from the folder it left.
#[test]
fn a_navigation_costs_about_two_rows() {
    let home = Home::staged();
    let mut app = home.picker(&home.catalog(400), &home.snapshot(8));
    let mut painter = Painter::new();
    draw(&mut app, &mut painter);
    // bench.py's 20 warm-up steps and 300 measured ones, all of them counted.
    let sizes: Vec<usize> = (0..320)
        .map(|_| {
            app.move_by(1, true);
            draw(&mut app, &mut painter)
        })
        .collect();
    let typical = median(sizes);
    assert!(typical <= NAV_BYTES, "median {typical} bytes per navigation");
}
