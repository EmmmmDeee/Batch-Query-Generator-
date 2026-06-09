//! The batch executor: a bounded thread pool fed by a channel.
//!
//! Threads + channels, not async. This mirrors how BurntSushi's tools (ripgrep,
//! the `ignore` crate) get parallelism: OS threads doing real work, coordinated
//! by channels, with bounded concurrency so we never melt a phone's CPU or blow
//! its memory. The producer streams rows in; workers execute; one writer thread
//! serializes results, the checkpoint journal, and progress.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::checkpoint::Journal;
use crate::csv::Reader;
use crate::json;
use crate::target::Target;
use crate::template::Template;

/// Live, lock-free counters shared with whatever is watching (CLI line, web UI).
#[derive(Default)]
pub struct Progress {
    pub total_seen: AtomicU64,
    pub ok: AtomicU64,
    pub err: AtomicU64,
    pub skipped: AtomicU64,
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn done(&self) -> u64 {
        self.ok.load(Ordering::Relaxed)
            + self.err.load(Ordering::Relaxed)
            + self.skipped.load(Ordering::Relaxed)
    }
    pub fn request_cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A per-row event, surfaced live (web UI tail, verbose CLI).
#[derive(Clone)]
pub struct Event {
    pub row: u64,
    pub ok: bool,
    pub skipped: bool,
    pub info: String,
}

/// Sink for per-row events. The CLI uses a no-op or progress printer; the web UI
/// stores a rolling tail for SSE.
pub trait Observer: Send + Sync {
    fn on_event(&self, ev: &Event);
}

pub struct NoopObserver;
impl Observer for NoopObserver {
    fn on_event(&self, _ev: &Event) {}
}

pub struct ExecConfig {
    pub concurrency: usize,
    pub retries: u32,
    pub backoff: Duration,
    pub limit: Option<u64>,
}

impl Default for ExecConfig {
    fn default() -> Self {
        ExecConfig {
            concurrency: 4,
            retries: 2,
            backoff: Duration::from_millis(250),
            limit: None,
        }
    }
}

struct Job {
    row: u64,
    query: String,
}

struct Done {
    row: u64,
    ok: bool,
    info: String,
    query: String,
}

/// Run a batch end to end. Generic over the input reader so the same code serves
/// a file (CLI) or an in-memory buffer (web paste).
#[allow(clippy::too_many_arguments)]
pub fn run_batch<R: Read>(
    mut reader: Reader<R>,
    template: &Template,
    target: Arc<dyn Target>,
    journal: Arc<Journal>,
    progress: Arc<Progress>,
    observer: Arc<dyn Observer>,
    mut out: Box<dyn Write + Send>,
    cfg: &ExecConfig,
) -> Result<(), String> {
    // 1. Header + up-front validation: every referenced column must exist.
    let headers = reader
        .headers()
        .map_err(|e| format!("reading header: {e}"))?;
    if headers.is_empty() {
        return Err("input has no header row".to_string());
    }
    let col_index: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.clone(), i))
        .collect();
    let missing: Vec<&String> = template
        .variables()
        .iter()
        .filter(|v| !col_index.contains_key(*v))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "template references columns not in CSV header {:?}: {:?}",
            headers, missing
        ));
    }

    // 2. Wire up channels and threads.
    let (job_tx, job_rx) = mpsc::channel::<Job>();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let (done_tx, done_rx) = mpsc::channel::<Done>();

    // Writer thread: serializes results out, journal, progress, events.
    let writer = {
        let journal = Arc::clone(&journal);
        let progress = Arc::clone(&progress);
        let observer = Arc::clone(&observer);
        thread::spawn(move || -> std::io::Result<()> {
            for d in done_rx.iter() {
                let line = format!(
                    "{{\"row\":{},\"ok\":{},\"info\":{},\"query\":{}}}\n",
                    d.row,
                    d.ok,
                    json::quote(&d.info),
                    json::quote(&d.query),
                );
                out.write_all(line.as_bytes())?;
                journal.record(d.row, d.ok, &d.info).ok();
                if d.ok {
                    progress.ok.fetch_add(1, Ordering::Relaxed);
                } else {
                    progress.err.fetch_add(1, Ordering::Relaxed);
                }
                observer.on_event(&Event {
                    row: d.row,
                    ok: d.ok,
                    skipped: false,
                    info: d.info,
                });
            }
            out.flush()
        })
    };

    // Worker threads.
    let mut workers = Vec::new();
    for _ in 0..cfg.concurrency.max(1) {
        let job_rx = Arc::clone(&job_rx);
        let done_tx = done_tx.clone();
        let target = Arc::clone(&target);
        let progress = Arc::clone(&progress);
        let retries = cfg.retries;
        let backoff = cfg.backoff;
        workers.push(thread::spawn(move || {
            loop {
                let job = {
                    let rx = job_rx.lock().unwrap();
                    rx.recv()
                };
                let job = match job {
                    Ok(j) => j,
                    Err(_) => break, // channel closed, drained
                };
                if progress.cancelled() {
                    break;
                }
                let mut attempt = 0u32;
                let outcome = loop {
                    let o = target.execute(&job.query);
                    if o.ok || !o.retryable || attempt >= retries || progress.cancelled() {
                        break o;
                    }
                    attempt += 1;
                    let wait = backoff * (1u32 << (attempt - 1).min(6));
                    thread::sleep(wait);
                };
                let _ = done_tx.send(Done {
                    row: job.row,
                    ok: outcome.ok,
                    info: outcome.info,
                    query: job.query,
                });
            }
        }));
    }
    drop(done_tx); // writer ends once producer + workers drop their senders

    // 3. Producer (this thread): stream rows, resume-skip, render, dispatch.
    //
    // Skips and render-errors never reach a worker, so the producer reports them
    // straight to progress + journal + observer here (no result line is written
    // for a row that produced no query).
    let mut row: u64 = 0;
    let mut producer_err: Option<String> = None;
    loop {
        if let Some(limit) = cfg.limit {
            if row >= limit {
                break;
            }
        }
        if progress.cancelled() {
            break;
        }
        let rec = match reader.next_record() {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(e) => {
                producer_err = Some(format!("reading row {row}: {e}"));
                break;
            }
        };
        let this_row = row;
        row += 1;
        progress.total_seen.fetch_add(1, Ordering::Relaxed);

        if journal.is_done(this_row) {
            progress.skipped.fetch_add(1, Ordering::Relaxed);
            observer.on_event(&Event {
                row: this_row,
                ok: true,
                skipped: true,
                info: "skipped (already completed)".to_string(),
            });
            continue;
        }

        let render = template.render(|name| {
            col_index
                .get(name)
                .map(|i| rec.get(*i).map_or("", |s| s.as_str()))
        });
        match render {
            Ok(query) => {
                if job_tx
                    .send(Job {
                        row: this_row,
                        query,
                    })
                    .is_err()
                {
                    break;
                }
            }
            Err(e) => {
                // Render failures are terminal for that row; record as error.
                progress.err.fetch_add(1, Ordering::Relaxed);
                journal.record(this_row, false, &e).ok();
                observer.on_event(&Event {
                    row: this_row,
                    ok: false,
                    skipped: false,
                    info: e,
                });
            }
        }
    }

    drop(job_tx); // signal workers no more jobs
    for w in workers {
        let _ = w.join();
    }
    // Workers have all dropped their done_tx clones now; writer will finish.
    writer
        .join()
        .map_err(|_| "writer thread panicked".to_string())?
        .map_err(|e| format!("writing results: {e}"))?;

    match producer_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target::Echo;
    use std::io::Cursor;

    fn journal_path(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("bqg_exec_{}_{}", std::process::id(), name));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn runs_and_resumes() {
        let csv = "id,name\n1,a\n2,b\n3,c\n";
        let tmpl = Template::parse("SELECT {{id}} -- {{name}}").unwrap();
        let jp = journal_path("run");

        // First run: all three rows execute.
        {
            let reader = Reader::new(Cursor::new(csv.as_bytes().to_vec()), b',');
            let journal = Arc::new(Journal::open(&jp).unwrap());
            let progress = Arc::new(Progress::default());
            let out: Box<dyn Write + Send> = Box::new(Vec::new());
            run_batch(
                reader,
                &tmpl,
                Arc::new(Echo),
                Arc::clone(&journal),
                Arc::clone(&progress),
                Arc::new(NoopObserver),
                out,
                &ExecConfig {
                    concurrency: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(progress.ok.load(Ordering::Relaxed), 3);
            assert_eq!(progress.skipped.load(Ordering::Relaxed), 0);
        }

        // Second run on the same journal: every row is skipped (resume).
        {
            let reader = Reader::new(Cursor::new(csv.as_bytes().to_vec()), b',');
            let journal = Arc::new(Journal::open(&jp).unwrap());
            let progress = Arc::new(Progress::default());
            let out: Box<dyn Write + Send> = Box::new(Vec::new());
            run_batch(
                reader,
                &tmpl,
                Arc::new(Echo),
                Arc::clone(&journal),
                Arc::clone(&progress),
                Arc::new(NoopObserver),
                out,
                &ExecConfig::default(),
            )
            .unwrap();
            assert_eq!(progress.skipped.load(Ordering::Relaxed), 3);
            assert_eq!(progress.ok.load(Ordering::Relaxed), 0);
        }
        std::fs::remove_file(&jp).unwrap();
    }

    #[test]
    fn missing_column_is_rejected() {
        let csv = "id\n1\n";
        let tmpl = Template::parse("{{nope}}").unwrap();
        let jp = journal_path("missing");
        let reader = Reader::new(Cursor::new(csv.as_bytes().to_vec()), b',');
        let journal = Arc::new(Journal::open(&jp).unwrap());
        let res = run_batch(
            reader,
            &tmpl,
            Arc::new(Echo),
            journal,
            Arc::new(Progress::default()),
            Arc::new(NoopObserver),
            Box::new(Vec::new()),
            &ExecConfig::default(),
        );
        assert!(res.is_err());
        let _ = std::fs::remove_file(&jp);
    }
}
