//! fzf-style fuzzy matching over channel names.
//!
//! Deliberately not SQLite FTS5: full-text search does token and prefix
//! matching, which cannot satisfy a picker where "northwindone" should find
//! "NORTHWIND ONE". Tens of thousands of names is a few MB held resident.

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::Matcher;

pub struct SearchIndex {
    entries: Vec<(i64, String)>,
    matcher: std::cell::RefCell<Matcher>,
}

impl SearchIndex {
    pub fn new(entries: Vec<(i64, String)>) -> Self {
        Self {
            entries,
            matcher: std::cell::RefCell::new(Matcher::new(nucleo::Config::DEFAULT)),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns ids ranked best-first, at most `limit`.
    pub fn query(&self, needle: &str, limit: usize) -> Vec<i64> {
        if needle.trim().is_empty() {
            return self.entries.iter().take(limit).map(|(id, _)| *id).collect();
        }
        let pattern = Pattern::parse(needle, CaseMatching::Ignore, Normalization::Smart);
        let mut m = self.matcher.borrow_mut();
        let mut scored: Vec<(u32, i64)> = self
            .entries
            .iter()
            .filter_map(|(id, name)| {
                let mut buf = Vec::new();
                let hay = nucleo::Utf32Str::new(name, &mut buf);
                pattern.score(hay, &mut m).map(|s| (s, *id))
            })
            .collect();
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored.into_iter().take(limit).map(|(_, id)| id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx() -> SearchIndex {
        SearchIndex::new(vec![
            (1, "CA: PRISM 1".into()),
            (2, "CA: PRISM 2".into()),
            (3, "UK: NORTHWIND ONE LONDON".into()),
            (4, "US: HARBOUR+ EVENTS".into()),
            (5, "CA: CHRONICLE".into()),
        ])
    }

    #[test]
    fn exact_substring_matches() {
        assert_eq!(idx().query("PRISM", 10).len(), 2);
    }

    #[test]
    fn subsequence_matches_like_fzf() {
        // "northwindone" is not a substring of "UK: NORTHWIND ONE LONDON" (there is a space)
        let hits = idx().query("northwindone", 10);
        assert!(hits.contains(&3), "expected NORTHWIND ONE, got {hits:?}");
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(idx().query("prism", 10).contains(&1));
    }

    #[test]
    fn limit_is_respected() {
        assert_eq!(idx().query("CA", 1).len(), 1);
    }

    #[test]
    fn no_match_returns_empty() {
        assert!(idx().query("zzzzzz", 10).is_empty());
    }
}
