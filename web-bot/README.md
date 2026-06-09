# web-bot

**Operational CLI for the web crawler system.**

`web-bot` is a command-line tool for preemptively crawling web content into a shared SQLite cache and inspecting cached artifacts. It is designed to work alongside the `web-crawler-engine-v3` library.

The crawler stores reusable page artifacts in SQLite:

* request/cache identity,
* response metadata,
* extracted page facts,
* HTML snapshots,
* payload descriptors,
* structured cache tags.

Tags are the durable association mechanism. A cached artifact is one thing; the reasons you care about it are many things. Tags let the CLI and downstream apps associate cached pages with entities, categories, batches, manual runs, experiments, or imports.

## Features

* Flexible input formats: plain text, NDJSON, or JSON lines
* JSON Pointer support for extracting URLs from structured input
* Global seed tags via `--tag kind:key`
* JSON-derived seed tags via `--tag-pointer kind=/json/pointer`
* Tag inheritance from seed pages to discovered pages
* SQLite-backed cache storage
* HTML snapshot inspection and export
* Cache management: lookup, snapshot, remove, clear, stats
* Structured tag lookup by exact tag or tag kind
* Health-aware browser session rotation
* Shared cache usable by downstream applications

## Building

```bash
cargo build --release -p web-bot
```

The binary will be located at:

```bash
target/release/web-bot
```

## Global Options

| Flag                    | Description                                 | Default                            |
| ----------------------- | ------------------------------------------- | ---------------------------------- |
| `--profile-root <path>` | Directory where browser profiles are stored | `./output/web-bot/profiles`        |
| `--cache-db <path>`     | SQLite cache database path                  | `./output/web-bot/db/cache.sqlite` |

Global options go before the subcommand:

```bash
web-bot --cache-db ./output/web-bot/db/cache.sqlite crawl -i https://example.com
```

## Commands

| Command                          | Description                                   |
| -------------------------------- | --------------------------------------------- |
| `crawl`                          | Crawl URLs into the SQLite cache              |
| `cache lookup <url>`             | Show metadata for a cached URL                |
| `cache snapshot <url>`           | Print or save the HTML snapshot               |
| `cache remove <url>`             | Remove a specific URL from the cache          |
| `cache clear`                    | Delete all cached data                        |
| `cache stats`                    | Show cache statistics                         |
| `cache tag <url> <tags...>`      | Add tags to one cached URL                    |
| `cache list-by-tag <tag>`        | List cache entries by exact tag               |
| `cache list-by-tag-kind <kind>`  | List cache entries carrying any tag of a kind |
| `cache list-tags-by-kind <kind>` | List known tags of a kind                     |

## Tag Model

Tags use this form:

```text
kind:key
```

Examples:

```text
entity:business-123
category:electricians
category:hvac
run:manual-debug
batch:2026-06-08
```

Tags are structured internally as:

```text
tag_kind = "entity"
tag_key  = "business-123"
```

This supports exact-tag lookup:

```bash
web-bot cache list-by-tag entity:business-123
```

and tag-kind lookup:

```bash
web-bot cache list-by-tag-kind entity
```

Tags attached to a seed are inherited by all discovered pages reached from that seed. This lets you ask:

```text
show me every cached page scraped for entity:business-123
show me every cached page scraped for category:electricians
show me every cached page scraped during run:manual-debug
```

## Crawling

### Basic crawl

```bash
web-bot crawl -i https://example.com
```

Multiple URLs can be provided:

```bash
web-bot crawl \
  -i https://example.com \
  -i https://example.org
```

Plain text can also come from stdin:

```bash
cat urls.txt | web-bot crawl
```

or explicitly:

```bash
cat urls.txt | web-bot crawl -i -
```

### Crawl with limits

```bash
web-bot crawl \
  -i https://example.com \
  --max-pages 300 \
  --max-depth 2
```

| Flag              | Description                                 | Default |
| ----------------- | ------------------------------------------- | ------- |
| `--max-pages <n>` | Maximum pages to process                    | `50`    |
| `--max-depth <n>` | Maximum crawl depth from each seed          | `1`     |
| `--no-cache`      | Disable cache lookup/storage for this crawl | `false` |
| `--json`          | Output crawl results as NDJSON              | `false` |

### Crawl with global tags

```bash
web-bot crawl \
  -i https://example.com \
  --tag run:manual-debug \
  --tag category:electricians
```

Every page reached from the seed will inherit these tags.

### NDJSON input with URL extraction

```bash
cat companies.ndjson | web-bot crawl \
  -i - \
  --format ndjson \
  --url-pointer /website
```

If no `--url-pointer` is supplied, the CLI looks for a top-level `url` field.

### NDJSON input with JSON-derived tags

Use `--tag-pointer kind=/json/pointer` to create tags from each input row.

```bash
cat businesses.ndjson | web-bot crawl \
  -i - \
  --format ndjson \
  --url-pointer /website \
  --tag category:electricians \
  --tag-pointer entity=/id
```

For an input row like:

```json
{
  "id": "business-123",
  "website": "https://example.com"
}
```

the seed receives:

```text
category:electricians
entity:business-123
```

If the JSON pointer resolves to an array, each scalar item becomes a tag.

Example:

```json
{
  "website": "https://example.com",
  "categories": ["electricians", "hvac"]
}
```

