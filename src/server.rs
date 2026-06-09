//! A tiny std-only HTTP/1.1 server for the local web UI.
//!
//! No web framework, no async runtime, no bundled JavaScript toolchain: one
//! thread per connection, hand-rolled request parsing, and the entire front end
//! embedded in the binary as a single HTML string. It binds to `127.0.0.1` by
//! default (no root needed for a high port) and gates mutating endpoints behind a
//! per-process token, so another app on the device cannot drive your batches.
//! Live progress streams over Server-Sent Events — no polling, no dependencies.

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::android;
use crate::checkpoint::Journal;
use crate::csv::Reader;
use crate::exec::{self, Event, ExecConfig, Observer, Progress};
use crate::json;
use crate::target::{Echo, Http, Target};
use crate::template::Template;

const PAGE: &str = include_str!("ui.html");

struct RunState {
    progress: Arc<Progress>,
    events: Mutex<Vec<Event>>,
    finished: AtomicBool,
    error: Mutex<Option<String>>,
    results_path: String,
}

struct WebObserver {
    run: Arc<RunState>,
}

impl Observer for WebObserver {
    fn on_event(&self, ev: &Event) {
        let mut evs = self.run.events.lock().unwrap();
        if evs.len() >= 1000 {
            // keep a rolling tail
            let drop = evs.len() - 999;
            evs.drain(0..drop);
        }
        evs.push(ev.clone());
    }
}

struct Server {
    token: String,
    runs: Mutex<HashMap<String, Arc<RunState>>>,
    next_id: AtomicU64,
}

/// Start the web UI. Blocks, serving until the process is killed.
pub fn serve(host: &str, port: u16, open: bool) -> io::Result<()> {
    let listener = TcpListener::bind((host, port))?;
    let token = random_token();
    let server = Arc::new(Server {
        token: token.clone(),
        runs: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
    });

    let url = format!("http://{host}:{port}/?t={token}");
    eprintln!("bqg web UI listening on http://{host}:{port}");
    eprintln!("open: {url}");
    if host == "0.0.0.0" {
        eprintln!("warning: bound to 0.0.0.0 — reachable from your LAN; the token guards control endpoints.");
    }
    if open {
        android::open_url(&url);
    }

    for conn in listener.incoming() {
        match conn {
            Ok(stream) => {
                let server = Arc::clone(&server);
                thread::spawn(move || {
                    let _ = handle(stream, server);
                });
            }
            Err(_) => continue,
        }
    }
    Ok(())
}

fn random_token() -> String {
    // Not cryptographic, just a deterrent against other local apps. Prefer
    // /dev/urandom; fall back to pid+time.
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut bytes).is_ok() {
            return hex(&bytes);
        }
    }
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}{:x}", std::process::id(), t)
}

fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    form: HashMap<String, String>,
}

fn parse_request(reader: &mut BufReader<TcpStream>) -> io::Result<Req> {
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let target = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h)?;
        if n == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    // Cap body to keep memory bounded.
    content_length = content_length.min(16 * 1024 * 1024);
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body)?;
    }

    let (path, query_str) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    Ok(Req {
        method,
        path,
        query: parse_kv(&query_str),
        form: parse_kv(&String::from_utf8_lossy(&body)),
    })
}

fn parse_kv(s: &str) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for pair in s.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        m.insert(percent_decode(k), percent_decode(v));
    }
    m
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let h = hexval(bytes[i + 1]);
                let l = hexval(bytes[i + 2]);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push(h << 4 | l);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn handle(stream: TcpStream, server: Arc<Server>) -> io::Result<()> {
    let read_stream = stream.try_clone()?;
    let mut reader = BufReader::new(read_stream);
    let req = parse_request(&mut reader)?;
    let mut w = stream;

    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => {
            let page = PAGE.replace("__TOKEN__", &server.token);
            respond(
                &mut w,
                200,
                "OK",
                "text/html; charset=utf-8",
                page.as_bytes(),
            )
        }
        ("GET", "/healthz") => respond(&mut w, 200, "OK", "text/plain", b"ok"),
        ("POST", "/run") => handle_run(&mut w, &server, &req),
        ("GET", "/cancel") => handle_cancel(&mut w, &server, &req),
        ("GET", "/events") => handle_events(w, &server, &req),
        _ => respond(&mut w, 404, "Not Found", "text/plain", b"not found"),
    }
}

