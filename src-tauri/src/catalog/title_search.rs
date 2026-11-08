//! Search in the Movies and Shows lists: a filter on the list, not a ranking.
//!
//! A query matches a title when its text appears in the title's name at the
//! start of a word: the start of the name, or just after anything that is not
//! a letter or a digit. Case and accents do not count, and a run of spaces is
//! one space. So `EN -` finds "EN - A Movie" and "FR/EN - A Movie" (a slash
//! ends a word as a space does) but not "Garden - A Movie", and `amber lan`
//! finds "The Amber Lantern". `a|b` finds either, each side read the same way
//! ([`search::alternatives`]).
//!
//! It is an SQLite function, `title_match(name, query)`, so that a search is
//! one more condition on the list's own query: the list keeps its sort, its
//! years and its paging, and nothing is held in memory between searches.
//! Live channels keep the fuzzy index ([`crate::catalog::index`]), where
//! "northwindone" must find "NORTHWIND ONE".

use nucleo::chars;
use rusqlite::functions::FunctionFlags;
use rusqlite::types::ValueRef;
use rusqlite::Connection;

use crate::catalog::search;

/// What a title is searched by: its cleaned name, else the name as given.
/// `$alias` is the table's alias in the query (`m`, `s`).
macro_rules! searched_name_sql {
    ($alias:literal) => {
        concat!(
            "CASE WHEN ",
            $alias,
            ".clean_name <> '' THEN ",
            $alias,
            ".clean_name ELSE ",
            $alias,
            ".name END"
        )
    };
}
pub(crate) use searched_name_sql;

/// A parsed query: each alternative, folded as a name is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Needle {
    alternatives: Vec<Vec<char>>,
    /// The alternatives that are ASCII once folded, as bytes: the only ones
    /// a name in plain ASCII (most of them) can match, and matched without
    /// decoding it.
    ascii: Vec<Vec<u8>>,
}

impl Needle {
    /// `None` when there is nothing to look for, pipes or not.
    pub fn parse(query: &str) -> Option<Self> {
        let alternatives: Vec<Vec<char>> = search::alternatives(query)
            .into_iter()
            .map(|a| {
                let mut folded = Vec::new();
                fold_into(a, &mut folded);
                folded
            })
            .filter(|a| !a.is_empty())
            .collect();
        let ascii = alternatives
            .iter()
            .filter(|a| a.iter().all(char::is_ascii))
            .map(|a| a.iter().map(|&c| c as u8).collect())
            .collect();
        (!alternatives.is_empty()).then_some(Self {
            alternatives,
            ascii,
        })
    }

    /// This runs for every title in the catalogue on every page asked for,
    /// so a name is folded as it is read, never copied.
    pub fn matches(&self, name: &str) -> bool {
        if name.is_ascii() {
            let hay = name.as_bytes();
            return self.ascii.iter().any(|alt| ascii_match(hay, alt));
        }
        let mut after_word = false;
        for (i, c) in name.char_indices() {
            let Some(c) = fold_char(c) else { continue };
            let starts = !after_word && !c.is_whitespace();
            after_word = c.is_alphanumeric();
            if starts
                && self
                    .alternatives
                    .iter()
                    .any(|alt| starts_with(&name[i..], alt))
            {
                return true;
            }
        }
        false
    }
}

/// [`Needle::matches`] for an ASCII name and an ASCII alternative.
fn ascii_match(hay: &[u8], needle: &[u8]) -> bool {
    // Every byte of the needle takes at least one of the name's.
    let (Some(&first), true) = (needle.first(), needle.len() <= hay.len()) else {
        return false;
    };
    let mut after_word = false;
    for (i, &b) in hay.iter().enumerate() {
        let starts = !after_word;
        after_word = b.is_ascii_alphanumeric();
        if starts && b.to_ascii_lowercase() == first && ascii_starts_with(&hay[i..], needle) {
            return true;
        }
    }
    false
}

/// [`starts_with`], for ASCII.
fn ascii_starts_with(hay: &[u8], needle: &[u8]) -> bool {
    let space = |b: &u8| (*b as char).is_whitespace();
    let mut j = 0;
    for n in needle {
        if *n == b' ' {
            if !hay.get(j).is_some_and(space) {
                return false;
            }
            while hay.get(j).is_some_and(space) {
                j += 1;
            }
        } else {
            if hay.get(j).map(u8::to_ascii_lowercase) != Some(*n) {
                return false;
            }
            j += 1;
        }
    }
    true
}

