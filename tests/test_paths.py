from __future__ import annotations

import os
import unittest

from herdr_newtab import paths
from herdr_newtab.paths import (
    SOURCE_RECENT,
    SOURCE_ROOT,
    SOURCE_SPACE,
    Suggestion,
    accept_text,
    build_catalog,
    complete_path,
    completion_for,
    filter_catalog,
    folder_name,
    looks_like_path,
    resolve_input,
    split_path_query,
    suggest,
)

from .fixtures import Tree, pane, snapshot


class QueryShapeTest(unittest.TestCase):
    def test_path_shapes(self):
        for query in ("/a", "~/a", "./a", "a/b", "$HOME/x"):
            self.assertTrue(looks_like_path(query), query)

    def test_bare_words_are_not_paths(self):
        for query in ("", "tri", "herdr bar"):
            self.assertFalse(looks_like_path(query), query)

    def test_trailing_separator_lists_the_folder_itself(self):
        directory, leaf, itself = split_path_query("/a/b/", "/base")
        self.assertEqual((directory, leaf, itself), ("/a/b", "", True))

    def test_a_partial_leaf_lists_its_parent(self):
        directory, leaf, itself = split_path_query("/a/bc", "/base")
        self.assertEqual((directory, leaf, itself), ("/a", "bc", False))

    def test_tilde_alone_means_home(self):
        directory, leaf, itself = split_path_query("~", "/base")
        self.assertEqual((directory, leaf, itself), (os.path.expanduser("~"), "", True))

    def test_relative_queries_hang_off_the_base(self):
        directory, _, _ = split_path_query("./sub/x", "/base")
        self.assertEqual(directory, "/base/sub")

    def test_resolve_input_is_absolute(self):
        self.assertEqual(resolve_input("sub", "/base"), "/base/sub")
        self.assertEqual(resolve_input("", "/base"), "")


class CompletePathTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("alpha", "alpaca", "beta", "beta/inner", ".hidden")
        paths.clear_cache()

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def names(self, query, hidden=False):
        return [row.suggestion.name for row in complete_path(query, self.tree.root, hidden, 50)]

    def test_prefix_matches_lead(self):
        self.assertEqual(self.names(self.tree.path("alp")), ["alpaca", "alpha"])

    def test_fuzzy_matches_follow_prefix_matches(self):
        found = self.names(self.tree.path("aa"))
        self.assertIn("alpaca", found)
        self.assertNotIn("beta", found)

    def test_a_trailing_separator_offers_the_folder_and_its_children(self):
        found = self.names(self.tree.path("beta") + os.sep)
        self.assertEqual(found[0], "beta")
        self.assertIn("inner", found)

    def test_hidden_folders_are_opt_in(self):
        self.assertNotIn(".hidden", self.names(self.tree.root + os.sep))
        self.assertIn(".hidden", self.names(self.tree.root + os.sep, hidden=True))

    def test_a_missing_directory_yields_nothing(self):
        self.assertEqual(self.names("/no/such/place/x"), [])

    def test_git_folders_are_marked(self):
        os.makedirs(self.tree.path("beta", ".git"), exist_ok=True)
        paths.clear_cache()
        rows = complete_path(self.tree.root + os.sep, self.tree.root, False, 50)
        marked = {row.suggestion.name: row.suggestion.is_git for row in rows}
        self.assertTrue(marked["beta"])
        self.assertFalse(marked["alpha"])


class CompletionTextTest(unittest.TestCase):
    def setUp(self):
        self.home = os.path.expanduser("~")
        self.suggestion = Suggestion(
            path=os.path.join(self.home, "workspace", "herdr"),
            name="herdr",
            source=SOURCE_ROOT,
        )

    def test_ghost_extends_what_was_typed(self):
        self.assertEqual(completion_for("~/workspace/her", self.suggestion), "dr/")

    def test_ghost_is_empty_when_the_row_is_not_an_extension(self):
        self.assertEqual(completion_for("herd", self.suggestion), "")

    def test_ghost_is_empty_on_an_empty_query(self):
        self.assertEqual(completion_for("", self.suggestion), "")

    def test_ghost_ignores_case(self):
        self.assertEqual(completion_for("~/workspace/HER", self.suggestion), "dr/")

    def test_accept_keeps_the_tilde_the_query_used(self):
        self.assertEqual(accept_text("~/work", self.suggestion), "~/workspace/herdr/")

    def test_accept_spells_it_out_for_an_absolute_query(self):
        self.assertEqual(
            accept_text("/Users", self.suggestion),
            os.path.join(self.home, "workspace", "herdr") + os.sep,
        )

    def test_accept_always_ends_in_a_separator_so_it_can_drill_down(self):
        self.assertTrue(accept_text("~", self.suggestion).endswith(os.sep))

    def test_home_is_named_for_what_it_is(self):
        self.assertEqual(folder_name(self.home), "~")
        self.assertEqual(folder_name("/a/b"), "b")


class CatalogTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("one", "two", "three")
        paths.clear_cache()

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def test_roots_are_scanned_one_level_deep(self):
        catalog = build_catalog(snapshot([]), [self.tree.root], use_zoxide=False)
        self.assertEqual(sorted(item.name for item in catalog), ["one", "three", "two"])
        self.assertTrue(all(item.source == SOURCE_ROOT for item in catalog))

    def test_an_open_space_relabels_a_folder_from_any_source(self):
        snap = snapshot([pane("w1:p1", self.tree.path("one"))])
        catalog = build_catalog(snap, [self.tree.root], use_zoxide=False)
        found = {item.name: item for item in catalog}
        self.assertEqual(found["one"].source, SOURCE_SPACE)
        self.assertEqual(found["one"].detail, "Atlas")
        self.assertEqual(found["two"].source, SOURCE_ROOT)

    def test_recents_lead_and_are_not_duplicated_by_the_scan(self):
        catalog = build_catalog(
            snapshot([]),
            [self.tree.root],
            recents=[self.tree.path("two")],
            use_zoxide=False,
        )
        self.assertEqual(catalog[0].name, "two")
        self.assertEqual(catalog[0].source, SOURCE_RECENT)
        self.assertEqual(sum(1 for item in catalog if item.name == "two"), 1)

    def test_recents_that_no_longer_exist_are_dropped(self):
        catalog = build_catalog(
            snapshot([]),
            [],
            recents=[self.tree.path("gone")],
            use_zoxide=False,
        )
        self.assertEqual(catalog, [])

    def test_a_missing_root_is_ignored(self):
        catalog = build_catalog(snapshot([]), ["/no/such/root"], use_zoxide=False)
        self.assertEqual(catalog, [])


class FilterTest(unittest.TestCase):
    def setUp(self):
        self.catalog = [
            Suggestion("/a/herdr-bar", "herdr-bar", SOURCE_ROOT),
            Suggestion("/a/herdr", "herdr", SOURCE_ROOT),
            Suggestion("/herdr/other", "other", SOURCE_ROOT),
        ]

    def test_an_empty_query_keeps_the_resting_order(self):
        self.assertEqual(
            [row.suggestion.name for row in filter_catalog(self.catalog, "", 10)],
            ["herdr-bar", "herdr", "other"],
        )

    def test_a_name_hit_outranks_the_same_hit_in_a_path(self):
        found = [row.suggestion.name for row in filter_catalog(self.catalog, "herdr", 10)]
        self.assertEqual(found[0], "herdr")
        self.assertEqual(found[-1], "other")

    def test_terms_are_anded(self):
        found = filter_catalog(self.catalog, "herdr bar", 10)
        self.assertEqual([row.suggestion.name for row in found], ["herdr-bar"])

    def test_no_match_is_no_rows(self):
        self.assertEqual(filter_catalog(self.catalog, "zzz", 10), [])

    def test_the_limit_is_honoured(self):
        self.assertEqual(len(filter_catalog(self.catalog, "", 2)), 2)

    def test_matched_positions_come_back_for_highlighting(self):
        row = filter_catalog(self.catalog, "hrd", 10)[0]
        self.assertTrue(row.name_positions)


class SuggestTest(unittest.TestCase):
    def setUp(self):
        self.tree = Tree("alpha")
        paths.clear_cache()

    def tearDown(self):
        self.tree.close()
        paths.clear_cache()

    def test_a_path_query_goes_to_the_filesystem(self):
        catalog = [Suggestion("/elsewhere/alpha", "alpha", SOURCE_ROOT)]
        rows = suggest(self.tree.path("alp"), catalog, self.tree.root, False, 10)
        self.assertEqual([row.suggestion.path for row in rows], [self.tree.path("alpha")])

    def test_a_bare_query_goes_to_the_catalog(self):
        catalog = [Suggestion("/elsewhere/alpha", "alpha", SOURCE_ROOT)]
        rows = suggest("alpha", catalog, self.tree.root, False, 10)
        self.assertEqual([row.suggestion.path for row in rows], ["/elsewhere/alpha"])


if __name__ == "__main__":
    unittest.main()
