#!/data/data/com.termux/files/usr/bin/env bash
# Install bqg in Termux (Android, aarch64, no root). Also works on a normal
# Linux box. Builds the single static binary and drops it on PATH.
set -euo pipefail

here="$(cd "$(dirname "$0")/.." && pwd)"
cd "$here"

# 1. Toolchain.
if ! command -v cargo >/dev/null 2>&1; then
  if command -v pkg >/dev/null 2>&1; then
    echo "==> installing rust via pkg"
    pkg install -y rust
  else
    echo "error: cargo not found and no 'pkg' to install it. Install Rust first." >&2
    exit 1
  fi
fi

# 2. Optional Termux niceties (wake lock / notifications / open-url).
if command -v pkg >/dev/null 2>&1 && ! command -v termux-wake-lock >/dev/null 2>&1; then
  echo "==> installing termux-api (optional, for wake lock + notifications)"
  pkg install -y termux-api || echo "   (skipped; install the Termux:API app to enable)"
fi

# 3. Build.
echo "==> building release binary"
cargo build --release

# 4. Install onto PATH.
bin="target/release/bqg"
dest="${PREFIX:-/usr/local}/bin"
mkdir -p "$dest"
install -m755 "$bin" "$dest/bqg"
echo "==> installed: $dest/bqg"
echo "    try: bqg serve"
