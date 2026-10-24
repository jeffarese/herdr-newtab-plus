//! Ported from the Python release's tests/test_paths.py.
#![allow(unused_imports)]

use crate::paths::{
    self, accept_text, build_catalog, complete_path, completion_for, filter_catalog, folder_name, looks_like_path,
    resolve_input, split_path_query, suggest, Row, Suggestion, SOURCE_RECENT, SOURCE_ROOT, SOURCE_SPACE,
};
use crate::testutil::*;

// -- QueryShapeTest -----------------------------------------------------------

#[test]
fn path_shapes() {
    for query in ["/a", "~/a", "./a", "a/b", "$HOME/x"] {
        assert!(looks_like_path(query), "{}", query);
    }
}

#[test]
fn bare_words_are_not_paths() {
    for query in ["", "tri", "herdr bar"] {
        assert!(!looks_like_path(query), "{}", query);
    }
}

#[test]
fn trailing_separator_lists_the_folder_itself() {
    let found = split_path_query("/a/b/", Some("/base"));
    assert_eq!(found, ("/a/b".to_string(), String::new(), true));
}

#[test]
fn a_partial_leaf_lists_its_parent() {
    let found = split_path_query("/a/bc", Some("/base"));
    assert_eq!(found, ("/a".to_string(), "bc".to_string(), false));
}

#[test]
fn tilde_alone_means_home() {
    let _lock = env_lock();
    let found = split_path_query("~", Some("/base"));
    assert_eq!(found, (crate::pyos::expanduser("~"), String::new(), true));
}

#[test]
fn relative_queries_hang_off_the_base() {
    let (directory, _, _) = split_path_query("./sub/x", Some("/base"));
    assert_eq!(directory, "/base/sub");
}

#[test]
fn resolve_input_is_absolute() {
    assert_eq!(resolve_input("sub", "/base"), "/base/sub");
    assert_eq!(resolve_input("", "/base"), "");
}

// -- CompletePathTest ---------------------------------------------------------

/// The tree CompletePathTest completes against, with the caches cleared on
/// both sides of the test.
struct CompleteFixture {
    tree: TempDir,
}

impl CompleteFixture {
    fn new() -> Self {
        let tree = TempDir::with(&["alpha", "alpaca", "beta", "beta/inner", ".hidden"]);
        paths::clear_cache();
        CompleteFixture { tree }
    }

    fn root(&self) -> &str {
        self.tree.path()
    }

    fn names(&self, query: &str, hidden: bool) -> Vec<String> {
        complete_path(query, self.root(), hidden, 50).iter().map(|row| row.suggestion.name.clone()).collect()
    }
}

impl Drop for CompleteFixture {
    fn drop(&mut self) {
        paths::clear_cache();
    }
}

#[test]
fn prefix_matches_lead() {
    let f = CompleteFixture::new();
    assert_eq!(f.names(&f.tree.join("alp"), false), vec!["alpaca", "alpha"]);
}

#[test]
fn fuzzy_matches_follow_prefix_matches() {
    let f = CompleteFixture::new();
    let found = f.names(&f.tree.join("aa"), false);
    assert!(found.iter().any(|n| n == "alpaca"));
    assert!(!found.iter().any(|n| n == "beta"));
}

#[test]
fn a_trailing_separator_offers_the_folder_and_its_children() {
    let f = CompleteFixture::new();
    let found = f.names(&(f.tree.join("beta") + "/"), false);
    assert_eq!(found[0], "beta");
    assert!(found.iter().any(|n| n == "inner"));
}

#[test]
fn hidden_folders_are_opt_in() {
    let f = CompleteFixture::new();
    let query = format!("{}/", f.root());
    assert!(!f.names(&query, false).iter().any(|n| n == ".hidden"));
    assert!(f.names(&query, true).iter().any(|n| n == ".hidden"));
}

#[test]
fn a_missing_directory_yields_nothing() {
    let f = CompleteFixture::new();
    assert!(f.names("/no/such/place/x", false).is_empty());
}

