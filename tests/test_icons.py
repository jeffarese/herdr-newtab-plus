from __future__ import annotations

import os
import shutil
import tempfile
import unittest
from unittest import mock

from herdr_newtab import icons
from herdr_newtab.config import Config


class LogoTest(unittest.TestCase):
    def test_the_codepoints_match_herdr_radar(self):
        # herdr-bar and herdr-radar read the same table, so a terminal mapped
        # for either draws these.
        self.assertEqual(icons.logo_for("claude"), "")
        self.assertEqual(icons.logo_for("codex"), "")

    def test_the_names_an_agent_goes_by_find_the_same_logo(self):
        self.assertEqual(icons.logo_for("Claude Code"), icons.logo_for("claude"))
        self.assertEqual(icons.logo_for("cursor-agent"), icons.logo_for("cursor"))
        self.assertEqual(icons.logo_for("open-code"), icons.logo_for("opencode"))
        self.assertEqual(icons.logo_for("qoder"), icons.logo_for("qodercli"))
        self.assertEqual(icons.logo_for("codex:gpt-5"), icons.logo_for("codex"))

    def test_an_agent_the_font_does_not_know_has_no_logo(self):
        self.assertIsNone(icons.logo_for("droid"))
        self.assertIsNone(icons.logo_for(""))
        self.assertIsNone(icons.logo_for(None))

    def test_every_logo_is_recognised_as_one(self):
        for vendor in icons.VENDORS:
            self.assertTrue(icons.is_logo(icons.logo_for(vendor)), vendor)
        self.assertFalse(icons.is_logo("a"))
        self.assertFalse(icons.is_logo(chr(icons.FIRST + len(icons.VENDORS))))


class DetectionTest(unittest.TestCase):
    def setUp(self):
        icons.clear_cache()
        self.fonts = tempfile.mkdtemp(prefix="newtab-fonts-")
        self.addCleanup(shutil.rmtree, self.fonts, True)
        self.addCleanup(icons.clear_cache)
        patcher = mock.patch.object(icons, "_font_dirs", lambda: [self.fonts])
        patcher.start()
        self.addCleanup(patcher.stop)
        # fontconfig is the fallback on Linux; keep this machine's fonts out of it.
        no_fc = mock.patch.object(icons.subprocess, "run", side_effect=OSError)
        no_fc.start()
        self.addCleanup(no_fc.stop)

    def test_the_font_file_is_enough(self):
        self.assertFalse(icons.detect())
        open(os.path.join(self.fonts, "HerdrAgentIconsMax-7526f2a2.ttf"), "w").close()
        self.assertTrue(icons.detect())

    def test_another_font_is_not_mistaken_for_it(self):
        open(os.path.join(self.fonts, "HerdrAgentIcons.otf"), "w").close()
        self.assertFalse(icons.detect())

    def test_auto_follows_the_font_and_the_answer_is_kept(self):
        self.assertFalse(icons.enabled("auto"))
        open(os.path.join(self.fonts, "HerdrAgentIconsMax-Regular.ttf"), "w").close()
        self.assertFalse(icons.enabled("auto"))  # asked once per popup
        icons.clear_cache()
        self.assertTrue(icons.enabled("auto"))

    def test_font_and_none_do_not_look(self):
        with mock.patch.object(icons, "detect", side_effect=AssertionError):
            self.assertTrue(icons.enabled("font"))
            self.assertFalse(icons.enabled("none"))
            self.assertFalse(icons.enabled("nonsense"))

    def test_a_prefetch_gives_the_same_answer(self):
        open(os.path.join(self.fonts, "HerdrAgentIconsMax-Regular.ttf"), "w").close()
        icons.prefetch()
        self.assertTrue(icons.font_available())


class ConfigTest(unittest.TestCase):
    def test_it_defaults_to_auto(self):
        self.assertEqual(Config().agent_icons, "auto")

    def test_booleans_mean_always_and_never(self):
        self.assertEqual(Config({"agent_icons": True}).agent_icons, "font")
        self.assertEqual(Config({"agent_icons": False}).agent_icons, "none")

    def test_anything_else_falls_back_to_auto(self):
        self.assertEqual(Config({"agent_icons": "sometimes"}).agent_icons, "auto")
        self.assertEqual(Config({"agent_icons": "none"}).agent_icons, "none")


if __name__ == "__main__":
    unittest.main()
