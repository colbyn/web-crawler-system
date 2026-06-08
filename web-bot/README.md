# web-bot

Operational CLI for the web crawler system.

`web-bot` lets you preemptively crawl content into a shared cache and inspect cached artifacts. It is designed to work alongside the `web-crawler-engine-v3` library.

## Features

- Flexible input: plain text, NDJSON, or JSON
- JSON Pointer support for extracting URLs from structured data
- Optional provenance attachment from input JSON
- Shared filesystem cache with the engine
- Session rotation and health-aware browser management

## Building

```bash
cargo build --release -p web-bot
```

The binary will be available at `target/release/web-bot`.

## Usage

### Crawl URLs

```bash
# Simple text input (one URL per line)
echo -e "https://example.com\nhttps://example.org" | web-bot crawl

# From a file
web-bot crawl --input seeds.txt

# NDJSON with URL pointer
cat companies.ndjson | web-bot crawl --format ndjson --url-pointer "/website"

# With provenance attached
cat companies.ndjson | web-bot crawl --format ndjson --url-pointer "/website" --attach-provenance

# Limit crawl size
web-bot crawl --max-pages 100 --max-depth 2
```

### Cache Inspection

```bash
# Check if a URL is cached
web-bot cache lookup https://example.com

# Show cache statistics
web-bot cache stats
```

## Common Options

| Flag                    | Description                              | Default      |
|-------------------------|------------------------------------------|--------------|
| `--profile-root`        | Browser profile directory                | `./profiles` |
| `--cache-root`          | Page cache directory                     | `./cache`    |
| `--format`              | Input format (`text`, `ndjson`, `json`)  | `text`       |
| `--url-pointer`         | JSON Pointer to extract URL              | -            |
| `--attach-provenance`   | Attach full JSON object as provenance    | `false`      |
| `--max-pages`           | Max pages to crawl                       | `50`         |
| `--max-depth`           | Max hop depth                            | `1`          |

## Input Formats

- **`text`**: One URL per line
- **`ndjson`**: One JSON object per line
- **`json`**: JSON array of objects

When using `ndjson` or `json`, use `--url-pointer` to specify where the URL lives (e.g. `/website` or `/contact/url`).

## Philosophy

`web-bot` is an **operational tool**. It focuses on:

- Populating the shared cache ahead of time
- Making cache inspection easy
- Supporting flexible data pipelines via stdin/NDJSON

Complex business logic and provenance interpretation belong in applications using the `web-crawler-engine-v3` library directly.