#[test]
fn git_folders_are_marked() {
    let f = CompleteFixture::new();
    std::fs::create_dir_all(f.tree.join("beta/.git")).unwrap();
    paths::clear_cache();
    let rows = complete_path(&format!("{}/", f.root()), f.root(), false, 50);
    let marked = |name: &str| rows.iter().find(|row| row.suggestion.name == name).unwrap().suggestion.is_git;
    assert!(marked("beta"));
    assert!(!marked("alpha"));
}

// -- CompletionTextTest -------------------------------------------------------

/// Home, and a root row under it. Holds the env lock, so a test elsewhere
/// that moves HOME cannot change it halfway through.
fn home_suggestion() -> (String, Suggestion, std::sync::MutexGuard<'static, ()>) {
    let lock = env_lock();
    let home = paths::home();
    let suggestion = Suggestion::new(&format!("{}/workspace/herdr", home), "herdr", SOURCE_ROOT);
    (home, suggestion, lock)
}

#[test]
fn ghost_extends_what_was_typed() {
    let (_, s, _lock) = home_suggestion();
    assert_eq!(completion_for("~/workspace/her", Some(&s)), "dr/");
}

#[test]
fn ghost_is_empty_when_the_row_is_not_an_extension() {
    let (_, s, _lock) = home_suggestion();
    assert_eq!(completion_for("herd", Some(&s)), "");
}

#[test]
fn ghost_is_empty_on_an_empty_query() {
    let (_, s, _lock) = home_suggestion();
    assert_eq!(completion_for("", Some(&s)), "");
}

#[test]
fn ghost_ignores_case() {
    let (_, s, _lock) = home_suggestion();
    assert_eq!(completion_for("~/workspace/HER", Some(&s)), "dr/");
}

#[test]
fn accept_keeps_the_tilde_the_query_used() {
    let (_, s, _lock) = home_suggestion();
    assert_eq!(accept_text("~/work", &s), "~/workspace/herdr/");
}

#[test]
fn accept_spells_it_out_for_an_absolute_query() {
    let (home, s, _lock) = home_suggestion();
    assert_eq!(accept_text("/Users", &s), format!("{}/workspace/herdr/", home));
}

#[test]
fn accept_always_ends_in_a_separator_so_it_can_drill_down() {
    let (_, s, _lock) = home_suggestion();
    assert!(accept_text("~", &s).ends_with('/'));
}

#[test]
fn home_is_named_for_what_it_is() {
    let (home, _, _lock) = home_suggestion();
    assert_eq!(folder_name(&home), "~");
    assert_eq!(folder_name("/a/b"), "b");
}

// -- CatalogTest --------------------------------------------------------------

struct CatalogFixture {
    tree: TempDir,
}

impl CatalogFixture {
    fn new() -> Self {
        let tree = TempDir::with(&["one", "two", "three"]);
        paths::clear_cache();
        CatalogFixture { tree }
    }

    fn roots(&self) -> Vec<String> {
        vec![self.tree.path().to_string()]
    }
}

impl Drop for CatalogFixture {
    fn drop(&mut self) {
        paths::clear_cache();
    }
}

fn find<'a>(catalog: &'a [Row], name: &str) -> &'a Row {
    catalog.iter().find(|item| item.name == name).unwrap()
}

#[test]
fn roots_are_scanned_one_level_deep() {
    let f = CatalogFixture::new();
    let catalog = build_catalog(&snapshot(vec![], None), &f.roots(), &[], false, false);
    let mut names: Vec<&str> = catalog.iter().map(|item| item.name.as_str()).collect();
    names.sort();
    assert_eq!(names, vec!["one", "three", "two"]);
    assert!(catalog.iter().all(|item| item.source == SOURCE_ROOT));
}

#[test]
fn an_open_space_relabels_a_folder_from_any_source() {
    let f = CatalogFixture::new();
    let snap = snapshot(vec![pane("w1:p1", &f.tree.join("one"), "w1")], None);
    let catalog = build_catalog(&snap, &f.roots(), &[], false, false);
    assert_eq!(find(&catalog, "one").source, SOURCE_SPACE);
    assert_eq!(find(&catalog, "one").detail, "Atlas");
    assert_eq!(find(&catalog, "two").source, SOURCE_ROOT);
}

