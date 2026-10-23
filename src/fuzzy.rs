//! Fuzzy matching.
//!
//! A compact take on the fzf scoring model: subsequence matching with bonuses
//! for word boundaries, camelCase transitions and runs of adjacent characters,
//! and penalties for the gaps between them. Query terms are space separated
//! and combined with AND, so "tri work" matches an item whose text contains
//! both. Positions are code point indices, as the Python release reported them.

use crate::pystr;
use std::cell::RefCell;

pub const SCORE_MATCH: i64 = 16;
pub const BONUS_BOUNDARY: i64 = 16;
pub const BONUS_CAMEL: i64 = 12;
pub const BONUS_CONSECUTIVE: i64 = 16;
pub const BONUS_FIRST_CHAR_MULTIPLIER: i64 = 2;
pub const PENALTY_GAP_START: i64 = -3;
pub const PENALTY_GAP_EXTENSION: i64 = -1;

const NEG: i64 = -1_000_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    pub score: i64,
    pub positions: Vec<usize>,
}

fn is_delimiter(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '/' | '\\' | '-' | '_' | '.' | ':' | ',' | ';' | '|' | '(' | ')' | '[' | ']' | '{' | '}' | '<'
            | '>' | '@' | '#' | '\'' | '"' | '~' | '+' | '*' | '=' | '&' | '%' | '!' | '?'
    )
}

/// Split a raw query into AND-ed terms, keeping quoted phrases together.
pub fn split_query(query: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in query.chars() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            continue;
        }
        if pystr::is_space(c) {
            if !current.is_empty() {
                terms.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(c);
    }
    if !current.is_empty() {
        terms.push(current);
    }
    terms
}

fn bonuses(text: &[char]) -> Vec<u8> {
    const BOUNDARY: u8 = BONUS_BOUNDARY as u8;
    const CAMEL: u8 = BONUS_CAMEL as u8;
    let mut out = Vec::with_capacity(text.len());
    for (index, &c) in text.iter().enumerate() {
        if index == 0 {
            out.push(BOUNDARY);
            continue;
        }
        let previous = text[index - 1];
        if is_delimiter(previous) {
            out.push(BOUNDARY);
        } else if (pystr::is_lower(previous) && pystr::is_upper(c)) || pystr::is_digit(previous) != pystr::is_digit(c) {
            out.push(CAMEL);
        } else {
            out.push(0);
        }
    }
    out
}

/// A field ready to be matched: its characters, folded characters and
/// per-character bonuses. Fields never change while the bar is open, so this
/// is worked out once per field rather than once per keystroke.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub chars: Vec<char>,
    /// The folded characters, or empty when folding changes nothing (most
    /// fields): `haystack` then answers with `chars`.
    folded: Vec<char>,
    bonus: Vec<u8>,
}

impl Prepared {
    pub fn new(text: &str) -> Prepared {
        let chars: Vec<char> = text.chars().collect();
        let mut folded: Vec<char> = if text.is_ascii() {
            if text.bytes().any(|b| b.is_ascii_uppercase()) {
                chars.iter().map(|c| c.to_ascii_lowercase()).collect()
            } else {
                Vec::new()
            }
        } else {
            let lowered = pystr::lower(text);
            let mut folded: Vec<char> = lowered.chars().collect();
            if folded.len() != chars.len() {
                // A handful of characters grow when they fold -- "İ" becomes an i
                // and a combining dot. Fold those one at a time, keeping the
                // leading one, so positions stay in step with the text.
                folded = chars.iter().map(|&c| pystr::lower_char(c).chars().next().unwrap_or(c)).collect();
            }
            folded
        };
        if folded == chars {
            folded = Vec::new();
        }
        let bonus = bonuses(&chars);
        Prepared { chars, folded, bonus }
    }

    fn haystack(&self, fold: bool) -> &[char] {
        if fold && !self.folded.is_empty() {
            &self.folded
        } else {
            &self.chars
        }
    }
}

/// A query term, folded once.
#[derive(Clone, Debug)]
pub struct Term {
    pub fold: bool,
    pub needle: Vec<char>,
}

impl Term {
    pub fn new(query: &str) -> Term {
        let fold = pystr::str_islower(query);
        let needle = if fold { pystr::lower(query).chars().collect() } else { query.chars().collect() };
        Term { fold, needle }
    }
}

fn find(haystack: &[char], c: char, from: usize) -> Option<usize> {
    if from >= haystack.len() {
        return None;
    }
    haystack[from..].iter().position(|&h| h == c).map(|p| p + from)
}

#[cfg(test)]
/// Score one term against one string, or None when it does not match.
pub fn match_text(query: &str, text: &str) -> Option<Match> {
    if query.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    if text.is_empty() {
        return None;
    }
    match_prepared(&Term::new(query), &Prepared::new(text))
}

