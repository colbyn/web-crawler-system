#!/usr/bin/env zsh
set -euo pipefail

# Infer JSON Schema from NDJSON samples using the existing GenSON CLI.
#
# Install:
#
#   python3 -m venv .venv
#   source .venv/bin/activate
#   pip install genson
#
# Example:
#
#   scripts/postgres/sample-jsonb.zsh \
#     --table cache_entries \
#     --column metadata_json \
#     --samples 1000 \
#     --out .output/schema/metadata.samples.ndjson
#
#   scripts/postgres/infer-json-schema-genson.zsh \
#     .output/schema/metadata.samples.ndjson \
#     .output/schema/metadata.schema.json

usage() {
  cat >&2 <<'EOF'
usage: infer-json-schema-genson.zsh INPUT_NDJSON OUTPUT_SCHEMA_JSON

Requires:
  genson

Install:
  pip install genson
EOF
}

if (( $# != 2 )); then
  usage
  exit 1
fi

INPUT_PATH="$1"
OUTPUT_PATH="$2"

[[ -f "$INPUT_PATH" ]] || {
  print -u2 -- "error: input file does not exist: $INPUT_PATH"
  exit 1
}

command -v genson >/dev/null 2>&1 || {
  print -u2 -- "error: genson not found"
  print -u2 -- "install with: pip install genson"
  exit 1
}

mkdir -p "$(dirname "$OUTPUT_PATH")"

genson \
  --delimiter newline \
  --indent 2 \
  "$INPUT_PATH" > "$OUTPUT_PATH"

print -u2 -- "wrote ${OUTPUT_PATH}"
