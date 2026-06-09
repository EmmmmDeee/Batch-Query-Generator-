//! Durable, append-only checkpoint journal — the feature that makes long batches
//! survive Android.
//!
//! On a phone the OS will suspend or kill a long-running process without warning
//! (Doze, the low-memory killer, the user swiping the app away). So a batch is a
//! resumable job, not a fire-and-forget loop. Every completed row is appended to
//! a journal on disk; on restart we load the set of already-finished rows and
//! skip them. A 100k-row batch that dies at 80k resumes at 80k.
//!
//! Format is line-oriented TSV — trivial to parse without a JSON reader and easy
//! to eyeball: `<row>\t<ok|err>\t<info>` (info has tabs/newlines flattened).

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct Journal {
    path: PathBuf,
    file: Mutex<File>,
    /// row -> succeeded? Loaded at open from existing contents, then kept current
    /// as rows are recorded in-session.
    seen: Mutex<HashMap<u64, bool>>,
}

impl Journal {
    /// Open (creating if needed) the journal at `path`, loading prior progress.
    pub fn open(path: impl AsRef<Path>) -> io::Result<Journal> {
        let path = path.as_ref().to_path_buf();
        let mut seen = HashMap::new();
        if path.exists() {
            let f = File::open(&path)?;
            for line in BufReader::new(f).lines() {
                let line = line?;
                let mut it = line.splitn(3, '\t');
                if let (Some(row), Some(status)) = (it.next(), it.next()) {
                    if let Ok(row) = row.parse::<u64>() {
                        seen.insert(row, status == "ok");
                    }
                }
            }
        }
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Journal {
            path,
            file: Mutex::new(file),
            seen: Mutex::new(seen),
        })
    }

    /// A row is "done" (skippable on resume) if it succeeded. Rows that errored
    /// are retried on the next run.
    pub fn is_done(&self, row: u64) -> bool {
        matches!(self.seen.lock().unwrap().get(&row), Some(true))
    }

    /// Number of rows recorded as succeeded.
    pub fn completed(&self) -> usize {
        self.seen.lock().unwrap().values().filter(|ok| **ok).count()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a completed row. Thread-safe.
    pub fn record(&self, row: u64, ok: bool, info: &str) -> io::Result<()> {
        let flat: String = info
            .chars()
            .map(|c| {
                if c == '\t' || c == '\n' || c == '\r' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        let status = if ok { "ok" } else { "err" };
        {
            let mut f = self.file.lock().unwrap();
            writeln!(f, "{row}\t{status}\t{flat}")?;
            f.flush()?;
        }
        self.seen.lock().unwrap().insert(row, ok);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("bqg_test_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn records_and_resumes() {
        let p = tmp("resume");
        {
            let j = Journal::open(&p).unwrap();
            j.record(0, true, "ok").unwrap();
            j.record(1, false, "boom\twith\ttabs").unwrap();
            j.record(2, true, "fine").unwrap();
            assert!(j.is_done(0));
            assert!(!j.is_done(1)); // errored rows are retried
            assert!(j.is_done(2));
            assert_eq!(j.completed(), 2);
        }
        // Reopen: prior progress is visible.
        let j2 = Journal::open(&p).unwrap();
        assert!(j2.is_done(0));
        assert!(!j2.is_done(1));
        assert!(j2.is_done(2));
        assert_eq!(j2.completed(), 2);
        std::fs::remove_file(&p).unwrap();
    }
}