fn authed(server: &Server, req: &Req) -> bool {
    let t = req
        .form
        .get("t")
        .or_else(|| req.query.get("t"))
        .map(String::as_str)
        .unwrap_or("");
    // constant-ish time compare is overkill for a localhost deterrent
    t == server.token
}

fn handle_run(w: &mut TcpStream, server: &Arc<Server>, req: &Req) -> io::Result<()> {
    if !authed(server, req) {
        return respond(w, 403, "Forbidden", "text/plain", b"bad token");
    }
    let f = &req.form;
    let get = |k: &str| f.get(k).map(String::as_str).unwrap_or("");

    let template_src = get("template");
    let template = match Template::parse(template_src) {
        Ok(t) => t,
        Err(e) => return respond_json(w, 400, &format!("{{\"error\":{}}}", json::quote(&e))),
    };

    let target: Arc<dyn Target> = match get("target") {
        "http" => {
            let url = get("url");
            let method = if get("method").is_empty() {
                "POST"
            } else {
                get("method")
            };
            let ct = if get("content_type").is_empty() {
                "text/plain"
            } else {
                get("content_type")
            };
            match Http::new(url, method, ct, 15) {
                Ok(h) => Arc::new(h),
                Err(e) => {
                    return respond_json(w, 400, &format!("{{\"error\":{}}}", json::quote(&e)))
                }
            }
        }
        _ => Arc::new(Echo),
    };

    let delimiter = get("delimiter").bytes().next().unwrap_or(b',');
    let concurrency = get("concurrency")
        .parse::<usize>()
        .unwrap_or(4)
        .clamp(1, 64);
    let retries = get("retries").parse::<u32>().unwrap_or(2);
    let limit = get("limit").parse::<u64>().ok().filter(|n| *n > 0);

    let csv_path = get("csv_path").to_string();
    let csv_text = get("csv_text").to_string();
    if csv_path.is_empty() && csv_text.trim().is_empty() {
        return respond_json(w, 400, "{\"error\":\"provide csv_path or csv_text\"}");
    }

    let id = server.next_id.fetch_add(1, Ordering::Relaxed).to_string();
    let results_path = format!("bqg-run-{id}.jsonl");
    let checkpoint = if get("checkpoint").is_empty() {
        format!("bqg-run-{id}.journal")
    } else {
        get("checkpoint").to_string()
    };

    let run = Arc::new(RunState {
        progress: Arc::new(Progress::default()),
        events: Mutex::new(Vec::new()),
        finished: AtomicBool::new(false),
        error: Mutex::new(None),
        results_path: results_path.clone(),
    });
    server
        .runs
        .lock()
        .unwrap()
        .insert(id.clone(), Arc::clone(&run));

    let cfg = ExecConfig {
        concurrency,
        retries,
        limit,
        ..Default::default()
    };

    // Run the batch on its own thread; the request returns immediately.
    let run_thread = Arc::clone(&run);
    thread::spawn(move || {
        android::wake_lock();
        let observer: Arc<dyn Observer> = Arc::new(WebObserver {
            run: Arc::clone(&run_thread),
        });
        let journal = match Journal::open(&checkpoint) {
            Ok(j) => Arc::new(j),
            Err(e) => {
                *run_thread.error.lock().unwrap() = Some(format!("opening checkpoint: {e}"));
                run_thread.finished.store(true, Ordering::Relaxed);
                android::wake_unlock();
                return;
            }
        };
        let out: Box<dyn Write + Send> = match std::fs::File::create(&run_thread.results_path) {
            Ok(f) => Box::new(io::BufWriter::new(f)),
            Err(e) => {
                *run_thread.error.lock().unwrap() = Some(format!("creating results file: {e}"));
                run_thread.finished.store(true, Ordering::Relaxed);
                android::wake_unlock();
                return;
            }
        };

        let result = if !csv_text.trim().is_empty() {
            let reader = Reader::new(io::Cursor::new(csv_text.into_bytes()), delimiter);
            exec::run_batch(
                reader,
                &template,
                target,
                journal,
                Arc::clone(&run_thread.progress),
                observer,
                out,
                &cfg,
            )
        } else {
            match std::fs::File::open(&csv_path) {
                Ok(file) => {
                    let reader = Reader::new(file, delimiter);
                    exec::run_batch(
                        reader,
                        &template,
                        target,
                        journal,
                        Arc::clone(&run_thread.progress),
                        observer,
                        out,
                        &cfg,
                    )
                }
                Err(e) => Err(format!("opening {csv_path}: {e}")),
            }
        };

        if let Err(e) = result {
            *run_thread.error.lock().unwrap() = Some(e);
        }
        run_thread.finished.store(true, Ordering::Relaxed);
        let p = &run_thread.progress;
        android::notify(
            "bqg: batch finished",
            &format!(
                "ok {} · err {} · skipped {}",
                p.ok.load(Ordering::Relaxed),
                p.err.load(Ordering::Relaxed),
                p.skipped.load(Ordering::Relaxed)
            ),
        );
        android::wake_unlock();
    });

    respond_json(
        w,
        200,
        &format!(
            "{{\"run\":{},\"results\":{}}}",
            json::quote(&id),
            json::quote(&results_path)
        ),
    )
}

