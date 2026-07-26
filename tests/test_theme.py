from __future__ import annotations

import unittest

from herdr_newtab.theme import DEFAULTS, Theme, default_dim, default_ghost, default_muted

# The three tiers, brightest first. Everything the picker draws in words sits in
# one of them, so their order is the whole contract.
TIERS = ("muted", "dim", "ghost")


def level(theme, role):
    """The 256-colour index a role resolves to."""
    return int(theme.fg(role).rsplit(";", 1)[1].rstrip("m"))


class ReadabilityTest(unittest.TestCase):
    def test_no_tier_is_left_to_the_terminals_decoration_slot(self):
        # `bright_black` is the one ANSI slot themes treat as furniture: Tokyo
        # Night and its relatives set it to a near-background navy, which made
        # every label drawn in it unreadable.
        for role in TIERS:
            self.assertNotIn(DEFAULTS[role], ("bright_black", "black", "8", "0"), role)

    def test_the_tiers_stay_in_order_on_a_dark_background(self):
        theme = Theme(appearance="dark")
        self.assertGreater(level(theme, "muted"), level(theme, "dim"))
        self.assertGreater(level(theme, "dim"), level(theme, "ghost"))

    def test_and_the_other_way_round_on_a_light_one(self):
        # Fainter means lighter here, so the same order runs the other way.
        theme = Theme(appearance="light")
        self.assertLess(level(theme, "muted"), level(theme, "dim"))
        self.assertLess(level(theme, "dim"), level(theme, "ghost"))

    def test_an_unknown_background_keeps_the_neutral_defaults(self):
        theme = Theme(appearance=None)
        for role in TIERS:
            self.assertEqual(theme.fg(role), Theme().fg(role), role)
        self.assertIsNone(default_muted(None))
        self.assertIsNone(default_dim(None))
        self.assertIsNone(default_ghost(None))

    def test_the_config_still_wins(self):
        theme = Theme({"dim": "#ff0000"}, appearance="dark")
        self.assertIn("2;255;0;0", theme.fg("dim"))


if __name__ == "__main__":
    unittest.main()
