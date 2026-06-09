//! A small streaming CSV reader (RFC 4180 subset), std only.
//!
//! Streaming is a principle here, not a nicety: on a phone we may be feeding a
//! batch of hundreds of thousands of rows through a few hundred MB of RAM, so we
//! parse one record at a time and never hold the whole file in memory.
//!
//! Supported: configurable delimiter, double-quoted fields, `""` escapes,
//! embedded delimiters/newlines inside quotes, and LF or CRLF line endings.
//! Bytes are decoded lossily to UTF-8 so messy real-world data does not abort a
//! run. (The production-grade upgrade is BurntSushi's `csv` crate; this keeps us
//! dependency-free.)

use std::io::{self, BufReader, Read};

pub struct Reader<R: Read> {
    inner: BufReader<R>,
    delimiter: u8,
    buf: [u8; 1],
    eof: bool,
}

impl<R: Read> Reader<R> {
    pub fn new(inner: R, delimiter: u8) -> Reader<R> {
        Reader {
            inner: BufReader::new(inner),
            delimiter,
            buf: [0u8; 1],
            eof: false,
        }
    }

    fn next_byte(&mut self) -> io::Result<Option<u8>> {
        if self.eof {
            return Ok(None);
        }
        match self.inner.read(&mut self.buf)? {
            0 => {
                self.eof = true;
                Ok(None)
            }
            _ => Ok(Some(self.buf[0])),
        }
    }

    /// Read the next record as a vector of fields. Returns `Ok(None)` at EOF.
    pub fn next_record(&mut self) -> io::Result<Option<Vec<String>>> {
        let mut fields: Vec<Vec<u8>> = Vec::new();
        let mut field: Vec<u8> = Vec::new();
        let mut in_quotes = false;
        let mut started = false; // saw any byte for this record?
        let mut field_started = false;

        loop {
            let b = match self.next_byte()? {
                Some(b) => b,
                None => {
                    // EOF: flush a final record if we accumulated anything.
                    if started || field_started || !field.is_empty() || !fields.is_empty() {
                        fields.push(field);
                        return Ok(Some(decode(fields)));
                    }
                    return Ok(None);
                }
            };
            started = true;

            if in_quotes {
                if b == b'"' {
                    // Could be an escaped quote ("") or the closing quote.
                    match self.next_byte()? {
                        Some(b'"') => field.push(b'"'),
                        Some(next) => {
                            in_quotes = false;
                            // Reprocess `next` as an unquoted byte.
                            if next == self.delimiter {
                                fields.push(std::mem::take(&mut field));
                                field_started = false;
                            } else if next == b'\n' {
                                fields.push(field);
                                return Ok(Some(decode(fields)));
                            } else if next == b'\r' {
                                // swallow; expect \n next (handled below at top)
                                if let Some(after) = self.next_byte()? {
                                    if after == b'\n' {
                                        fields.push(field);
                                        return Ok(Some(decode(fields)));
                                    } else {
                                        // lone CR; push field break? Treat CR as record end.
                                        fields.push(field);
                                        // 'after' belongs to next record; rare lone-CR case,
                                        // we drop it for simplicity.
                                        let _ = after;
                                        return Ok(Some(decode(fields)));
                                    }
                                } else {
                                    fields.push(field);
                                    return Ok(Some(decode(fields)));
                                }
                            } else {
                                field.push(next);
                            }
                        }
                        None => {
                            // EOF right after closing quote.
                            fields.push(field);
                            return Ok(Some(decode(fields)));
                        }
                    }
                } else {
                    field.push(b);
                }
                continue;
            }

            // Not in quotes.
            if b == b'"' && !field_started {
                in_quotes = true;
                field_started = true;
            } else if b == self.delimiter {
                fields.push(std::mem::take(&mut field));
                field_started = false;
            } else if b == b'\n' {
                fields.push(field);
                return Ok(Some(decode(fields)));
            } else if b == b'\r' {
                // Expect \n.
                match self.next_byte()? {
                    Some(b'\n') | None => {
                        fields.push(field);
                        return Ok(Some(decode(fields)));
                    }
                    Some(_other) => {
                        // Lone CR; treat as record terminator, drop the stray byte.
                        fields.push(field);
                        return Ok(Some(decode(fields)));
                    }
                }
            } else {
                field.push(b);
                field_started = true;
            }
        }
    }

    /// Convenience: read the header record.
    pub fn headers(&mut self) -> io::Result<Vec<String>> {
        Ok(self.next_record()?.unwrap_or_default())
    }
}

fn decode(fields: Vec<Vec<u8>>) -> Vec<String> {
    fields
        .into_iter()
        .map(|f| String::from_utf8_lossy(&f).into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse_all(data: &str) -> Vec<Vec<String>> {
        let mut r = Reader::new(Cursor::new(data.as_bytes().to_vec()), b',');
        let mut out = Vec::new();
        while let Some(rec) = r.next_record().unwrap() {
            out.push(rec);
        }
        out
    }

    #[test]
    fn simple_rows() {
        let recs = parse_all("a,b,c\n1,2,3\n4,5,6\n");
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[1], vec!["1", "2", "3"]);
    }

    #[test]
    fn no_trailing_newline() {
        let recs = parse_all("a,b\n1,2");
        assert_eq!(recs, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn crlf() {
        let recs = parse_all("a,b\r\n1,2\r\n");
        assert_eq!(recs, vec![vec!["a", "b"], vec!["1", "2"]]);
    }

    #[test]
    fn quoted_fields() {
        let recs = parse_all("name,note\n\"Ada, L.\",\"hi\"\"there\"\n");
        assert_eq!(recs[1], vec!["Ada, L.", "hi\"there"]);
    }

    #[test]
    fn quoted_newline() {
        let recs = parse_all("a\n\"line1\nline2\"\n");
        assert_eq!(recs, vec![vec!["a"], vec!["line1\nline2"]]);
    }

    #[test]
    fn empty_fields() {
        let recs = parse_all("a,b,c\n1,,3\n");
        assert_eq!(recs[1], vec!["1", "", "3"]);
    }
}