#[test]
fn recents_lead_and_are_not_duplicated_by_the_scan() {
    let f = CatalogFixture::new();
    let catalog = build_catalog(&snapshot(vec![], None), &f.roots(), &[f.tree.join("two")], false, false);
    assert_eq!(catalog[0].name, "two");
    assert_eq!(catalog[0].source, SOURCE_RECENT);
    assert_eq!(catalog.iter().filter(|item| item.name == "two").count(), 1);
}

#[test]
fn recents_that_no_longer_exist_are_dropped() {
    let f = CatalogFixture::new();
    let catalog = build_catalog(&snapshot(vec![], None), &[], &[f.tree.join("gone")], false, false);
    assert!(catalog.is_empty());
}

#[test]
fn a_missing_root_is_ignored() {
    let _f = CatalogFixture::new();
    let catalog = build_catalog(&snapshot(vec![], None), &["/no/such/root".to_string()], &[], false, false);
    assert!(catalog.is_empty());
}

/// The root rows made while zoxide is still answering must make the catalog
/// adding the listing row by row makes: the same rows in the same order, each
/// duplicate of an earlier source merged into it, the home folder named for
/// what it is. A folder whose name normalises onto a sibling makes the
/// listing fall back to the row-by-row way.
#[test]
fn root_rows_made_ahead_make_the_catalog_row_by_row_does() {
    let _lock = env_lock();
    let mut env = EnvVars::new();
    env.set("NTP_SIBLING", "beta");
    let tree = TempDir::with(&[
        "root/alpha/.git",
        "root/beta",
        "root/gamma/.git",
        "root/me",
        "root/open/.git",
        "root/zed",
        "other",
    ]);
    let previous_home = paths::home();
    paths::set_home(&tree.join("root/me"));
    let root = vec![tree.join("root")];
    let snap = snapshot(vec![pane("w1:p1", &tree.join("root/open"), "w1")], None);
    let owned = crate::session::space_folders(&snap);
    let recents = vec![tree.join("root/alpha"), tree.join("other")];
    let zoxide = Some(vec![tree.join("root/gamma"), tree.join("other"), tree.join("root/open")]);
    let build = |ahead: bool| {
        paths::clear_cache();
        let mut builder = paths::CatalogBuilder::new(&owned, Default::default());
        builder.add_recents_and_spaces(&recents);
        let prepared =
            if ahead { builder.prepare_roots(&root, false) } else { builder.prepare_roots_row_by_row(&root, false) };
        builder.add_zoxide(zoxide.clone(), Default::default());
        builder.add_roots(prepared);
        let rows: Vec<(String, String, &str, String, bool)> = builder
            .finish()
            .iter()
            .map(|row| (pyos_rel(&row.path, tree.path()), row.name.clone(), row.source, row.detail.clone(), row.is_git))
            .collect();
        rows
    };
    let ahead = build(true);
    assert_eq!(ahead, build(false));
    let names: Vec<(&str, &str)> = ahead.iter().map(|row| (row.1.as_str(), row.2)).collect();
    assert_eq!(
        names[..4],
        [("alpha", SOURCE_RECENT), ("other", SOURCE_RECENT), ("open", SOURCE_SPACE), ("gamma", "zoxide")]
    );
    assert_eq!(ahead.len(), 7, "{ahead:?}");
    assert!(ahead.iter().any(|row| row.0 == "root/me" && row.1 == folder_name(&tree.join("root/me"))));

    std::fs::create_dir(tree.join("root/$NTP_SIBLING")).unwrap();
    let fallback = build(true);
    assert_eq!(fallback, build(false));
    assert_eq!(fallback.iter().filter(|row| row.0 == "root/beta").count(), 1, "{fallback:?}");

    paths::set_home(&previous_home);
    paths::clear_cache();
}

