//! Best-effort Termux/Android niceties. Every function is a no-op when the
//! `termux-*` helpers are not on PATH, so the same binary runs fine on a laptop.
//!
//! These are what make the tool feel native on a phone:
//!   * a wake lock so a long batch is not suspended by Doze mid-run,
//!   * a notification when the batch finishes,
//!   * opening the web UI in the device browser.

use std::path::Path;
use std::process::{Command, Stdio};

/// Is `name` an executable on PATH?
pub fn has(name: &str) -> bool {
    let path = match std::env::var_os("PATH") {
        Some(p) => p,
        None => return false,
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(name);
        candidate.is_file() && is_exec(&candidate)
    })
}

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_exec(_p: &Path) -> bool {
    true
}

fn run_quiet(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Acquire a wake lock for the duration of a batch (released by [`wake_unlock`]).
pub fn wake_lock() {
    if has("termux-wake-lock") {
        run_quiet("termux-wake-lock", &[]);
    }
}

pub fn wake_unlock() {
    if has("termux-wake-unlock") {
        run_quiet("termux-wake-unlock", &[]);
    }
}

/// Post an Android notification (no-op off Termux).
pub fn notify(title: &str, content: &str) {
    if has("termux-notification") {
        run_quiet(
            "termux-notification",
            &["--title", title, "--content", content],
        );
    }
}

/// Open `url` in the device browser (no-op off Termux).
pub fn open_url(url: &str) {
    if has("termux-open-url") {
        run_quiet("termux-open-url", &[url]);
    }
}

/// True when we appear to be inside Termux.
pub fn in_termux() -> bool {
    std::env::var_os("TERMUX_VERSION").is_some()
        || Path::new("/data/data/com.termux/files/usr").exists()
}