pub fn match_prepared(term: &Term, field: &Prepared) -> Option<Match> {
    let needle = &term.needle;
    if needle.is_empty() {
        return Some(Match { score: 0, positions: Vec::new() });
    }
    if field.chars.is_empty() {
        return None;
    }
    let haystack = field.haystack(term.fold);
    let bonus = |index: usize| field.bonus[index] as i64;

    // Cheap rejection before the DP: every query character must appear in order.
    let mut cursor = 0;
    for &c in needle.iter() {
        cursor = find(haystack, c, cursor)? + 1;
    }

    let n = haystack.len();
    let m = needle.len();
    let low = find(haystack, needle[0], 0)?;
    let high = haystack.iter().rposition(|&h| h == needle[m - 1])?;

    // One score row and one parent row per needle character, laid end to end
    // in scratch buffers kept between calls: scoring a list is thousands of
    // these, and allocating 2m rows each time was most of the cost.
    SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let (scores, parents) = &mut *scratch;
        scores.clear();
        scores.resize(m * n, NEG);
        parents.clear();
        parents.resize(m * n, -1);

        let ceiling = high as isize - m as isize + 1;
        let mut at = Some(low);
        while let Some(a) = at {
            if (a as isize) > ceiling {
                break;
            }
            scores[a] = SCORE_MATCH + bonus(a) * BONUS_FIRST_CHAR_MULTIPLIER;
            at = find(haystack, needle[0], a + 1);
        }

        for j in 1..m {
            let qchar = needle[j];
            let (done, rest) = scores.split_at_mut(j * n);
            let previous = &done[(j - 1) * n..];
            let row = &mut rest[..n];
            let back = &mut parents[j * n..(j + 1) * n];
            let ceiling = high as isize - m as isize + 1 + j as isize;
            let mut carry = NEG;
            let mut carry_from: isize = -1;
            let mut carry_gap = 0;
            let mut landed = false;
            let mut i = low as isize + 1;
            while i <= ceiling {
                let iu = i as usize;
                if carry > NEG {
                    carry += if carry_gap == 0 { PENALTY_GAP_START } else { PENALTY_GAP_EXTENSION };
                    carry_gap += 1;
                }
                let adjacent = previous[iu - 1];
                if adjacent > NEG && adjacent >= carry {
                    carry = adjacent;
                    carry_from = i - 1;
                    carry_gap = 0;
                }
                if carry == NEG || haystack[iu] != qchar {
                    i += 1;
                    continue;
                }
                let mut gained = bonus(iu);
                if carry_gap == 0 && gained < BONUS_CONSECUTIVE {
                    gained = BONUS_CONSECUTIVE;
                }
                row[iu] = carry + SCORE_MATCH + gained;
                back[iu] = carry_from;
                landed = true;
                i += 1;
            }
            if !landed {
                return None;
            }
        }

        let last = &scores[(m - 1) * n..];
        let best = *last.iter().max()?;
        if best == NEG {
            return None;
        }
        let best_index = last.iter().position(|&s| s == best)?;
        let mut positions = Vec::with_capacity(m);
        let mut index = best_index as isize;
        for j in (0..m).rev() {
            positions.push(index as usize);
            index = parents[j * n + index as usize];
            if index < 0 {
                break;
            }
        }
        positions.reverse();
        Some(Match { score: last[best_index], positions })
    })
}

thread_local! {
    static SCRATCH: RefCell<(Vec<i64>, Vec<isize>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        assert!(match_text("abc", "xaxbxc").is_some());
        assert!(match_text("abd", "abc").is_none());
        assert_eq!(match_text("ws", "weekly sync").unwrap().positions, vec![0, 7]);
        assert_eq!(split_query("a 'b c' d"), vec!["a", "b c", "d"]);
    }

    // Ported from the Python release's tests/test_fuzzy.py.

    #[test]
    fn splits_on_whitespace() { assert_eq!(split_query("tri  week"), vec!["tri", "week"]); }

    #[test]
    fn keeps_quoted_phrases_together() { assert_eq!(split_query("\"week gen\" tri"), vec!["week gen", "tri"]); }

    #[test]
    fn empty_query_has_no_terms() { assert!(split_query("   ").is_empty()); }

    #[test]
    fn returns_none_when_characters_are_missing() { assert!(match_text("xyz", "week generation").is_none()); }

    #[test]
    fn returns_none_when_order_does_not_hold() { assert!(match_text("kew", "week").is_none()); }

    #[test]
    fn positions_point_at_the_matched_characters() {
        let text = "Redesign week loading";
        let found = match_text("week", text).unwrap();
        let chars: Vec<char> = text.chars().collect();
        assert_eq!(found.positions.iter().map(|&i| chars[i]).collect::<String>(), "week");
    }

    #[test]
    fn prefix_beats_a_later_match() {
        assert!(match_text("log", "logs").unwrap().score > match_text("log", "the catalog").unwrap().score);
    }

    #[test]
    fn word_boundary_beats_mid_word() {
        assert!(match_text("wg", "week gen").unwrap().score > match_text("wg", "weekgen").unwrap().score);
    }

    #[test]
    fn consecutive_beats_scattered() {
        assert!(match_text("week", "week").unwrap().score > match_text("week", "w e e k").unwrap().score);
    }

    #[test]
    fn smart_case_is_insensitive_for_lowercase_queries() { assert!(match_text("rc", "RC").is_some()); }

    #[test]
    fn smart_case_is_sensitive_once_the_query_has_uppercase() {
        assert!(match_text("RC", "rc").is_none());
        assert!(match_text("RC", "RC").is_some());
    }

    #[test]
    fn empty_query_matches_with_no_positions() {
        assert!(match_text("", "anything").unwrap().positions.is_empty());
    }

    #[test]
    fn single_character_query() { assert_eq!(match_text("k", "week").unwrap().positions, vec![3]); }
}

