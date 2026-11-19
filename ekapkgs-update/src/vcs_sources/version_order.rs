//! Total order on version strings ([`compare_version_components`]).
//!
//! A version is split into tokens at `.`, `-`, `_`, `+`, `~` and at every
//! digit/letter boundary (`1.0.0rc1` → `1 0 0 rc 1`). Numbers compare
//! numerically, so `1.10 > 1.9` and dates (`2024-01-15`) order as expected.
//!
//! Prerelease words rank below the release they precede, separated or not
//! (`1.0-rc1`, `1.0rc1`, `1.0.0-beta.2`, PEP 440 `1.0a1`/`1.0b2`/`1.0.dev1`):
//! `dev < alpha (a<n>) < beta (b<n>) < pre/preview < rc < <end of version>`.
//! Any other word ranks above the end of a version but below a number, so
//! `1.1.1 < 1.1.1w`, `1.2.3 < 1.2.3-unstable-2024-01-01`, `1.0.post1 > 1.0`,
//! and `0-unstable-2024-02-01 < 0.1` (a word against a number: the number
//! wins).
//!
//! Trailing zeros of each run of numbers are not significant
//! (`1.0 == 1.0.0`, `1.0rc1 == 1.0.0rc1`). Every version maps to one token
//! sequence and sequences are compared lexicographically, so this is a total
//! order (a valid `sort_by` comparator).

use std::cmp::Ordering;

/// One version token; the derived order is the ranking described in the
/// module docs (`Pre < End < Word < Num`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Token<'a> {
    /// A prerelease marker with its rank (`dev` 0 … `rc` 4).
    Pre(u8),
    /// The end of the version (only used as padding when comparing).
    End,
    /// Any other word, compared lowercase.
    Word(String),
    /// A number: digits without leading zeros, ordered by length then text.
    Num(Num<'a>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Num<'a>(&'a str);

impl Ord for Num<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .len()
            .cmp(&other.0.len())
            .then_with(|| self.0.cmp(other.0))
    }
}

impl PartialOrd for Num<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Prerelease rank of a word, if it is one. `a`/`b` only count when a number
/// follows directly (`1.0a1`), so a letter release (`1.1.1a`) stays a word.
fn prerelease_rank(word: &str, digit_follows: bool) -> Option<u8> {
    match word {
        "dev" => Some(0),
        "alpha" => Some(1),
        "a" if digit_follows => Some(1),
        "beta" => Some(2),
        "b" if digit_follows => Some(2),
        "pre" | "preview" => Some(3),
        "rc" => Some(4),
        _ => None,
    }
}

fn tokenize(version: &str) -> Vec<Token<'_>> {
    let bytes = version.as_bytes();
    let mut raw: Vec<Token<'_>> = Vec::new();
    let mut i = 0;
    // A leading `v`/`V` directly before a digit is a tag prefix, not a word.
    if bytes.len() > 1 && matches!(bytes[0], b'v' | b'V') && bytes[1].is_ascii_digit() {
        i = 1;
    }
    while i < bytes.len() {
        let start = i;
        if bytes[i].is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let digits = version[start..i].trim_start_matches('0');
            raw.push(Token::Num(Num(digits)));
        } else if bytes[i].is_ascii_alphabetic() {
            while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                i += 1;
            }
            let word = version[start..i].to_ascii_lowercase();
            let digit_follows = bytes.get(i).is_some_and(u8::is_ascii_digit);
            raw.push(match prerelease_rank(&word, digit_follows) {
                Some(rank) => Token::Pre(rank),
                None => Token::Word(word),
            });
        } else {
            // Separators (and any other punctuation) only delimit tokens.
            i += 1;
        }
    }

    // Drop trailing zeros of every run of numbers.
    let mut tokens = Vec::with_capacity(raw.len());
    let mut iter = raw.into_iter().peekable();
    while let Some(token) = iter.next() {
        if !matches!(token, Token::Num(_)) {
            tokens.push(token);
            continue;
        }
        let mut run = vec![token];
        while let Some(Token::Num(_)) = iter.peek() {
            run.extend(iter.next());
        }
        while run.len() > 1 && matches!(run.last(), Some(Token::Num(Num("")))) {
            run.pop();
        }
        // A lone zero is kept: `0-unstable-…` starts with a number.
        tokens.extend(run);
    }
    tokens
}

