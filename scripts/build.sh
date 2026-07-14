#!/bin/sh
# Build step run by herdr at install/link time.
set -eu
cd "$(dirname "$0")/.."
if ! command -v cargo >/dev/null 2>&1; then
  echo "herdr-jira: cargo not found — install a Rust toolchain (https://rustup.rs) and re-run the install" >&2
  exit 1
fi
cargo build --release
