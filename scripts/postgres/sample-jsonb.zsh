#!/usr/bin/env zsh
set -euo pipefail

# Dump sampled JSON/JSONB values from a PostgreSQL table as NDJSON.
#
# Intended for web-crawler-db JSON columns:
#
#   cache_entries.key_json
#   cache_entries.capture_policy_json
#   cache_entries.metadata_json
#   cache_auxiliary.value_json
#
# Required:
#
#   DATABASE_URL='postgresql://user:password@host:5432/db'
#
# Examples:
#
#   scripts/postgres/sample-jsonb.zsh \
#     --table cache_entries \
#     --column metadata_json \
#     --samples 1000 \
#     --out .output/schema/metadata.samples.ndjson
#
#   scripts/postgres/sample-jsonb.zsh \
#     --table cache_entries \
#     --column metadata_json \
#     --method system \
#     --system-percent 2 \
#     --where "entry_kind = 'page' AND status_code BETWEEN 200 AND 299" \
#     --out .output/schema/page-success.metadata.samples.ndjson

usage() {
  cat >&2 <<'EOF'
usage: sample-jsonb.zsh [options]

Options:
  --database-url URL        Postgres connection URL. Defaults to DATABASE_URL.
  --table NAME              Table name. Default: cache_entries
  --column NAME             JSON/JSONB column. Default: metadata_json
  --samples N               Max rows to sample. Default: 1000
  --method random|system|latest
                            Sampling method. Default: random
  --system-percent N        TABLESAMPLE SYSTEM percent. Default: 1
  --order-by NAME           Column for latest method. Default: stored_at_unix_ms
  --where SQL               Extra SQL predicate.
  --out PATH                Output NDJSON file. Default: stdout
  -h, --help                Show this help.
EOF
}

die() {
  print -u2 -- "error: $*"
  exit 1
}

sql_ident_ok() {
  [[ "$1" =~ '^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)?$' ]]
}

DATABASE_URL_ARG="${DATABASE_URL:-}"
TABLE_NAME="cache_entries"
COLUMN_NAME="metadata_json"
SAMPLES="1000"
METHOD="random"
SYSTEM_PERCENT="1"
ORDER_BY="stored_at_unix_ms"
WHERE_SQL=""
OUT_PATH=""

while (( $# > 0 )); do
  case "$1" in
    --database-url)
      DATABASE_URL_ARG="${2:-}"
      shift 2
      ;;
    --table)
      TABLE_NAME="${2:-}"
      shift 2
      ;;
    --column)
      COLUMN_NAME="${2:-}"
      shift 2
      ;;
    --samples)
      SAMPLES="${2:-}"
      shift 2
      ;;
    --method)
      METHOD="${2:-}"
      shift 2
      ;;
    --system-percent)
      SYSTEM_PERCENT="${2:-}"
      shift 2
      ;;
    --order-by)
      ORDER_BY="${2:-}"
      shift 2
      ;;
    --where)
      WHERE_SQL="${2:-}"
      shift 2
      ;;
    --out)
      OUT_PATH="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "unknown argument: $1"
      ;;
  esac
done

[[ -n "$DATABASE_URL_ARG" ]] || die "missing DATABASE_URL or --database-url"
sql_ident_ok "$TABLE_NAME" || die "bad table identifier: $TABLE_NAME"
sql_ident_ok "$COLUMN_NAME" || die "bad column identifier: $COLUMN_NAME"
sql_ident_ok "$ORDER_BY" || die "bad order-by identifier: $ORDER_BY"
[[ "$SAMPLES" =~ '^[0-9]+$' ]] || die "--samples must be an integer"
(( SAMPLES > 0 )) || die "--samples must be positive"

BASE_WHERE="${COLUMN_NAME} IS NOT NULL"
if [[ -n "$WHERE_SQL" ]]; then
  BASE_WHERE="${BASE_WHERE} AND (${WHERE_SQL})"
fi

case "$METHOD" in
  random)
    QUERY="
      COPY (
        SELECT ${COLUMN_NAME}::text
        FROM ${TABLE_NAME}
        WHERE ${BASE_WHERE}
        ORDER BY random()
        LIMIT ${SAMPLES}
      ) TO STDOUT;
    "
    ;;
  system)
    QUERY="
      COPY (
        SELECT ${COLUMN_NAME}::text
        FROM ${TABLE_NAME} TABLESAMPLE SYSTEM (${SYSTEM_PERCENT})
        WHERE ${BASE_WHERE}
        LIMIT ${SAMPLES}
      ) TO STDOUT;
    "
    ;;
  latest)
    QUERY="
      COPY (
        SELECT ${COLUMN_NAME}::text
        FROM ${TABLE_NAME}
        WHERE ${BASE_WHERE}
        ORDER BY ${ORDER_BY} DESC
        LIMIT ${SAMPLES}
      ) TO STDOUT;
    "
    ;;
  *)
    die "bad method: $METHOD"
    ;;
esac

if [[ -n "$OUT_PATH" ]]; then
  mkdir -p "$(dirname "$OUT_PATH")"
  psql "$DATABASE_URL_ARG" \
    -v ON_ERROR_STOP=1 \
    -X \
    -q \
    -c "$QUERY" > "$OUT_PATH"

  print -u2 -- "wrote ${OUT_PATH}"
else
  psql "$DATABASE_URL_ARG" \
    -v ON_ERROR_STOP=1 \
    -X \
    -q \
    -c "$QUERY"
fi
