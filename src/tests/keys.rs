//! Ported from the Python release's tests/test_keys.py.
#![allow(unused_imports)]

use crate::keys::{Event, KeyDecoder};
use crate::testutil::*;

fn key_names(events: Vec<Event>) -> Vec<String> {
    events
        .into_iter()
        .filter_map(|event| match event {
            Event::Key(name) => Some(name),
            _ => None,
        })
        .collect()
}

/// The keys one decoder reads out of these chunks, fed in order.
fn keys(chunks: &[&[u8]]) -> Vec<String> {
    let mut decoder = KeyDecoder::new();
    chunks.iter().flat_map(|chunk| key_names(decoder.feed(chunk))).collect()
}

// F1 is the spelling of `?` that works with a path in the input.

#[test]
fn ss3_is_decoded() {
    assert_eq!(keys(&[b"\x1bOP"]), vec!["f1"]);
}

#[test]
fn the_csi_spelling_is_decoded_too() {
    // Terminals disagree about which form they send, and both are ordinary.
    assert_eq!(keys(&[b"\x1b[11~"]), vec!["f1"]);
}

#[test]
fn the_cursor_keys_still_come_through_ss3() {
    assert_eq!(keys(&[b"\x1bOA", b"\x1bOB"]), vec!["up", "down"]);
}

#[test]
fn a_split_sequence_is_held_until_it_is_whole() {
    assert_eq!(keys(&[b"\x1bO", b"P"]), vec!["f1"]);
}

#[test]
fn the_front_of_one_is_known_to_be_unfinished() {
    let mut decoder = KeyDecoder::new();
    assert!(decoder.feed(b"\x1bO").is_empty());
    assert!(decoder.pending_escape());
}

#[test]
fn alt_o_is_not_swallowed_by_the_wait() {
    let mut decoder = KeyDecoder::new();
    decoder.feed(b"\x1bO");
    // Python compared every event's value, whatever its kind.
    let values: Vec<String> = decoder
        .flush()
        .into_iter()
        .map(|event| match event {
            Event::Key(v) | Event::Text(v) | Event::Paste(v) => v,
            Event::Mouse { .. } => "<mouse>".to_string(),
        })
        .collect();
    assert_eq!(values, vec!["alt+O"]);
}
