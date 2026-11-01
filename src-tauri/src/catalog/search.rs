//! fzf-style fuzzy matching over channel names.
//!
//! Deliberately not SQLite FTS5: full-text search does token and prefix
//! matching, which cannot satisfy a picker where "northwindone" should find
//! "NORTHWIND ONE". Tens of thousands of names is a few MB held resident.
//!
//! A query can offer alternatives, separated by `|`: `news|weather` finds
//! either. The pipe binds loosest, so each alternative is a whole query of
//! its own and its words must all match: `news|weather uk` is "news", or
//! "weather" and "uk" together. That is the reading of someone listing the
//! things they would take — and it lets an alternative be narrowed on its
//! own, which the other precedence cannot say at all. Programme search reads
//! the same syntax ([`alternatives`] serves both).

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::Matcher;

/// The alternatives in a query: split on `|`, trimmed, empty ones dropped.
/// Empty when there is nothing to look for, pipes or not.
pub fn alternatives(input: &str) -> Vec<&str> {
    input
        .split('|')
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .collect()
}

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
    ///
    /// A name matching several alternatives ranks by its best one: the
    /// alternatives are separate questions, and a name answering two of them
    /// is no better an answer to either.
    pub fn query(&self, needle: &str, limit: usize) -> Vec<i64> {
        let alts = alternatives(needle);
        if alts.is_empty() {
            return self.entries.iter().take(limit).map(|(id, _)| *id).collect();
        }
        let patterns: Vec<Pattern> = alts
            .iter()
            .map(|a| Pattern::parse(a, CaseMatching::Ignore, Normalization::Smart))
            .collect();
        let mut m = self.matcher.borrow_mut();
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, i64)> = self
            .entries
            .iter()
            .filter_map(|(id, name)| {
                let hay = nucleo::Utf32Str::new(name, &mut buf);
                patterns
                    .iter()
                    .filter_map(|p| p.score(hay, &mut m))
                    .max()
                    .map(|s| (s, *id))
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

    fn sorted(mut v: Vec<i64>) -> Vec<i64> {
        v.sort_unstable();
        v
    }

    #[test]
    fn a_pipe_matches_either_side() {
        assert_eq!(sorted(idx().query("chronicle|harbour", 10)), vec![4, 5]);
        // Spaces around the pipe are not part of either side.
        assert_eq!(sorted(idx().query("chronicle | harbour", 10)), vec![4, 5]);
    }

    #[test]
    fn words_within_an_alternative_must_all_match() {
        // "prism 2", or "harbour": not every PRISM.
        assert_eq!(sorted(idx().query("harbour|prism 2", 10)), vec![2, 4]);
        // "uk" alone would find nothing to add; with "prism" it is one
        // alternative needing both, which nothing satisfies.
        assert_eq!(idx().query("zzz|prism uk", 10), Vec::<i64>::new());
    }

    #[test]
    fn a_name_matching_both_sides_appears_once() {
        assert_eq!(idx().query("prism 1|ca prism 1", 10), vec![1]);
    }

    #[test]
    fn empty_alternatives_are_ignored() {
        assert_eq!(idx().query("|chronicle|", 10), vec![5]);
        assert_eq!(idx().query(" | ", 10).len(), 5, "only pipes is no query");
    }

    #[test]
    fn alternatives_split_and_trim() {
        assert_eq!(alternatives("news|weather"), vec!["news", "weather"]);
        assert_eq!(alternatives(" news | weather uk |"), vec!["news", "weather uk"]);
        assert!(alternatives("||").is_empty());
    }
}
