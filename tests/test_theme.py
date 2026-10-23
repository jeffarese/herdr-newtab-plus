from __future__ import annotations

import unittest

from herdr_newtab.theme import (
    DEFAULTS,
    Theme,
    default_dim,
    default_ghost,
    default_muted,
    default_rule,
)

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

    def test_the_rules_sit_below_every_tier_of_text(self):
        dark, light = Theme(appearance="dark"), Theme(appearance="light")
        self.assertLess(level(dark, "rule"), level(dark, "ghost"))
        self.assertGreater(level(light, "rule"), level(light, "ghost"))
        self.assertIsNone(default_rule(None))


class AgentColourTest(unittest.TestCase):
    def test_an_agent_is_drawn_in_its_vendors_colour(self):
        theme = Theme()
        self.assertEqual(theme.agent_role("claude"), "agent_claude")
        self.assertEqual(theme.agent_role("Claude Code"), "agent_claude")
        self.assertEqual(theme.agent_role("cursor-agent"), "agent_cursor")
        self.assertIn("2;217;119;87", theme.fg("agent_claude"))

    def test_a_monochrome_mark_takes_the_text_colour(self):
        theme = Theme()
        self.assertEqual(theme.agent_role("pi"), "agent_pi")
        self.assertEqual(theme.fg("agent_pi"), "")

    def test_an_agent_without_a_colour_falls_back_to_the_agent_role(self):
        self.assertEqual(Theme().agent_role("droid"), "agent")
        self.assertEqual(Theme().agent_role(None), "agent")

    def test_the_config_can_recolour_an_agent(self):
        theme = Theme({"agent_codex": "#ff00ff"})
        self.assertIn("2;255;0;255", theme.fg(theme.agent_role("codex")))

    def test_logos_only_appear_once_they_are_switched_on(self):
        theme = Theme()
        self.assertEqual(theme.logo("claude"), "")
        theme.agent_icons = True
        self.assertEqual(theme.logo("claude"), "\ue1a0")
        self.assertEqual(theme.logo("droid"), "")


if __name__ == "__main__":
    unittest.main()