/// Whether `query` matches `name`; a blank query matches everything, as a
/// search with nothing typed is no search.
pub fn matches(query: &str, name: &str) -> bool {
    Needle::parse(query).is_none_or(|n| n.matches(name))
}

/// Combining marks, which a decomposed accent ("e" then U+0301) is made of.
fn is_combining(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}'
        | '\u{1AB0}'..='\u{1AFF}'
        | '\u{1DC0}'..='\u{1DFF}'
        | '\u{20D0}'..='\u{20FF}'
        | '\u{FE20}'..='\u{FE2F}')
}

/// A character as it is compared: lower case and without its accent, or
/// `None` for a combining mark, which does not count at all.
fn fold_char(c: char) -> Option<char> {
    if c.is_ascii() {
        return Some(c.to_ascii_lowercase());
    }
    if is_combining(c) {
        return None;
    }
    let c = chars::normalize(c);
    Some(if c.is_ascii() {
        c.to_ascii_lowercase()
    } else {
        chars::to_lower_case(c)
    })
}

/// Lower case without accents, each run of whitespace one space, none at
/// either end: a query as [`starts_with`] compares it.
fn fold_into(s: &str, out: &mut Vec<char>) {
    out.clear();
    for c in s.chars().filter_map(fold_char) {
        if c.is_whitespace() {
            if out.last().is_some_and(|l| *l != ' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    if out.last() == Some(&' ') {
        out.pop();
    }
}

/// Whether `name` begins with the folded `needle`, folding as it goes; a
/// space in the needle stands for any run of whitespace.
fn starts_with(name: &str, needle: &[char]) -> bool {
    let mut hay = name.chars().filter_map(fold_char).peekable();
    for &n in needle {
        match hay.next() {
            Some(c) if n == ' ' && c.is_whitespace() => {
                while hay.next_if(|c| c.is_whitespace()).is_some() {}
            }
            Some(c) if c == n => {}
            _ => return false,
        }
    }
    true
}

/// Makes `title_match(name, query)` callable on `conn`. The query is parsed
/// once per statement (SQLite keeps it beside the statement), the name once
/// per row. A NULL or blank query matches every name, a NULL name none.
pub fn install(conn: &Connection) -> rusqlite::Result<()> {
    conn.create_scalar_function(
        "title_match",
        2,
        FunctionFlags::SQLITE_UTF8
            | FunctionFlags::SQLITE_DETERMINISTIC
            | FunctionFlags::SQLITE_INNOCUOUS,
        |ctx| {
            let needle = ctx.get_or_create_aux(1, |q| -> rusqlite::Result<Option<Needle>> {
                Ok(match q {
                    ValueRef::Text(t) => Needle::parse(&String::from_utf8_lossy(t)),
                    _ => None,
                })
            })?;
            let Some(needle) = needle.as_ref() else {
                return Ok(true);
            };
            Ok(match ctx.get_raw(0) {
                ValueRef::Text(t) => match std::str::from_utf8(t) {
                    Ok(name) => needle.matches(name),
                    Err(_) => needle.matches(&String::from_utf8_lossy(t)),
                },
                _ => false,
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_language_prefix_matches_only_where_a_word_starts() {
        assert!(matches("EN -", "EN - A Movie"));
        assert!(!matches("EN -", "Garden - A Movie"));
        assert!(!matches("EN -", "Harbour Lights"));
    }

    #[test]
    fn a_slash_ends_a_word() {
        assert!(matches("EN -", "FR/EN - A Movie"));
        assert!(matches("fr", "FR/EN - A Movie"));
    }

    #[test]
    fn a_phrase_matches_in_order_from_a_word_start() {
        assert!(matches("amber lan", "The Amber Lantern"));
        assert!(matches("the amber", "The Amber Lantern"));
        assert!(!matches("lantern amber", "The Amber Lantern"));
        assert!(!matches("mber", "The Amber Lantern"));
        assert!(!matches("he amber", "The Amber Lantern"));
    }

    #[test]
    fn case_does_not_count() {
        assert!(matches("en -", "EN - A Movie"));
        assert!(matches("HARBOUR", "Harbour Lights"));
    }

    #[test]
    fn accents_do_not_count() {
        assert!(matches("éte", "L'Été"));
        assert!(matches("ete", "L'Été"));
        assert!(matches("ÉTÉ", "l'ete"));
        // Decomposed: an "e" and a combining acute accent.
        assert!(matches("ete", "L'E\u{301}te\u{301}"));
        assert!(matches("crème", "Creme Lane"));
    }

    #[test]
    fn whitespace_runs_are_one_space() {
        assert!(matches("amber   lan", "The Amber Lantern"));
        assert!(matches("amber lan", "The  Amber \t Lantern"));
        assert!(matches("  harbour  ", "Harbour Lights"));
    }

    #[test]
    fn a_pipe_matches_either_side() {
        assert!(matches("harbour|lantern", "Harbour Lights"));
        assert!(matches("harbour|lantern", "The Lantern Bay"));
        assert!(!matches("harbour|lantern", "Garden Party"));
        assert!(matches("EN -|FR -", "FR - Le Film"));
        assert!(!matches("EN -|FR -", "Garden - X"));
    }

    #[test]
    fn empty_alternatives_are_ignored_and_a_blank_query_is_no_filter() {
        assert!(matches("|harbour|", "Harbour Lights"));
        assert!(!matches("|harbour|", "Garden Party"));
        assert!(Needle::parse("").is_none());
        assert!(Needle::parse(" | ").is_none());
        assert!(matches("", "Anything"));
        assert!(matches(" | ", "Anything"));
    }

    #[test]
    fn a_query_longer_than_the_name_does_not_match() {
        assert!(!matches("harbour lights at night", "Harbour Lights"));
        assert!(!matches("x", ""));
    }

    /// The rule read plainly: fold both, then look at every word start.
    fn by_the_rule(query: &str, name: &str) -> bool {
        let mut hay = Vec::new();
        fold_into(name, &mut hay);
        let Some(needle) = Needle::parse(query) else {
            return true;
        };
        needle.alternatives.iter().any(|alt| {
            hay.len() >= alt.len()
                && (0..=hay.len() - alt.len()).any(|i| {
                    (i == 0 || !hay[i - 1].is_alphanumeric()) && hay[i..i + alt.len()] == **alt
                })
        })
    }

    #[test]
    fn the_quick_paths_agree_with_the_rule() {
        let names = [
            "EN - A Movie",
            "FR/EN - A Movie",
            "Garden - X",
            "The Amber  Lantern",
            "  Harbour\tLights ",
            "L'Été",
            "L'E\u{301}te\u{301} - EN",
            "Seven - Kestrel",
            "a",
            "",
            "EN-",
            "en  -  x",
        ];
        let queries = [
            "EN -",
            "en",
            "-",
            "e",
            "ete",
            "été",
            "amber lan",
            "harbour lights",
            "a|x",
            "n -",
            "kestrel",
            "en - x",
            "l'e",
            "'",
        ];
        for name in names {
            for query in queries {
                assert_eq!(
                    matches(query, name),
                    by_the_rule(query, name),
                    "{query:?} in {name:?}"
                );
            }
        }
    }

    #[test]
    fn the_cleaned_name_keeps_a_language_prefix() {
        assert_eq!(crate::catalog::clean_name("EN - A Movie"), "EN - A Movie");
        assert_eq!(
            crate::catalog::clean_name("FR/EN - A Movie [4K]"),
            "FR/EN - A Movie"
        );
    }

    #[test]
    fn the_sql_function_filters_rows() {
        let c = Connection::open_in_memory().unwrap();
        install(&c).unwrap();
        let hit = |name: Option<&str>, q: Option<&str>| -> bool {
            c.query_row("SELECT title_match(?1, ?2)", (name, q), |r| r.get(0))
                .unwrap()
        };
        assert!(hit(Some("EN - A Movie"), Some("EN -")));
        assert!(!hit(Some("Garden - X"), Some("EN -")));
        assert!(hit(Some("Garden - X"), Some("")));
        assert!(hit(Some("Garden - X"), None));
        assert!(!hit(None, Some("EN -")));
    }
}
