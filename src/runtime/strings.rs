//! Code-point positions in strings (INTERNALS 5.4).
//!
//! User-visible string positions are code-point indexes. An 8-slot round-robin cache of
//! `CharIndex` keeps `s[i]`, `len(s)`, slices and `index_of` from being O(n) per call.

use super::value::{Str, WeakStr};

/// Strings shorter than this are counted directly, without an index.
const SHORT: usize = 64;
/// One mark every this many chars.
const STEP: usize = 64;

struct CharIndex {
    /// The indexed string; the `Weak` keeps the allocation, so its address can't be reused, and
    /// it stops `s += piece` from growing the string in place, which would make the entry wrong.
    s: WeakStr,
    ascii: bool,
    chars: usize,
    /// Byte offsets of chars 0, 64, 128, …
    marks: Vec<usize>,
}

#[derive(Default)]
pub struct CharCache {
    slots: [Option<CharIndex>; 8],
    next: usize,
}

impl CharCache {
    fn index(&mut self, s: &Str) -> &CharIndex {
        // A `Weak` keeps the whole allocation, so an entry for a string no one holds any more
        // goes now, and its memory with it.
        for slot in &mut self.slots {
            if slot.as_ref().is_some_and(|ix| !ix.s.alive()) {
                *slot = None;
            }
        }
        let ptr = s.addr();
        if let Some(i) = self.slots.iter().position(|slot| matches!(slot, Some(ix) if ix.s.addr() == ptr)) {
            return self.slots[i].as_ref().expect("found above");
        }
        let ascii = s.is_ascii();
        let (chars, marks) = if ascii {
            (s.len(), Vec::new())
        } else {
            let mut marks = Vec::with_capacity(s.len() / STEP + 1);
            let mut n = 0;
            for (b, _) in s.char_indices() {
                if n % STEP == 0 {
                    marks.push(b);
                }
                n += 1;
            }
            (n, marks)
        };
        let i = self.next;
        self.next = (self.next + 1) % self.slots.len();
        self.slots[i] = Some(CharIndex { s: s.downgrade(), ascii, chars, marks });
        self.slots[i].as_ref().expect("just stored")
    }

    /// The number of chars.
    pub fn len(&mut self, s: &Str) -> usize {
        if s.len() < SHORT {
            return s.chars().count();
        }
        self.index(s).chars
    }

    /// The byte offset of char `i` (`i >= len` gives `s.len()`).
    pub fn byte_of(&mut self, s: &Str, i: usize) -> usize {
        if s.len() < SHORT {
            return s.char_indices().nth(i).map_or(s.len(), |(b, _)| b);
        }
        let ix = self.index(s);
        if i >= ix.chars {
            return s.len();
        }
        if ix.ascii {
            return i;
        }
        let start = ix.marks[i / STEP];
        s[start..].char_indices().nth(i % STEP).map_or(s.len(), |(b, _)| start + b)
    }

    /// The char index at byte offset `b` (a char boundary).
    pub fn char_of(&mut self, s: &Str, b: usize) -> usize {
        if s.len() < SHORT {
            return s[..b].chars().count();
        }
        let ix = self.index(s);
        if ix.ascii {
            return b;
        }
        let k = ix.marks.partition_point(|&m| m <= b) - 1;
        k * STEP + s[ix.marks[k]..b].chars().count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_string_doesnt_stay_in_the_cache() {
        let mut cache = CharCache::default();
        let big: Str = "x".repeat(1000).into();
        assert_eq!(cache.len(&big), 1000);
        drop(big);
        let other: Str = "y".repeat(1000).into();
        cache.len(&other);
        // Each entry left points at a live string; the dropped one's allocation went with its entry.
        assert!(cache.slots.iter().flatten().all(|ix| ix.s.alive()));
        assert_eq!(cache.slots.iter().flatten().count(), 1);
    }

    #[test]
    fn positions_match_a_plain_char_walk() {
        let mut cache = CharCache::default();
        for text in ["", "héllo", &"aé€😀".repeat(100), &"x".repeat(300), &format!("{}é", "a".repeat(200))] {
            let s: Str = text.into();
            let chars: Vec<(usize, char)> = text.char_indices().collect();
            assert_eq!(cache.len(&s), chars.len());
            for (i, (b, _)) in chars.iter().enumerate() {
                assert_eq!(cache.byte_of(&s, i), *b, "{text:?} char {i}");
                assert_eq!(cache.char_of(&s, *b), i, "{text:?} byte {b}");
            }
            assert_eq!(cache.byte_of(&s, chars.len()), text.len());
            assert_eq!(cache.char_of(&s, text.len()), chars.len());
        }
    }

    #[test]
    fn the_cache_tells_strings_apart() {
        let mut cache = CharCache::default();
        let a: Str = "é".repeat(100).into();
        let b: Str = "aé".repeat(100).into();
        assert_eq!((cache.len(&a), cache.len(&b)), (100, 200));
        for k in 0..20 {
            let s: Str = "€".repeat(70 + k).into();
            assert_eq!(cache.len(&s), 70 + k);
        }
        assert_eq!(cache.byte_of(&a, 3), 6);
    }
}
