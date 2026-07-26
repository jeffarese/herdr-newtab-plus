"""The benchmark is only worth its numbers if the numbers mean something.

Timings are not asserted on -- a test suite running on a shared CI box cannot
say anything true about milliseconds. What is asserted is that the harness
measures what it claims to: that its instruments are wired to the right calls,
that its scoring curve behaves at the ends and in the middle, and that the
scenarios it runs actually exercise the picker rather than silently measuring
nothing.
"""

from __future__ import annotations

import argparse
import os
import sys
import unittest

sys.path.insert(
    0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "scripts")
)

import bench  # noqa: E402

SIZE = (92, 20)


class WorkloadTest(unittest.TestCase):
    def test_catalog_is_the_size_asked_for(self):
        self.assertEqual(len(bench.catalog(400)), 400)
        self.assertEqual(len(bench.catalog(7)), 7)

    def test_catalog_is_deterministic(self):
        self.assertEqual(bench.catalog(120), bench.catalog(120))

    def test_catalog_carries_names_the_width_cache_has_to_measure(self):
        names = [item.name for item in bench.catalog(50)]
        self.assertTrue(any(not name.isascii() for name in names))

    def test_typed_query_actually_narrows_the_catalog(self):
        # A workload that matched everything, or nothing, would time the wrong
        # thing while still producing a number.
        from herdr_newtab.paths import filter_catalog

        items = bench.catalog(400)
        first = filter_catalog(items, bench.TYPED[0], 200)
        last = filter_catalog(items, bench.TYPED, 200)
        self.assertGreater(len(first), len(last))
        self.assertGreater(len(last), 0)

    def test_snapshot_panes_are_spread_over_spaces(self):
        session = bench.snapshot(8)
        self.assertEqual(len(session["panes"]), 8)
        self.assertEqual(len({pane["workspace_id"] for pane in session["panes"]}), 3)


class CallsTest(unittest.TestCase):
    HERE = os.path.dirname(os.path.abspath(__file__))

    def test_counts_a_bare_syscall(self):
        with bench.Calls() as counter:
            os.stat(self.HERE)
        self.assertEqual(counter.total, 1)

    def test_counts_a_derived_lookup_once(self):
        # os.path.isdir is built on os.stat: counting both would price it twice.
        with bench.Calls() as counter:
            os.path.isdir(self.HERE)
        self.assertEqual(counter.total, 1)

    def test_prices_realpath_by_the_question_not_the_depth(self):
        # realpath costs one lstat per component, so counting primitives would
        # make the number depend on where the repository happens to live.
        with bench.Calls() as shallow:
            os.path.realpath("/")
        with bench.Calls() as deep:
            os.path.realpath(self.HERE)
        self.assertEqual(shallow.total, deep.total)
        self.assertEqual(deep.total, 1)

    def test_counts_each_question_separately(self):
        with bench.Calls() as counter:
            os.path.isdir(self.HERE)
            os.path.realpath(self.HERE)
        self.assertEqual(counter.total, 2)

    def test_does_not_count_expanduser(self):
        with bench.Calls() as counter:
            os.path.expanduser("~/somewhere")
        self.assertEqual(counter.total, 0)

    def test_restores_what_it_wrapped(self):
        before = os.stat
        with bench.Calls():
            pass
        self.assertIs(os.stat, before)

    def test_restores_after_an_exception(self):
        before = os.stat
        try:
            with bench.Calls():
                raise ValueError("boom")
        except ValueError:
            pass
        self.assertIs(os.stat, before)


