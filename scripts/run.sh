#!/data/data/com.termux/files/usr/bin/env bash
# Convenience launcher for the bqg web UI.
#
#   scripts/run.sh                  # build if needed, serve on 127.0.0.1:8787
#   scripts/run.sh --open           # also open it in the device browser
#   scripts/run.sh --host 0.0.0.0 --port 9000   # expose on the LAN
#
# Any arguments are passed straight through to `bqg serve`.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

bin="target/release/bqg"
if [ ! -x "$bin" ]; then
  echo "==> building (first run)"
  cargo build --release
fi

exec "$bin" serve "$@"
