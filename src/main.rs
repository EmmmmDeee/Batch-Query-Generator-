//! `bqg` — Batch Query Generator.
//!
//! A thin CLI over the `bqg` library. Three subcommands:
//!   * `gen`   — render queries from a CSV + template and print them (dry run).
//!   * `run`   — execute the batch against a target, with checkpoint/resume.
//!   * `serve` — start the local web UI.
//!
//! Argument parsing is hand-rolled and dependency-free, on purpose: it keeps the
//! binary a single self-contained artifact and the build instant in Termux.

use std::io::{self, Write};
use std::process::ExitCode;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use bqg::android;
use bqg::checkpoint::Journal;
use bqg::csv::Reader;
use bqg::exec::{self, ExecConfig, NoopObserver, Observer, Progress};
use bqg::server;
use bqg::target::{Echo, Http, Target};
use bqg::template::Template;

const USAGE: &str = "\
bqg — Batch Query Generator

USAGE:
    bqg <command> [options]

COMMANDS:
    gen      Render queries from a CSV + template and print them (dry run)
    run      Execute a batch against a target, with checkpoint/resume
    serve    Start the local web UI
    help     Show this help

Run `bqg <command> --help` for command-specific options.
";

const RUN_USAGE: &str = "\
bqg run — execute a batch against a target

USAGE:
    bqg run -t <template> [options] [input.csv]

    Reads CSV from <input.csv> or stdin. Writes JSONL results to stdout or -o.

OPTIONS:
    -t, --template <STR>     Query template with {{column}} placeholders
        --template-file <P>  Read the template from a file
        --target <NAME>      echo (default) | http
        --url <URL>          http:// endpoint (target=http)
        --method <M>         HTTP method (default POST)
        --content-type <C>   Content-Type for POST bodies (default text/plain)
    -o, --output <FILE>      Write JSONL results here (default: stdout)
    -c, --checkpoint <FILE>  Journal for resume (default: <input>.journal)
    -j, --concurrency <N>    Worker threads (default 4)
        --retries <N>        Retries on retryable failures (default 2)
        --delimiter <CH>     CSV delimiter (default ,)
        --limit <N>          Process at most N rows
        --no-wakelock        Do not acquire a Termux wake lock
    -h, --help               Show this help
";

const GEN_USAGE: &str = "\
bqg gen — render queries and print them (no execution)

USAGE:
    bqg gen -t <template> [--delimiter CH] [--limit N] [input.csv]
";

const SERVE_USAGE: &str = "\
bqg serve — start the local web UI

USAGE:
    bqg serve [--host H] [--port P] [--open]

OPTIONS:
        --host <H>   Bind address (default 127.0.0.1; use 0.0.0.0 for LAN)
        --port <P>   Port (default 8787)
        --open       Open the UI in the device browser (Termux)
    -h, --help       Show this help
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = if args.is_empty() { &[][..] } else { &args[1..] };

    let result = match cmd {
        "gen" => cmd_gen(rest),
        "run" => cmd_run(rest),
        "serve" => cmd_serve(rest),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("bqg {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("bqg: {e}");
            ExitCode::FAILURE
        }
    }
}

/// A minimal flag scanner: supports `--key value`, `--key=value`, short aliases,
/// boolean flags, and a single trailing positional.
struct Args {
    map: std::collections::HashMap<String, String>,
    flags: std::collections::HashSet<String>,
    positional: Vec<String>,
}

impl Args {
    fn parse(raw: &[String], bools: &[&str], aliases: &[(&str, &str)]) -> Result<Args, String> {
        let mut map = std::collections::HashMap::new();
        let mut flags = std::collections::HashSet::new();
        let mut positional = Vec::new();
        let canon = |k: &str| -> String {
            for (short, long) in aliases {
                if k == *short {
                    return long.to_string();
                }
            }
            k.to_string()
        };
        let mut i = 0;
        while i < raw.len() {
            let a = &raw[i];
            if let Some(stripped) = a.strip_prefix("--").or_else(|| a.strip_prefix('-')) {
                if let Some((k, v)) = stripped.split_once('=') {
                    map.insert(canon(k), v.to_string());
                } else {
                    let key = canon(stripped);
                    if bools.contains(&key.as_str()) {
                        flags.insert(key);
                    } else {
                        let v = raw
                            .get(i + 1)
                            .ok_or_else(|| format!("missing value for --{key}"))?;
                        map.insert(key, v.clone());
                        i += 1;
                    }
                }
            } else {
                positional.push(a.clone());
            }
            i += 1;
        }
        Ok(Args {
            map,
            flags,
            positional,
        })
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.map.get(k).map(String::as_str)
    }
    fn has(&self, k: &str) -> bool {
        self.flags.contains(k) || self.map.contains_key(k)
    }
}

fn load_template(a: &Args) -> Result<Template, String> {
    let src = if let Some(p) = a.get("template-file") {
        std::fs::read_to_string(p).map_err(|e| format!("reading template file {p}: {e}"))?
    } else if let Some(t) = a.get("template") {
        t.to_string()
    } else {
        return Err("a template is required (-t/--template or --template-file)".to_string());
    };
    Template::parse(&src)
}

fn open_input(a: &Args) -> Result<(Box<dyn io::Read>, Option<String>), String> {
    match a.positional.first() {
        Some(p) => {
            let f = std::fs::File::open(p).map_err(|e| format!("opening {p}: {e}"))?;
            Ok((Box::new(f), Some(p.clone())))
        }
        None => Ok((Box::new(io::stdin()), None)),
    }
}

