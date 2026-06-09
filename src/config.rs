//! A tiny, std-only config reader (a small TOML subset).
//!
//! We do not pull in a TOML crate for a handful of defaults. Supported: `#`
//! comments, `[section]` headers, and `key = value` lines whose value is a
//! double-quoted string or a bare token (number/word). Keys are addressed as
//! `section.key`. Unknown lines are ignored rather than fatal — a config file
//! should never stop a batch from running.

use std::collections::HashMap;
use std::path::Path;

#[derive(Default)]
pub struct Config {
    map: HashMap<String, String>,
}

impl Config {
    /// Parse config text. Never fails: malformed lines are skipped.
    pub fn parse(text: &str) -> Config {
        let mut map = HashMap::new();
        let mut section = String::new();
        for raw in text.lines() {
            let line = strip_comment(raw).trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                section = name.trim().to_string();
                continue;
            }
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                if key.is_empty() {
                    continue;
                }
                let val = unquote(v.trim());
                let full = if section.is_empty() {
                    key.to_string()
                } else {
                    format!("{section}.{key}")
                };
                map.insert(full, val);
            }
        }
        Config { map }
    }

    /// Load from `path` if it exists; otherwise an empty config.
    pub fn load(path: impl AsRef<Path>) -> Config {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::parse(&text),
            Err(_) => Config::default(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }

    pub fn get_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.get(key).unwrap_or(default)
    }
}

fn strip_comment(s: &str) -> &str {
    // A '#' outside quotes starts a comment. Good enough for our flat values.
    let mut in_q = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_q = !in_q,
            '#' if !in_q => return &s[..i],
            _ => {}
        }
    }
    s
}

fn unquote(s: &str) -> String {
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_and_values() {
        let cfg = Config::parse(
            "# a comment\n\
             [serve]\n\
             host = \"0.0.0.0\"  # bind everywhere\n\
             port = 9000\n\
             \n\
             [run]\n\
             concurrency = 8\n\
             target = echo\n",
        );
        assert_eq!(cfg.get("serve.host"), Some("0.0.0.0"));
        assert_eq!(cfg.get("serve.port"), Some("9000"));
        assert_eq!(cfg.get("run.concurrency"), Some("8"));
        assert_eq!(cfg.get("run.target"), Some("echo"));
        assert_eq!(cfg.get("missing.key"), None);
        assert_eq!(cfg.get_or("missing.key", "x"), "x");
    }

    #[test]
    fn tolerates_garbage() {
        let cfg = Config::parse("this is not valid\n= nokey\n[unclosed\n");
        assert!(cfg.get("anything").is_none());
    }
}
