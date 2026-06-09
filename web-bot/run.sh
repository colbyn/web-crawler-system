#!/usr/bin/env zsh
set -euo pipefail

# Run the web-bot CLI from anywhere inside the repo
SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
cd "$SCRIPT_DIR"

exec cargo run --release -p web-bot -- "$@"