fn delimiter(a: &Args) -> u8 {
    a.get("delimiter")
        .and_then(|s| s.bytes().next())
        .unwrap_or(b',')
}

fn cmd_gen(raw: &[String]) -> Result<(), String> {
    let a = Args::parse(raw, &["help"], &[("t", "template"), ("h", "help")])?;
    if a.has("help") {
        print!("{GEN_USAGE}");
        return Ok(());
    }
    let template = load_template(&a)?;
    let (input, _name) = open_input(&a)?;
    let mut reader = Reader::new(input, delimiter(&a));
    let headers = reader
        .headers()
        .map_err(|e| format!("reading header: {e}"))?;
    let col: std::collections::HashMap<String, usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| (h.clone(), i))
        .collect();
    for v in template.variables() {
        if !col.contains_key(v) {
            return Err(format!(
                "template references missing column '{v}'; header is {headers:?}"
            ));
        }
    }
    let limit = a.get("limit").and_then(|s| s.parse::<u64>().ok());
    let mut out = io::BufWriter::new(io::stdout());
    let mut n = 0u64;
    while let Some(rec) = reader
        .next_record()
        .map_err(|e| format!("reading row: {e}"))?
    {
        if let Some(l) = limit {
            if n >= l {
                break;
            }
        }
        let q = template.render(|name| {
            col.get(name)
                .map(|i| rec.get(*i).map_or("", |s| s.as_str()))
        })?;
        writeln!(out, "{q}").map_err(|e| e.to_string())?;
        n += 1;
    }
    out.flush().map_err(|e| e.to_string())
}

fn cmd_run(raw: &[String]) -> Result<(), String> {
    let a = Args::parse(
        raw,
        &["help", "no-wakelock"],
        &[
            ("t", "template"),
            ("o", "output"),
            ("c", "checkpoint"),
            ("j", "concurrency"),
            ("h", "help"),
        ],
    )?;
    if a.has("help") {
        print!("{RUN_USAGE}");
        return Ok(());
    }
    let template = load_template(&a)?;
    let (input, input_name) = open_input(&a)?;
    let reader = Reader::new(input, delimiter(&a));

    let target: Arc<dyn Target> = match a.get("target").unwrap_or("echo") {
        "echo" => Arc::new(Echo),
        "http" => {
            let url = a.get("url").ok_or("target=http requires --url")?;
            let method = a.get("method").unwrap_or("POST");
            let ct = a.get("content-type").unwrap_or("text/plain");
            Arc::new(Http::new(url, method, ct, 15)?)
        }
        other => return Err(format!("unknown target '{other}' (echo|http)")),
    };

    let checkpoint = a
        .get("checkpoint")
        .map(String::from)
        .unwrap_or_else(|| match &input_name {
            Some(p) => format!("{p}.journal"),
            None => "bqg.journal".to_string(),
        });
    let journal =
        Arc::new(Journal::open(&checkpoint).map_err(|e| format!("checkpoint {checkpoint}: {e}"))?);
    let resumed = journal.completed();
    if resumed > 0 {
        eprintln!("bqg: resuming — {resumed} row(s) already completed in {checkpoint}");
    }

    let out: Box<dyn Write + Send> = match a.get("output") {
        Some(p) => Box::new(io::BufWriter::new(
            std::fs::File::create(p).map_err(|e| format!("creating {p}: {e}"))?,
        )),
        None => Box::new(io::BufWriter::new(io::stdout())),
    };

    let cfg = ExecConfig {
        concurrency: a
            .get("concurrency")
            .and_then(|s| s.parse().ok())
            .unwrap_or(4),
        retries: a.get("retries").and_then(|s| s.parse().ok()).unwrap_or(2),
        backoff: Duration::from_millis(250),
        limit: a.get("limit").and_then(|s| s.parse().ok()),
    };

    let progress = Arc::new(Progress::default());
    let observer: Arc<dyn Observer> = Arc::new(NoopObserver);

    let wakelock = !a.has("no-wakelock");
    if wakelock {
        android::wake_lock();
    }

    let res = exec::run_batch(
        reader,
        &template,
        target,
        Arc::clone(&journal),
        Arc::clone(&progress),
        observer,
        out,
        &cfg,
    );

    if wakelock {
        android::wake_unlock();
    }

    let p = &progress;
    eprintln!(
        "bqg: done — ok {} · err {} · skipped {} · journal {}",
        p.ok.load(Ordering::Relaxed),
        p.err.load(Ordering::Relaxed),
        p.skipped.load(Ordering::Relaxed),
        checkpoint,
    );
    if android::in_termux() {
        android::notify(
            "bqg: batch finished",
            &format!(
                "ok {} · err {}",
                p.ok.load(Ordering::Relaxed),
                p.err.load(Ordering::Relaxed)
            ),
        );
    }
    res
}

fn cmd_serve(raw: &[String]) -> Result<(), String> {
    let a = Args::parse(raw, &["help", "open"], &[("h", "help")])?;
    if a.has("help") {
        print!("{SERVE_USAGE}");
        return Ok(());
    }
    let host = a.get("host").unwrap_or("127.0.0.1").to_string();
    let port: u16 = a.get("port").and_then(|s| s.parse().ok()).unwrap_or(8787);
    let open = a.has("open");
    server::serve(&host, port, open).map_err(|e| format!("serve: {e}"))
}
