#!/usr/bin/env bash
# Runs only the backend in the foreground (useful for API/SMTP development).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec cargo run --release --manifest-path "$ROOT/backend/Cargo.toml" -- --db "$ROOT/data/mymail.db" "$@"