/// The rows the first frame can be drawn from before zoxide is in: the
/// catalog's own first rows, as many as are on screen, then enough of the
/// roots' rows that are not repeats to make the list's length, which zoxide
/// can only add to. Short of either, there are none.
#[test]
fn leading_rows_are_the_whole_catalogs_first_rows() {
    let _lock = env_lock();
    let tree =
        TempDir::with(&["root/alpha/.git", "root/beta", "root/gamma/.git", "root/open/.git", "root/delta", "other"]);
    paths::clear_cache();
    let root = vec![tree.join("root")];
    let snap = snapshot(vec![pane("w1:p1", &tree.join("root/open"), "w1")], None);
    let owned = crate::session::space_folders(&snap);
    let recents = vec![tree.join("root/alpha"), tree.join("other"), tree.join("root/beta")];
    // zoxide repeats a recent and the space, and the root repeats them all.
    let zoxide = Some(vec![tree.join("root/gamma"), tree.join("root/beta"), tree.join("root/open")]);
    let mut builder = paths::CatalogBuilder::new(&owned, Default::default());
    builder.add_recents_and_spaces(&recents);
    let prepared = builder.prepare_roots(&root, false);
    assert!(builder.leading_rows(Some(&prepared), 5, 6).is_none(), "four rows cannot fill five on screen");
    assert!(builder.leading_rows(Some(&prepared), 4, 7).is_none(), "the root adds two, so six is all that is known");
    assert!(builder.leading_rows(None, 4, 6).is_none(), "without the root, four rows are all that is known");
    assert_eq!(builder.leading_rows(None, 4, 4).map(|rows| rows.len()), Some(4));
    let leading = builder.leading_rows(Some(&prepared), 4, 6).unwrap();
    builder.add_zoxide(zoxide, Default::default());
    builder.add_roots(prepared);
    let full = builder.finish();
    assert_eq!(full.len(), 6, "{full:?}");
    assert_eq!(leading.len(), 6);
    assert_eq!(leading[..4], full[..4]);
    assert!(leading.iter().zip(&full).all(|(copy, row)| !std::sync::Arc::ptr_eq(copy, row)));
    paths::clear_cache();
}

/// `path` written relative to `base`, for comparing rows across trees.
fn pyos_rel(path: &str, base: &str) -> String {
    path.strip_prefix(base).map(|rest| rest.trim_start_matches('/').to_string()).unwrap_or_else(|| path.to_string())
}

/// The catalog filter as it first was: every row matched with its
/// positions, all of them sorted, the list cut after. What the faster one
/// is held to.
fn every_row_scored(catalog: &[Row], query: &str, limit: usize) -> Vec<paths::Scored> {
    let terms = paths::terms_of(query);
    if terms.is_empty() {
        return catalog.iter().take(limit).map(|item| paths::Scored::plain(item.clone())).collect();
    }
    let mut scored: Vec<paths::Scored> = catalog.iter().filter_map(|item| paths::score(item, &terms)).collect();
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| paths::rank(a.suggestion.source).cmp(&paths::rank(b.suggestion.source)))
            .then_with(|| a.suggestion.path.cmp(&b.suggestion.path))
    });
    scored.truncate(limit);
    scored
}

/// Typing on scores only the rows the shorter query matched. Whatever is
/// typed -- on, back, a space, a quote, a capital, a character that folds --
/// every step must show the rows a filter of the whole catalog shows, with
/// the same scores and highlights.
#[test]
fn narrowing_from_the_last_query_filters_as_the_whole_catalog_does() {
    let names = [
        "herdr-bar",
        "herdr-newtab-plus",
        "Herdr-Radar",
        "HERDR",
        "web-relay",
        "Webhook",
        "café-ledger",
        "CAFÉ",
        "İstanbul",
        "istanbul",
        "日本語",
        "a b c",
        "quote\"d",
        "x-1",
        "X1",
        "notes",
        "atlas",
        "Atlas-Web",
    ];
    let catalog: Vec<Row> = names
        .iter()
        .enumerate()
        .map(|(index, name)| {
            suggestion(&format!("/w/{}/{}", ["src", "Docs", "ab"][index % 3], name), name, SOURCE_ROOT)
        })
        .collect();
    let rows = |found: &[paths::Scored]| -> Vec<(String, i64, Vec<usize>, Vec<usize>)> {
        found
            .iter()
            .map(|row| (row.suggestion.name.clone(), row.score, row.name_positions.clone(), row.path_positions.clone()))
            .collect()
    };
    let alphabet: Vec<char> = "herdabwX1 \"'Iİíé日-".chars().collect();
    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |below: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % below as u64) as usize
    };
    for _ in 0..300 {
        let mut last = paths::Narrowing::default();
        let mut query = String::new();
        for _ in 0..12 {
            if !query.is_empty() && next(4) == 0 {
                query.pop();
            } else {
                query.push(alphabet[next(alphabet.len())]);
            }
            let narrowed = paths::filter_catalog_narrowing(&catalog, &query, 5, &mut last);
            let reference = rows(&every_row_scored(&catalog, &query, 5));
            assert_eq!(rows(&narrowed), reference, "{query:?}");
            assert_eq!(rows(&filter_catalog(&catalog, &query, 5)), reference, "{query:?}");
        }
    }
}