fn handle_cancel(w: &mut TcpStream, server: &Arc<Server>, req: &Req) -> io::Result<()> {
    if !authed(server, req) {
        return respond(w, 403, "Forbidden", "text/plain", b"bad token");
    }
    let id = req.query.get("run").map(String::as_str).unwrap_or("");
    if let Some(run) = server.runs.lock().unwrap().get(id) {
        run.progress.request_cancel();
        return respond_json(w, 200, "{\"cancelled\":true}");
    }
    respond_json(w, 404, "{\"error\":\"no such run\"}")
}

fn handle_events(mut w: TcpStream, server: &Arc<Server>, req: &Req) -> io::Result<()> {
    if !authed(server, req) {
        return respond(&mut w, 403, "Forbidden", "text/plain", b"bad token");
    }
    let id = req
        .query
        .get("run")
        .map(String::as_str)
        .unwrap_or("")
        .to_string();
    let run = match server.runs.lock().unwrap().get(&id) {
        Some(r) => Arc::clone(r),
        None => return respond(&mut w, 404, "Not Found", "text/plain", b"no such run"),
    };

    w.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
    )?;
    w.flush()?;

    let mut cursor = 0usize;
    loop {
        let p = &run.progress;
        let progress_json = format!(
            "{{\"seen\":{},\"ok\":{},\"err\":{},\"skipped\":{},\"done\":{}}}",
            p.total_seen.load(Ordering::Relaxed),
            p.ok.load(Ordering::Relaxed),
            p.err.load(Ordering::Relaxed),
            p.skipped.load(Ordering::Relaxed),
            p.done(),
        );
        if write_sse(&mut w, "progress", &progress_json).is_err() {
            break;
        }

        // Drain new per-row events.
        let new_events: Vec<Event> = {
            let evs = run.events.lock().unwrap();
            if cursor < evs.len() {
                let slice = evs[cursor..].to_vec();
                cursor = evs.len();
                slice
            } else {
                Vec::new()
            }
        };
        for ev in new_events {
            let data = format!(
                "{{\"row\":{},\"ok\":{},\"skipped\":{},\"info\":{}}}",
                ev.row,
                ev.ok,
                ev.skipped,
                json::quote(&ev.info)
            );
            if write_sse(&mut w, "row", &data).is_err() {
                return Ok(());
            }
        }

        if run.finished.load(Ordering::Relaxed) {
            let err = run.error.lock().unwrap().clone();
            let payload = match err {
                Some(e) => format!(
                    "{{\"error\":{},\"results\":{}}}",
                    json::quote(&e),
                    json::quote(&run.results_path)
                ),
                None => format!("{{\"results\":{}}}", json::quote(&run.results_path)),
            };
            let _ = write_sse(&mut w, "done", &payload);
            break;
        }
        thread::sleep(Duration::from_millis(250));
    }
    Ok(())
}

fn write_sse(w: &mut TcpStream, event: &str, data: &str) -> io::Result<()> {
    w.write_all(format!("event: {event}\ndata: {data}\n\n").as_bytes())?;
    w.flush()
}

fn respond(
    w: &mut TcpStream,
    status: u16,
    status_text: &str,
    content_type: &str,
    body: &[u8],
) -> io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    w.write_all(head.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

fn respond_json(w: &mut TcpStream, status: u16, body: &str) -> io::Result<()> {
    let text = if status < 300 { "OK" } else { "Error" };
    respond(w, status, text, "application/json", body.as_bytes())
}
