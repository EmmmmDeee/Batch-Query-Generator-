//! Where generated queries go. A `Target` is the pluggable backend.
//!
//! Two ship in the box:
//!
//! * [`Echo`] — offline, zero-network. The "query" is the result. The default,
//!   and ideal for dry runs and demos on a device with no connectivity.
//! * [`Http`] — POST/GET the rendered query to a plain `http://` endpoint via std
//!   `TcpStream`. No TLS (that is the documented upgrade path to keep us
//!   dependency-free); ideal for localhost services and internal APIs.
//!
//! Adding a target is implementing one trait. That is the whole extensibility
//! story, on purpose.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// The result of executing one query.
pub struct Outcome {
    pub ok: bool,
    /// True only when `ok == false` and a retry might succeed (timeouts, 5xx,
    /// connection resets).
    pub retryable: bool,
    /// Short human-readable detail (status code, error, echoed query head).
    pub info: String,
}

impl Outcome {
    pub fn ok(info: impl Into<String>) -> Outcome {
        Outcome {
            ok: true,
            retryable: false,
            info: info.into(),
        }
    }
    pub fn err(retryable: bool, info: impl Into<String>) -> Outcome {
        Outcome {
            ok: false,
            retryable,
            info: info.into(),
        }
    }
}

pub trait Target: Send + Sync {
    fn execute(&self, query: &str) -> Outcome;
    fn name(&self) -> &str;
}

/// Offline target: echoes the query back as the result.
pub struct Echo;

impl Target for Echo {
    fn execute(&self, query: &str) -> Outcome {
        let head: String = query.chars().take(160).collect();
        Outcome::ok(head)
    }
    fn name(&self) -> &str {
        "echo"
    }
}

/// Plain-HTTP target. Sends the rendered query as the request (body for POST,
/// appended to the path as a query string for GET).
pub struct Http {
    host: String,
    port: u16,
    path: String,
    method: String,
    content_type: String,
    timeout: Duration,
}

impl Http {
    /// Parse `http://host[:port]/path`. Returns an error for anything else
    /// (notably `https://`, which would need TLS).
    pub fn new(
        url: &str,
        method: &str,
        content_type: &str,
        timeout_secs: u64,
    ) -> Result<Http, String> {
        let rest = url.strip_prefix("http://").ok_or_else(|| {
            if url.starts_with("https://") {
                "https:// is not supported by the built-in http target (no TLS in the \
                     zero-dependency build); use a localhost/plain-http endpoint or add a \
                     TLS-capable target"
                    .to_string()
            } else {
                format!("url must start with http:// : {url}")
            }
        })?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((h, p)) => (
                h.to_string(),
                p.parse::<u16>().map_err(|_| format!("bad port in {url}"))?,
            ),
            None => (authority.to_string(), 80),
        };
        if host.is_empty() {
            return Err(format!("empty host in {url}"));
        }
        Ok(Http {
            host,
            port,
            path: path.to_string(),
            method: method.to_uppercase(),
            content_type: content_type.to_string(),
            timeout: Duration::from_secs(timeout_secs.max(1)),
        })
    }

    fn request(&self, query: &str) -> std::io::Result<u16> {
        let mut stream = TcpStream::connect((self.host.as_str(), self.port))?;
        stream.set_read_timeout(Some(self.timeout))?;
        stream.set_write_timeout(Some(self.timeout))?;

        let (request_target, body) = if self.method == "GET" {
            let sep = if self.path.contains('?') { '&' } else { '?' };
            (
                format!("{}{}q={}", self.path, sep, urlencode(query)),
                String::new(),
            )
        } else {
            (self.path.clone(), query.to_string())
        };

        let mut req = format!(
            "{} {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: bqg/0.1\r\nConnection: close\r\n",
            self.method, request_target, self.host
        );
        if self.method != "GET" {
            req.push_str(&format!(
                "Content-Type: {}\r\nContent-Length: {}\r\n",
                self.content_type,
                body.len()
            ));
        }
        req.push_str("\r\n");
        req.push_str(&body);

        stream.write_all(req.as_bytes())?;
        stream.flush()?;

        // Read just enough to get the status line.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while head.len() < 64 {
            match stream.read(&mut byte)? {
                0 => break,
                _ => {
                    head.push(byte[0]);
                    if byte[0] == b'\n' {
                        break;
                    }
                }
            }
        }
        let line = String::from_utf8_lossy(&head);
        // "HTTP/1.1 200 OK"
        let code = line
            .split_whitespace()
            .nth(1)
            .and_then(|c| c.parse::<u16>().ok())
            .unwrap_or(0);
        Ok(code)
    }
}

impl Target for Http {
    fn execute(&self, query: &str) -> Outcome {
        match self.request(query) {
            Ok(code) if (200..300).contains(&code) => Outcome::ok(format!("HTTP {code}")),
            Ok(code) if (500..600).contains(&code) || code == 0 => {
                Outcome::err(true, format!("HTTP {code}"))
            }
            Ok(code) => Outcome::err(false, format!("HTTP {code}")),
            Err(e) => Outcome::err(true, format!("connection error: {e}")),
        }
    }
    fn name(&self) -> &str {
        "http"
    }
}

/// Minimal percent-encoding for GET query strings.
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_is_ok() {
        let o = Echo.execute("SELECT 1");
        assert!(o.ok);
        assert_eq!(o.info, "SELECT 1");
    }

    #[test]
    fn http_rejects_https() {
        assert!(Http::new("https://x/y", "POST", "text/plain", 5).is_err());
    }

    #[test]
    fn http_parses_authority() {
        let h = Http::new("http://example.com:8080/api", "POST", "application/json", 5).unwrap();
        assert_eq!(h.host, "example.com");
        assert_eq!(h.port, 8080);
        assert_eq!(h.path, "/api");
    }

    #[test]
    fn http_default_port_and_path() {
        let h = Http::new("http://localhost", "GET", "text/plain", 5).unwrap();
        assert_eq!(h.port, 80);
        assert_eq!(h.path, "/");
    }

    #[test]
    fn urlencode_basic() {
        assert_eq!(urlencode("a b&c"), "a%20b%26c");
    }
}