// -- FilterTest ---------------------------------------------------------------

fn filter_fixture() -> Vec<Row> {
    vec![
        suggestion("/a/herdr-bar", "herdr-bar", SOURCE_ROOT),
        suggestion("/a/herdr", "herdr", SOURCE_ROOT),
        suggestion("/herdr/other", "other", SOURCE_ROOT),
    ]
}

fn filtered_names(catalog: &[Row], query: &str, limit: usize) -> Vec<String> {
    filter_catalog(catalog, query, limit).iter().map(|row| row.suggestion.name.clone()).collect()
}

#[test]
fn an_empty_query_keeps_the_resting_order() {
    assert_eq!(filtered_names(&filter_fixture(), "", 10), vec!["herdr-bar", "herdr", "other"]);
}

#[test]
fn a_name_hit_outranks_the_same_hit_in_a_path() {
    let found = filtered_names(&filter_fixture(), "herdr", 10);
    assert_eq!(found[0], "herdr");
    assert_eq!(found.last().unwrap(), "other");
}

#[test]
fn terms_are_anded() {
    assert_eq!(filtered_names(&filter_fixture(), "herdr bar", 10), vec!["herdr-bar"]);
}

#[test]
fn no_match_is_no_rows() {
    assert!(filter_catalog(&filter_fixture(), "zzz", 10).is_empty());
}

#[test]
fn the_limit_is_honoured() {
    assert_eq!(filter_catalog(&filter_fixture(), "", 2).len(), 2);
}

#[test]
fn matched_positions_come_back_for_highlighting() {
    let rows = filter_catalog(&filter_fixture(), "hrd", 10);
    assert!(!rows[0].name_positions.is_empty());
}

// -- SuggestTest --------------------------------------------------------------

struct SuggestFixture {
    tree: TempDir,
}

impl SuggestFixture {
    fn new() -> Self {
        let tree = TempDir::with(&["alpha"]);
        paths::clear_cache();
        SuggestFixture { tree }
    }
}

impl Drop for SuggestFixture {
    fn drop(&mut self) {
        paths::clear_cache();
    }
}

#[test]
fn a_path_query_goes_to_the_filesystem() {
    let f = SuggestFixture::new();
    let catalog = vec![suggestion("/elsewhere/alpha", "alpha", SOURCE_ROOT)];
    let rows = suggest(&f.tree.join("alp"), &catalog, f.tree.path(), false, 10);
    let found: Vec<&str> = rows.iter().map(|row| row.suggestion.path.as_str()).collect();
    assert_eq!(found, vec![f.tree.join("alpha")]);
}

#[test]
fn a_bare_query_goes_to_the_catalog() {
    let f = SuggestFixture::new();
    let catalog = vec![suggestion("/elsewhere/alpha", "alpha", SOURCE_ROOT)];
    let rows = suggest("alpha", &catalog, f.tree.path(), false, 10);
    let found: Vec<&str> = rows.iter().map(|row| row.suggestion.path.as_str()).collect();
    assert_eq!(found, vec!["/elsewhere/alpha"]);
}
