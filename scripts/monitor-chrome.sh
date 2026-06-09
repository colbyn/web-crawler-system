#!/usr/bin/env zsh
# chrome-crawl-monitor.zsh
#
# Lightweight Chrome/Chromium process monitor for crawler runs.
#
# Reports:
# - number of Chrome-like processes
# - aggregate RSS memory in GiB
# - aggregate CPU percentage
#
# Usage:
#   ./chrome-crawl-monitor.zsh
#   ./chrome-crawl-monitor.zsh --interval 2
#   ./chrome-crawl-monitor.zsh --pattern 'chrome|chromium|Google Chrome'
#
# Stop with Ctrl-C.

set -euo pipefail

interval=5
pattern='chrome|chromium'

usage() {
  cat <<EOF
Usage: $0 [options]

Options:
  -i, --interval SECONDS   Poll interval. Default: 5
  -p, --pattern REGEX      Process regex. Default: chrome|chromium
  -h, --help               Show this help message

Examples:
  $0
  $0 --interval 2
  $0 --pattern 'chrome|chromium|Google Chrome'
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    -i|--interval)
      interval="${2:?missing value for $1}"
      shift 2
      ;;
    -p|--pattern)
      pattern="${2:?missing value for $1}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if ! [[ "$interval" =~ '^[0-9]+([.][0-9]+)?$' ]]; then
  echo "interval must be numeric, got: $interval" >&2
  exit 2
fi

printf "Monitoring Chrome processes every %ss. Stop with Ctrl-C.\n" "$interval"
printf "Pattern: %s\n\n" "$pattern"

while true; do
  timestamp="$(date '+%Y-%m-%d %H:%M:%S')"

  stats="$(
    ps aux \
      | grep -Ei "$pattern" \
      | grep -v -E 'grep|chrome-crawl-monitor\.zsh' \
      | awk '
          {
            rss += $6
            cpu += $3
            n += 1
          }
          END {
            printf "chrome_procs=%d chrome_rss=%.2f GiB chrome_cpu=%.1f%%", n, rss / 1024 / 1024, cpu
          }
        '
  )"

  printf "%s  %s\n" "$timestamp" "$stats"
  sleep "$interval"
done
