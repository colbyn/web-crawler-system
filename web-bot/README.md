# TODO

Possibly out of date. See CLI definitions for current API.

# web-bot

**Operational CLI for the web crawler system.**

`web-bot` is a command-line tool for preemptively crawling web content into a shared filesystem cache and inspecting cached artifacts. It is designed to work alongside the `web-crawler-engine-v3` library.

## Features

- Flexible input formats: plain text, NDJSON, or JSON arrays
- JSON Pointer support for extracting URLs from structured data
- Optional provenance attachment from input JSON
- Rich cache inspection (metadata + HTML snapshots)
- Cache management (`remove`, `clear`, `stats`)
- Health-aware browser session rotation
- Shared cache with applications using the engine

## Building

```bash
cargo build --release -p web-bot
```

The binary will be located at `target/release/web-bot`.

## Commands

| Command                    | Description                                      |
|---------------------------|--------------------------------------------------|
| `crawl`                   | Crawl URLs into the cache                        |
| `cache lookup <url>`      | Show metadata for a cached URL                   |
| `cache snapshot <url>`    | Print or save the HTML snapshot                  |
| `cache remove <url>`      | Remove a specific URL from the cache             |
| `cache clear`             | Delete all cached data                           |
| `cache stats`             | Show cache disk usage statistics                 |

## Usage Examples

### Crawling

```bash
# Basic crawl (one URL per line via stdin)
echo -e "https://example.com\nhttps://example.org" | web-bot crawl

# Crawl with limits
web-bot crawl --max-pages 300 --max-depth 2

# NDJSON input with JSON Pointer
cat companies.ndjson | web-bot crawl \
  --format ndjson \
  --url-pointer "/website"

# Attach full JSON object as provenance
cat leads.ndjson | web-bot crawl \
  --format ndjson \
  --url-pointer "/url" \
  --attach-provenance
```

### Cache Operations

```bash
# View metadata for a cached page
web-bot cache lookup https://example.com/about

# Print the HTML snapshot to stdout
web-bot cache snapshot https://example.com/about

# Save snapshot to a file
web-bot cache snapshot https://example.com/about -o about.html

# Remove one URL from the cache
web-bot cache remove https://example.com/old-page

# Force remove without confirmation
web-bot cache remove https://example.com/old-page --force

# View cache statistics
web-bot cache stats

# Clear the entire cache (with confirmation)
web-bot cache clear

# Force clear everything
web-bot cache clear --force
```

### Common Workflows

**Pre-crawling content before a job:**

```bash
cat production-urls.txt | web-bot crawl --max-pages 2000
web-bot cache stats
```

**Debugging / inspecting a specific page:**

```bash
web-bot cache lookup https://example.com/tricky-page
web-bot cache snapshot https://example.com/tricky-page | head -100
```

**Extracting cached HTML for offline analysis:**

```bash
web-bot cache snapshot https://example.com/report -o report.html
```

## Configuration

| Flag                | Description                        | Default      |
|---------------------|------------------------------------|--------------|
| `--profile-root`    | Browser profile directory          | `./profiles` |
| `--cache-root`      | Page cache directory               | `./cache`    |
| `--format`          | Input format (`text` / `ndjson`)   | `text`       |
| `--url-pointer`     | JSON Pointer to extract URL        | -            |
| `--attach-provenance` | Attach original JSON as provenance | `false`    |
| `--max-pages`       | Max pages to crawl                 | `50`         |
| `--max-depth`       | Max crawl depth                    | `1`          |

## Philosophy

`web-bot` is an **operational / data-preparation tool**. It focuses on:

- Populating the shared cache ahead of time
- Making cache inspection and management easy
- Supporting flexible data pipelines (stdin, NDJSON, JSON Pointers)

Complex business logic, entity resolution, and rich provenance interpretation belong in applications that use the `web-crawler-engine-v3` library directly.

## License

Colbyn Wadman
