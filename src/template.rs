//! A small, focused template engine: `{{ column }}` placeholders over CSV rows.
//!
//! Intentionally tiny. We resolve placeholders against a per-row lookup. We do
//! *not* invent a Turing-complete templating language; batch query generation
//! wants predictable substitution, validated up front, with clear errors when a
//! referenced column does not exist. Anything fancier is a footgun.

/// A compiled template: a flat list of literal and variable segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    segments: Vec<Segment>,
    vars: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Lit(String),
    Var(String),
}

impl Template {
    /// Parse a template string. `{{ name }}` is a variable; `{{{{` / `}}}}`
    /// emit literal braces. An unterminated `{{` is an error.
    pub fn parse(input: &str) -> Result<Template, String> {
        let bytes = input.as_bytes();
        let mut segments: Vec<Segment> = Vec::new();
        let mut vars: Vec<String> = Vec::new();
        let mut lit = String::new();
        let mut i = 0;
        while i < bytes.len() {
            // Escaped literal braces: {{{{ -> {{ , }}}} -> }}
            if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
                if i + 3 < bytes.len() && bytes[i + 2] == b'{' && bytes[i + 3] == b'{' {
                    lit.push_str("{{");
                    i += 4;
                    continue;
                }
                // Open a variable. Find the closing }}.
                let rest = &input[i + 2..];
                let end = match rest.find("}}") {
                    Some(e) => e,
                    None => {
                        return Err(format!(
                            "unterminated '{{{{' starting at byte {i}: missing closing '}}}}'"
                        ));
                    }
                };
                let name = rest[..end].trim();
                if name.is_empty() {
                    return Err(format!("empty placeholder '{{{{}}}}' at byte {i}"));
                }
                if name.contains('{') {
                    return Err(format!("nested '{{' inside placeholder near byte {i}"));
                }
                if !lit.is_empty() {
                    segments.push(Segment::Lit(std::mem::take(&mut lit)));
                }
                let name = name.to_string();
                if !vars.contains(&name) {
                    vars.push(name.clone());
                }
                segments.push(Segment::Var(name));
                i += 2 + end + 2;
                continue;
            }
            if bytes[i] == b'}' && i + 3 < bytes.len() && &input[i..i + 4] == "}}}}" {
                lit.push_str("}}");
                i += 4;
                continue;
            }
            // Default: copy one UTF-8 char.
            let ch = input[i..].chars().next().unwrap();
            lit.push(ch);
            i += ch.len_utf8();
        }
        if !lit.is_empty() {
            segments.push(Segment::Lit(lit));
        }
        Ok(Template { segments, vars })
    }

    /// The distinct variable names referenced, in first-seen order.
    pub fn variables(&self) -> &[String] {
        &self.vars
    }

    /// Render the template using `lookup` to resolve each variable. If `lookup`
    /// returns `None` for a referenced variable, rendering fails.
    pub fn render<'a, F>(&self, mut lookup: F) -> Result<String, String>
    where
        F: FnMut(&str) -> Option<&'a str>,
    {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                Segment::Lit(s) => out.push_str(s),
                Segment::Var(name) => match lookup(name) {
                    Some(v) => out.push_str(v),
                    None => return Err(format!("no value for column '{name}'")),
                },
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn render_map(t: &Template, m: &HashMap<&str, &str>) -> Result<String, String> {
        t.render(|k| m.get(k).copied())
    }

    #[test]
    fn plain_literal() {
        let t = Template::parse("hello world").unwrap();
        assert!(t.variables().is_empty());
        assert_eq!(render_map(&t, &HashMap::new()).unwrap(), "hello world");
    }

    #[test]
    fn simple_vars() {
        let t =
            Template::parse("SELECT * FROM t WHERE id = {{ id }} AND name = '{{name}}'").unwrap();
        assert_eq!(t.variables(), &["id".to_string(), "name".to_string()]);
        let mut m = HashMap::new();
        m.insert("id", "7");
        m.insert("name", "ada");
        assert_eq!(
            render_map(&t, &m).unwrap(),
            "SELECT * FROM t WHERE id = 7 AND name = 'ada'"
        );
    }

    #[test]
    fn missing_value_errors() {
        let t = Template::parse("{{a}}").unwrap();
        assert!(render_map(&t, &HashMap::new()).is_err());
    }

    #[test]
    fn unterminated_errors() {
        assert!(Template::parse("oops {{ id ").is_err());
    }

    #[test]
    fn empty_placeholder_errors() {
        assert!(Template::parse("x {{  }} y").is_err());
    }

    #[test]
    fn escaped_braces() {
        let t = Template::parse("{{{{literal}}}} and {{v}}").unwrap();
        assert_eq!(t.variables(), &["v".to_string()]);
        let mut m = HashMap::new();
        m.insert("v", "X");
        assert_eq!(render_map(&t, &m).unwrap(), "{{literal}} and X");
    }

    #[test]
    fn dedup_vars_first_seen_order() {
        let t = Template::parse("{{b}}{{a}}{{b}}").unwrap();
        assert_eq!(t.variables(), &["b".to_string(), "a".to_string()]);
    }
}
