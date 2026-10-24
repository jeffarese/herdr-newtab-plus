//! A fast, non-cryptographic hash for maps keyed by paths.
//!
//! The standard library's SipHash defends against keys chosen to collide,
//! which a map of the user's own folders has no need of, and it is the
//! larger part of what the catalog costs per row. This is the multiply and
//! rotate hash rustc uses (FxHash), eight bytes at a time.

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasherDefault, Hasher};

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Clone, Copy, Default)]
pub struct FxHasher {
    hash: u64,
}

impl FxHasher {
    #[inline]
    fn add(&mut self, word: u64) {
        self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
    }
}

impl Hasher for FxHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (chunks, rest) = bytes.as_chunks::<8>();
        for chunk in chunks {
            self.add(u64::from_le_bytes(*chunk));
        }
        if !rest.is_empty() {
            let mut word = [0u8; 8];
            word[..rest.len()].copy_from_slice(rest);
            self.add(u64::from_le_bytes(word));
        }
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.add(value as u64);
    }

    #[inline]
    fn write_usize(&mut self, value: usize) {
        self.add(value as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.hash
    }
}

pub type FxBuild = BuildHasherDefault<FxHasher>;
pub type FxHashMap<K, V> = HashMap<K, V, FxBuild>;
pub type FxHashSet<K> = HashSet<K, FxBuild>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_keys_hash_equal_and_lookups_work() {
        let mut map: FxHashMap<String, usize> = FxHashMap::default();
        for index in 0..2000 {
            map.insert(format!("/home/me/work/project-{index:04}"), index);
        }
        assert_eq!(map.len(), 2000);
        for index in 0..2000 {
            assert_eq!(map.get(&format!("/home/me/work/project-{index:04}")), Some(&index));
        }
        assert_eq!(map.get("/home/me/work/project-2000"), None);
        // Keys that differ only past the first eight bytes, or by length.
        let mut short: FxHashMap<&str, u8> = FxHashMap::default();
        for key in ["", "a", "a\0", "abcdefgh", "abcdefghi", "abcdefgh\0"] {
            short.insert(key, 0);
        }
        assert_eq!(short.len(), 6);
    }
}
