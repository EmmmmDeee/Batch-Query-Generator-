//! Oathnet Batch Query Generator
//!
//! This library filters arbitrary text input into structured Oathnet batch queries.
//! It extracts meaningful tokens from the input, normalizes them, and emits
//! ready-to-submit `BatchQuery` records.

use std::fmt;
use uuid::Uuid;

/// A single Oathnet batch query.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchQuery {
    /// Unique identifier for this query.
    pub id: String,
    /// The normalized query string derived from the input text.
    pub query: String,
    /// Index of this query within the current batch (0-based).
    pub index: usize,
}

impl BatchQuery {
    /// Creates a new `BatchQuery` with a fresh UUID.
    pub fn new(query: impl Into<String>, index: usize) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            query: query.into(),
            index,
        }
    }
}

impl fmt::Display for BatchQuery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "BATCH_QUERY {{ id: \"{}\", index: {}, query: \"{}\" }}",
            self.id, self.index, self.query
        )
    }
}

/// Filters raw text into a list of Oathnet `BatchQuery` records.
///
/// The filtering pipeline:
/// 1. Split input on newlines and semicolons to get candidate query lines.
/// 2. Strip leading/trailing whitespace from each candidate.
/// 3. Discard blank lines and lines that consist entirely of punctuation/symbols.
/// 4. Collapse internal whitespace runs to a single space.
/// 5. Assign each surviving line a sequential index and a unique UUID.
pub fn generate_batch_queries(input: &str) -> Vec<BatchQuery> {
    input
        .split(|c| c == '\n' || c == ';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter(|s| s.chars().any(|c| c.is_alphanumeric()))
        .map(normalize_query)
        .enumerate()
        .map(|(idx, q)| BatchQuery::new(q, idx))
        .collect()
}

/// Normalizes a query string by collapsing internal whitespace runs and
/// removing characters that are not alphanumeric, whitespace, `-`, `_`, `.`,
/// `(`, `)`, `[`, `]`, `{`, `}`, `:`, `=`, `<`, `>`, `!`, `?`, `@`, `#`, or `*`.
pub fn normalize_query(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric()
                || c.is_whitespace()
                || matches!(
                    c,
                    '-' | '_'
                        | '.'
                        | '('
                        | ')'
                        | '['
                        | ']'
                        | '{'
                        | '}'
                        | ':'
                        | '='
                        | '<'
                        | '>'
                        | '!'
                        | '?'
                        | '@'
                        | '#'
                        | '*'
                )
            {
                c
            } else {
                ' '
            }
        })
        .collect();

    // Collapse runs of whitespace into a single space and trim.
    cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Serializes a slice of `BatchQuery` records into the Oathnet batch query
/// text format, one query per line.
pub fn format_batch(queries: &[BatchQuery]) -> String {
    queries
        .iter()
        .map(|q| q.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize_query ────────────────────────────────────────────────────────

    #[test]
    fn normalize_collapses_whitespace() {
        assert_eq!(normalize_query("  hello   world  "), "hello world");
    }

    #[test]
    fn normalize_removes_unwanted_chars() {
        // commas, quotes, ampersands are stripped
        let result = normalize_query("SELECT \"name\", age & 5");
        assert!(!result.contains('"'));
        assert!(!result.contains(','));
        assert!(!result.contains('&'));
    }

    #[test]
    fn normalize_keeps_allowed_special_chars() {
        let q = "filter age>=18 type:user";
        let result = normalize_query(q);
        assert!(result.contains(">="));
        assert!(result.contains(':'));
    }

    #[test]
    fn normalize_empty_string() {
        assert_eq!(normalize_query(""), "");
    }

    // ── generate_batch_queries ─────────────────────────────────────────────────

    #[test]
    fn generates_one_query_per_non_blank_line() {
        let input = "query one\nquery two\nquery three";
        let queries = generate_batch_queries(input);
        assert_eq!(queries.len(), 3);
    }

    #[test]
    fn splits_on_semicolons() {
        let input = "alpha; beta; gamma";
        let queries = generate_batch_queries(input);
        assert_eq!(queries.len(), 3);
    }

    #[test]
    fn blank_lines_are_discarded() {
        let input = "first\n\n\nsecond";
        let queries = generate_batch_queries(input);
        assert_eq!(queries.len(), 2);
    }

    #[test]
    fn pure_punctuation_lines_are_discarded() {
        let input = "valid query\n---\n!!!";
        let queries = generate_batch_queries(input);
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].query, "valid query");
    }

    #[test]
    fn indices_are_sequential() {
        let input = "a\nb\nc";
        let queries = generate_batch_queries(input);
        for (i, q) in queries.iter().enumerate() {
            assert_eq!(q.index, i);
        }
    }

    #[test]
    fn each_query_has_unique_id() {
        let input = "first\nsecond\nthird";
        let queries = generate_batch_queries(input);
        let ids: std::collections::HashSet<_> = queries.iter().map(|q| q.id.clone()).collect();
        assert_eq!(ids.len(), queries.len());
    }

    #[test]
    fn empty_input_yields_no_queries() {
        assert!(generate_batch_queries("").is_empty());
    }

    #[test]
    fn whitespace_only_input_yields_no_queries() {
        assert!(generate_batch_queries("   \n  \n  ").is_empty());
    }

    // ── format_batch ──────────────────────────────────────────────────────────

    #[test]
    fn format_batch_contains_id_and_query() {
        let queries = generate_batch_queries("hello oathnet");
        let output = format_batch(&queries);
        assert!(output.contains("BATCH_QUERY"));
        assert!(output.contains("hello oathnet"));
        assert!(output.contains("id:"));
        assert!(output.contains("index:"));
    }

    #[test]
    fn format_batch_empty_slice() {
        assert_eq!(format_batch(&[]), "");
    }

    // ── BatchQuery Display ────────────────────────────────────────────────────

    #[test]
    fn display_format_is_well_formed() {
        let q = BatchQuery {
            id: "test-id".to_string(),
            query: "find users".to_string(),
            index: 0,
        };
        let s = q.to_string();
        assert!(s.starts_with("BATCH_QUERY {"));
        assert!(s.contains("\"test-id\""));
        assert!(s.contains("\"find users\""));
    }
}