Command:

```bash
cat businesses.ndjson | web-bot crawl \
  -i - \
  --format ndjson \
  --url-pointer /website \
  --tag-pointer category=/categories
```

Resulting tags:

```text
category:electricians
category:hvac
```

### Deprecated provenance flag

`--attach-provenance` is currently ignored.

Use tags instead:

```bash
web-bot crawl \
  --format ndjson \
  --url-pointer /website \
  --tag-pointer entity=/id \
  --tag-pointer batch=/batch_id
```

## Cache Operations

### Lookup metadata

```bash
web-bot cache lookup https://example.com/about
```

With JSON output:

```bash
web-bot cache lookup https://example.com/about --json
```

Include payload bodies when possible:

```bash
web-bot cache lookup https://example.com/about --json --full
```

### Print the HTML snapshot

```bash
web-bot cache snapshot https://example.com/about
```

### Save a snapshot to a file

```bash
web-bot cache snapshot https://example.com/about -o about.html
```

### Add tags to an existing cached URL

```bash
web-bot cache tag https://example.com/about \
  entity:business-123 \
  category:electricians
```

### List entries by exact tag

```bash
web-bot cache list-by-tag entity:business-123
```

JSON output:

```bash
web-bot cache list-by-tag entity:business-123 --json
```

### List entries by tag kind

```bash
web-bot cache list-by-tag-kind category
```

This returns all entries with any `category:*` tag.

### List known tags by kind

```bash
web-bot cache list-tags-by-kind entity
```

### Remove one URL from the cache

```bash
web-bot cache remove https://example.com/old-page
```

Force remove without confirmation:

```bash
web-bot cache remove https://example.com/old-page --force
```

### View cache statistics

```bash
web-bot cache stats
```

JSON output:

```bash
web-bot cache stats --json
```

### Clear the entire cache

```bash
web-bot cache clear
```

Force clear everything:

```bash
web-bot cache clear --force
```

## Common Workflows

### Manual debug crawl

```bash
web-bot crawl \
  -i https://example.com \
  --max-depth 1 \
  --max-pages 20 \
  --tag run:manual-debug
```

Then inspect all pages from that run:

```bash
web-bot cache list-by-tag run:manual-debug
```

### Pre-crawl one business category

```bash
cat electricians.ndjson | web-bot crawl \
  -i - \
  --format ndjson \
  --url-pointer /website \
  --tag category:electricians \
  --tag-pointer entity=/id \
  --max-depth 1 \
  --max-pages 5000
```

Then list all cached pages associated with that category:

```bash
web-bot cache list-by-tag category:electricians
```

### Inspect a specific cached page

```bash
web-bot cache lookup https://example.com/contact
web-bot cache snapshot https://example.com/contact | head -100
```

### Export cached HTML for offline analysis

```bash
web-bot cache snapshot https://example.com/report -o report.html
```

### Query all entity-associated pages

```bash
web-bot cache list-by-tag-kind entity
```

### Query all known entity tags

```bash
web-bot cache list-tags-by-kind entity
```

## Configuration Summary

| Scope            | Flag                          | Description                               | Default                            |
| ---------------- | ----------------------------- | ----------------------------------------- | ---------------------------------- |
| Global           | `--profile-root`              | Browser profile directory                 | `.output/web-bot/profiles`        |
| Global           | `--cache-db`                  | SQLite cache database path                | `.output/web-bot/db/cache.sqlite` |
| `crawl`          | `--format`                    | Input format: `text`, `ndjson`, or `json` | `text`                             |
| `crawl`          | `--url-pointer`               | JSON Pointer to extract URL               | none                               |
| `crawl`          | `--tag kind:key`              | Attach global tag to every seed           | none                               |
| `crawl`          | `--tag-pointer kind=/pointer` | Attach JSON-derived tags                  | none                               |
| `crawl`          | `--max-pages`                 | Max pages to crawl                        | `50`                               |
| `crawl`          | `--max-depth`                 | Max crawl depth                           | `1`                                |
| `crawl`          | `--no-cache`                  | Disable cache lookup/storage              | `false`                            |
| `crawl`          | `--json`                      | Output results as NDJSON                  | `false`                            |
| `cache lookup`   | `--json`                      | Output metadata as JSON                   | `false`                            |
| `cache lookup`   | `--full`                      | Include payload bodies when possible      | `false`                            |
| `cache snapshot` | `-o, --output`                | Save snapshot to file                     | stdout                             |
| `cache remove`   | `--force`                     | Skip confirmation                         | `false`                            |
| `cache clear`    | `--force`                     | Skip confirmation                         | `false`                            |
| `cache stats`    | `--json`                      | Output stats as JSON                      | `false`                            |

## Philosophy

`web-bot` is an operational data-preparation tool.

It focuses on:

* populating the shared SQLite cache ahead of time,
* making cache inspection and management easy,
* supporting flexible data pipelines,
* preserving caller associations through structured tags.

Complex business logic, entity resolution, scoring, and rich interpretation belong in applications that use `web-crawler-engine-v3` directly.

The important boundary is:

```text
cache artifact = reusable browser/page evidence
tag association = why a caller cares about that artifact
auxiliary data = derived facts about that artifact
```

This keeps the cache boring, queryable, and reusable.

## License

Copyright 2026 Colbyn Wadman

