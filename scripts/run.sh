#!/usr/bin/env bash
# Builds everything and launches MyMail. The app starts (and stops) the server itself.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/build.sh"
echo "==> Launching MyMail"
open "$ROOT/build/MyMail.app"
