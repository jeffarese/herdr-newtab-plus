from __future__ import annotations

import unittest

from herdr_newtab.keys import KEY, KeyDecoder


def keys(*chunks):
    decoder = KeyDecoder()
    out = []
    for chunk in chunks:
        out.extend(event.value for event in decoder.feed(chunk) if event.kind == KEY)
    return out


class FunctionKeyTest(unittest.TestCase):
    """F1 is the spelling of `?` that works with a path in the input."""

    def test_ss3_is_decoded(self):
        self.assertEqual(keys(b"\x1bOP"), ["f1"])

    def test_the_csi_spelling_is_decoded_too(self):
        # Terminals disagree about which form they send, and both are ordinary.
        self.assertEqual(keys(b"\x1b[11~"), ["f1"])

    def test_the_cursor_keys_still_come_through_ss3(self):
        self.assertEqual(keys(b"\x1bOA", b"\x1bOB"), ["up", "down"])

    def test_a_split_sequence_is_held_until_it_is_whole(self):
        self.assertEqual(keys(b"\x1bO", b"P"), ["f1"])

    def test_the_front_of_one_is_known_to_be_unfinished(self):
        decoder = KeyDecoder()
        self.assertEqual(decoder.feed(b"\x1bO"), [])
        self.assertTrue(decoder.pending_escape())

    def test_alt_o_is_not_swallowed_by_the_wait(self):
        decoder = KeyDecoder()
        decoder.feed(b"\x1bO")
        self.assertEqual([event.value for event in decoder.flush()], ["alt+O"])


if __name__ == "__main__":
    unittest.main()
