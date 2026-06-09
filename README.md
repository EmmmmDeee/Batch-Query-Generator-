# bqg — Batch Query Generator

Turn a **CSV + a template** into **many queries**, run them with a checkpointed
thread pool, and watch progress in a **local web UI** — from a phone.

`bqg` is a single, statically-linked binary with **zero external dependencies**
(std only). It compiles in seconds, runs identically on x86-64 and aarch64, and
is built to be a good citizen inside **Termux on Android, no root required**.

```
CSV rows ──▶ template {{col}} ──▶ generated queries ──▶ target ──▶ JSONL results
                                          │                          │
                                   resume-skip via            live progress via
                                   checkpoint journal         CLI / web UI (SSE)
```

## Why it looks the way it does

This project is deliberately built in the spirit of Andrew Gallant's
(BurntSushi's) tools — ripgrep, the `csv`/`regex`/`aho-corasick` crates:

- **One binary, no runtime.** No Node, no Python, no `node_modules`. On Android,
  where dependency management is painful, this is the whole game.
- **Zero dependencies, std only.** Nothing to fail to cross-compile for aarch64.
- **Streaming, never slurp.** Rows are parsed one at a time; a 500k-row batch
  runs in flat memory.
- **Threads + channels, not async.** A bounded worker pool — the same way the
  `ignore` crate parallelizes ripgrep — so we never melt a phone's CPU.
- **Library + thin front ends.** The CLI and the web UI are two skins over the
  exact same engine.
- **Tested.** Property-ish unit tests for the CSV/template/JSON cores plus
  black-box integration tests that drive the real binary, including resume.
- **Dual `Unlicense OR MIT`, no telemetry.**

## Install (Termux, aarch64, no root)

```sh
pkg install rust
git clone https://github.com/emmmmdeee/batch-query-generator- bqg && cd bqg
cargo build --release
install -Dm755 target/release/bqg "$PREFIX/bin/bqg"   # or copy anywhere on PATH
```

To read CSVs from shared storage, grant Termux storage once:

```sh
termux-setup-storage    # files then live under ~/storage/shared
```

The optional notification / wake-lock / open-browser niceties use the
`termux-api` package (`pkg install termux-api` + the Termux:API app). They are
**best-effort**: if the helpers aren't present, `bqg` simply skips them.

## Use it

### Web UI

```sh
bqg serve            # binds 127.0.0.1:8787 (a high port — no root needed)
bqg serve --open     # also opens it in the device browser (Termux:API)
bqg serve --host 0.0.0.0 --port 9000   # expose on your LAN (guarded by a token)
```

It prints a URL containing a per-process token; open it in your browser. Paste
CSV (or give a file path), write a template, pick a target, hit **Run**, and
watch live progress stream in over Server-Sent Events. Results are written to
`bqg-run-<id>.jsonl` and the run is checkpointed to `bqg-run-<id>.journal`.

### CLI

```sh
# Dry run: render queries and look at them.
bqg gen -t "SELECT * FROM users WHERE id = {{id}} AND email = '{{email}}';" \
        examples/people.csv

# Execute offline against the echo target (the query is the result).
bqg run -t "Q {{id}}" --target echo examples/people.csv -o out.jsonl

# POST each rendered query to a plain-HTTP endpoint, 8 at a time.
bqg run --template-file examples/query.sql.tmpl \
        --target http --url http://127.0.0.1:8080/query --method POST \
        -j 8 examples/people.csv -o out.jsonl
```

Reads from a file argument or stdin; writes JSONL to stdout or `-o`.

## Resume — why this matters on Android

Android *will* suspend or kill a long-running process (Doze, the low-memory
killer, swiping the app away). So a batch here is a **resumable job**, not a
fire-and-forget loop. Every completed row is appended to a checkpoint journal;
re-running the same command skips rows that already succeeded:

```sh
bqg run -t "Q {{id}}" big.csv -o out.jsonl       # dies at row 80,000…
bqg run -t "Q {{id}}" big.csv -o out.jsonl       # …resumes at 80,000
```

Rows that *errored* are retried on the next run; rows that *succeeded* are
skipped. During a `run`, `bqg` also grabs a Termux wake lock (disable with
`--no-wakelock`) and posts a notification when the batch finishes.

## Targets

| target | what it does | needs network |
|--------|--------------|---------------|
| `echo` | returns the rendered query as the result — default, great for dry runs and demos | no |
| `http` | sends the query to a plain `http://` endpoint (POST body or GET `?q=`) | yes |

Adding a target is implementing one trait (`target::Target`). `https://` is
intentionally **not** built in (it would require a TLS dependency); point the
`http` target at a localhost/internal endpoint, or add a TLS-capable target.

## Output format

One JSON object per row, newline-delimited:

```json
{"row":0,"ok":true,"info":"HTTP 200","query":"SELECT * FROM users WHERE id = 1 ..."}
```

## Develop

```sh
cargo test          # 24 unit + 3 integration tests
cargo build --release
```

## License

Dual-licensed under [Unlicense](UNLICENSE) **or** [MIT](LICENSE-MIT), at your
option.