/// Compare two version strings (see the module docs for the order).
///
/// This is the single version comparison used for release selection, watch
/// dispatch and "the tree has reached this version"
/// ([`crate::database::version_at_least`]).
pub fn compare_version_components(a: &str, b: &str) -> Ordering {
    let ta = tokenize(a);
    let tb = tokenize(b);
    for i in 0..ta.len().max(tb.len()) {
        let x = ta.get(i).unwrap_or(&Token::End);
        let y = tb.get(i).unwrap_or(&Token::End);
        match x.cmp(y) {
            Ordering::Equal => {},
            ord => return ord,
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering_cases() {
        use Ordering::{Equal, Greater, Less};
        // (a, b, a cmp b)
        let cases = [
            ("1.2.3", "1.2.4", Less),
            ("1.2.3", "1.2.3", Equal),
            ("6.9.10", "6.9.7", Greater),
            ("1.10", "1.9", Greater),
            ("1.12.1", "1.9.6", Greater),
            ("2.0", "1.9.5", Greater),
            ("1.2.3", "1.2.3.1", Less),
            ("1.0", "1.0.0", Equal),
            ("v1.2", "1.2", Equal),
            // Prereleases rank below the release.
            ("1.0.0rc1", "1.0.0", Less),
            ("1.0.0-rc1", "1.0.0", Less),
            ("1.0rc1", "1.0.0", Less),
            ("1.0.0rc1", "1.0.0rc2", Less),
            ("1.0.0-rc.2", "1.0.0-rc.10", Less),
            ("1.3.0", "1.3.0-rc1", Greater),
            ("1.0a1", "1.0b1", Less),
            ("1.0b2", "1.0rc1", Less),
            ("1.0.dev1", "1.0a1", Less),
            ("1.0.0-alpha", "1.0.0-alpha.1", Less),
            ("1.0.0-alpha.1", "1.0.0-beta", Less),
            ("6.0.0-Alpha9", "6.0.0", Less),
            ("1.0.0-preview.3", "1.0.0-rc1", Less),
            ("1.0rc1", "1.0.1", Less),
            ("3.0.0rc1", "3.0.0", Less),
            // Words other than prerelease markers rank above the end.
            ("1.1.1", "1.1.1w", Less),
            ("1.0", "1.0.post1", Less),
            ("1.2.3", "1.2.3-unstable-2024-01-01", Less),
            // Dates and unstable versions.
            ("2024-01-01", "2023-12-31", Greater),
            ("2024.01.15", "2024.1.9", Greater),
            ("20240115", "20231231", Greater),
            ("0-unstable-2024-02-01", "0-unstable-2024-01-15", Greater),
            ("0-unstable-2024-02-01", "0.1", Less),
            ("0-unstable-2024-02-01", "0", Greater),
            // Big numbers and leading zeros.
            ("1.99999999999999999999", "1.100000000000000000000", Less),
            ("1.010", "1.10", Equal),
            ("abd", "abc", Greater),
        ];
        for (a, b, expected) in cases {
            assert_eq!(compare_version_components(a, b), expected, "{a} vs {b}");
            assert_eq!(
                compare_version_components(b, a),
                expected.reverse(),
                "{b} vs {a}"
            );
        }
    }

    #[test]
    fn total_order() {
        let versions = [
            "1.0",
            "1.0.0",
            "1.0a",
            "1.0.0a",
            "1.0rc1",
            "1.0-rc2",
            "1.0.1",
            "1.0.post1",
            "1.1.1w",
            "0-unstable-2024-01-01",
            "0.1",
            "2024.01.15",
            "20240115",
            "latest",
            "v2",
            "1.0.dev1",
            "1.0a1",
        ];
        // Transitivity and antisymmetry over every triple.
        for a in versions {
            for b in versions {
                assert_eq!(
                    compare_version_components(a, b),
                    compare_version_components(b, a).reverse()
                );
                for c in versions {
                    if compare_version_components(a, b).is_le()
                        && compare_version_components(b, c).is_le()
                    {
                        assert!(
                            compare_version_components(a, c).is_le(),
                            "{a} <= {b} <= {c}"
                        );
                    }
                }
            }
        }
    }
}