class ScenarioTest(unittest.TestCase):
    def setUp(self):
        self.items = bench.catalog(60)
        self.session = bench.snapshot(4)

    def test_keystroke_measures_every_character(self):
        result = bench.bench_keystroke(self.items, self.session, SIZE, 24)
        self.assertGreaterEqual(len(result["total_ms"].values), len(bench.TYPED) * 3)
        self.assertGreater(result["bytes"], 0)

    def test_navigation_emits_less_than_a_repaint(self):
        navigation = bench.bench_navigation(self.items, self.session, SIZE, 24)
        repaint = bench.bench_repaint(self.items, self.session, SIZE, 5)
        self.assertLess(navigation["bytes"], repaint["bytes"])

    def test_an_unchanged_frame_costs_nothing(self):
        from herdr_newtab.paint import Painter

        app = bench.picker(self.items, self.session)
        painter = Painter()
        painter.frame(app.frame(*SIZE), *SIZE)
        self.assertEqual(painter.frame(app.frame(*SIZE), *SIZE), "")

    def test_frame_lookups_do_not_grow_with_the_catalog(self):
        # The invariants --check gates on, asserted here too so a change that
        # breaks one fails the suite rather than only the benchmark.
        small = bench.bench_calls(bench.catalog(50), self.session, SIZE)
        large = bench.bench_calls(bench.catalog(1600), self.session, SIZE)
        self.assertEqual(small, large)

    def test_frame_lookups_do_not_grow_with_open_panes(self):
        few = bench.bench_calls(self.items, bench.snapshot(2), SIZE)
        many = bench.bench_calls(self.items, bench.snapshot(32), SIZE)
        self.assertEqual(few, many)

    def test_returning_to_a_folder_costs_less_than_arriving(self):
        calls = bench.bench_calls(self.items, self.session, SIZE)
        self.assertLess(calls["nav_repeat"], calls["nav"])

    def test_call_counts_do_not_depend_on_the_order_they_are_taken_in(self):
        # The picker caches what it looks up, so a scenario that did not start
        # from a cleared cache would be paid for by whichever ran before it.
        first = bench.bench_calls(self.items, self.session, SIZE)
        second = bench.bench_calls(self.items, self.session, SIZE)
        self.assertEqual(first, second)


class ScoreTest(unittest.TestCase):
    METRIC = bench.Metric("x", "x", "ms", 2.0, 32.0, 1, True)

    def test_at_or_under_budget_is_full_marks(self):
        self.assertEqual(bench.score_of(2.0, self.METRIC), 100.0)
        self.assertEqual(bench.score_of(0.1, self.METRIC), 100.0)

    def test_at_or_over_the_limit_is_nothing(self):
        self.assertEqual(bench.score_of(32.0, self.METRIC), 0.0)
        self.assertEqual(bench.score_of(1000.0, self.METRIC), 0.0)

    def test_between_them_it_falls_monotonically(self):
        values = [bench.score_of(x, self.METRIC) for x in (3.0, 6.0, 12.0, 24.0)]
        self.assertEqual(values, sorted(values, reverse=True))

    def test_equal_ratios_cost_equal_score(self):
        # The curve is logarithmic on purpose: 2ms -> 4ms should be the same
        # amount of worse as 4ms -> 8ms, because that is how it is perceived.
        first = bench.score_of(4.0, self.METRIC) - bench.score_of(8.0, self.METRIC)
        second = bench.score_of(8.0, self.METRIC) - bench.score_of(16.0, self.METRIC)
        self.assertAlmostEqual(first, second, places=6)

    def test_weights_are_a_whole_hundred(self):
        self.assertEqual(sum(metric.weight for metric in bench.METRICS), 100)

    def test_every_metric_has_a_budget_under_its_limit(self):
        for metric in bench.METRICS:
            self.assertLess(metric.budget, metric.limit, metric.key)

    def test_ceilings_only_gate_machine_independent_metrics(self):
        fixed = {metric.key for metric in bench.METRICS if metric.fixed}
        self.assertTrue(set(bench.CEILINGS) <= fixed)

    def test_ceilings_are_no_tighter_than_the_budgets_they_guard(self):
        # A ceiling below its budget would fail a run the score calls perfect.
        for metric in bench.METRICS:
            if metric.key in bench.CEILINGS:
                self.assertGreaterEqual(bench.CEILINGS[metric.key], metric.budget, metric.key)

    def test_grades_run_from_best_to_worst(self):
        self.assertEqual(bench.grade_of(100.0), "A+")
        self.assertEqual(bench.grade_of(85.0), "B")
        self.assertEqual(bench.grade_of(0.0), "F")


class CollectTest(unittest.TestCase):
    def test_a_full_run_scores_every_metric(self):
        args = argparse.Namespace(
            size="80x18", catalog=40, panes=3, iterations=24, startup_runs=0
        )
        result = bench.collect(args, 0)
        for metric in bench.METRICS:
            self.assertIn(metric.key, result["values"], metric.key)
            self.assertIn(metric.key, result["scores"], metric.key)
        self.assertEqual(result["grade"], bench.grade_of(result["score"]))
        self.assertGreater(result["detail"]["repaint"]["bytes"], 0)


if __name__ == "__main__":
    unittest.main()
