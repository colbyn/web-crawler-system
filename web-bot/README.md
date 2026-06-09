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
* Unified CLI/config vocabulary
* Reusable TOML crawl profiles
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
| `--profile-root <path>` | Directory where browser profiles are stored | `.output/web-bot/profiles`         |
| `--cache-db <path>`     | SQLite cache database path                  | `.output/web-bot/db/cache.sqlite`  |

Global options go before the subcommand:

```bash
web-bot --cache-db .output/web-bot/db/cache.sqlite crawl -i https://example.com
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

## Design: One Vocabulary, Two Shapes

`web-bot crawl` exposes the same concepts through CLI flags and TOML settings.

TOML is nested and reusable:

```toml
[budget]
pages = 20
depth = 1

[runtime]
jobs = 8
sessions = 4
tabs = 2
```

CLI is flattened and convenient:

```bash
web-bot crawl \
  -i https://example.com \
  --pages 20 \
  --depth 1 \
  --jobs 8 \
  --sessions 4 \
  --tabs 2
```

The leaf names match wherever practical:

| TOML setting              | CLI flag              |
| ------------------------- | --------------------- |
| `[input].urls`            | `-i`, `--input`       |
| `[input].format`          | `--format`            |
| `[input].url-pointer`     | `--url-pointer`       |
| `[tags].global`           | `--tag`               |
| `[tags].pointers`         | `--tag-pointer`       |
| `[output].format`         | `--output`            |
| `[budget].pages`          | `--pages`             |
| `[budget].total-pages`    | `--total-pages`       |
| `[budget].depth`          | `--depth`             |
| `[budget].frontier-items` | `--frontier-items`    |
| `[runtime].jobs`          | `--jobs`              |
| `[runtime].sessions`      | `--sessions`          |
| `[runtime].tabs`          | `--tabs`              |
| `[runtime].cache-jobs`    | `--cache-jobs`        |
| `[runtime].rotate`        | `--rotate`            |
| `[runtime].timeout-secs`  | `--timeout-secs`      |
| `[profile].strategy`      | `--profile-strategy`  |
| `[profile].key`           | `--profile`           |
| `[cache].enabled`         | `--no-cache`          |
| `[cache].namespace`       | `--namespace`         |

CLI arguments override TOML settings. Repeated inputs and tags are additive: config values first, CLI values second.

```text
explicit CLI argument > TOML config file > hardcoded default
```

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

Exact-tag lookup:

```bash
web-bot cache list-by-tag entity:business-123
```

Tag-kind lookup:

```bash
web-bot cache list-by-tag-kind entity
```

Tags attached to a seed are inherited by discovered pages reached from that seed. This lets downstream tools ask:

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

### Crawl with budget limits

```bash
web-bot crawl \
  -i https://example.com \
  --pages 20 \
  --depth 1 \
  --total-pages 100
```

| Flag                   | Description                                      | Default  |
| ---------------------- | ------------------------------------------------ | -------- |
| `--pages <n>`          | Maximum opened pages per seed                    | `10`     |
| `--total-pages <n>`    | Optional global page budget for the whole crawl  | none     |
| `--depth <n>`          | Maximum crawl depth from each seed               | `1`      |
| `--frontier-items <n>` | Maximum URLs retained in the frontier            | `100000` |

`--pages` is the primary crawl budget. It is per original seed. `--total-pages` is a global emergency brake across the whole invocation.

### Crawl with runtime controls

```bash
web-bot crawl \
  -i https://example.com \
  --jobs 8 \
  --sessions 4 \
  --tabs 2 \
  --rotate 150 \
  --timeout-secs 45
```

| Flag                 | Description                                      | Default |
| -------------------- | ------------------------------------------------ | ------- |
| `--jobs <n>`         | Global in-flight page jobs                       | `8`     |
| `--sessions <n>`     | Maximum live Chromium browser sessions           | `4`     |
| `--tabs <n>`         | Maximum concurrent tabs/pages per browser        | `2`     |
| `--cache-jobs <n>`   | Maximum concurrent cache operations              | `32`    |
| `--rotate <n>`       | Rotate each browser session after this many pages| `150`   |
| `--timeout-secs <n>` | Page open timeout in seconds                     | `45`    |

A practical rule:

```text
jobs ≈ sessions × tabs
```

For example:

```bash
web-bot crawl -i https://example.com --sessions 4 --tabs 2 --jobs 8
```

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

Input row:

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

### Output formats

Human output is the default:

```bash
web-bot crawl -i https://example.com --output human
```

NDJSON output is useful for pipelines:

```bash
cat businesses.ndjson | web-bot crawl \
  -i - \
  --format ndjson \
  --url-pointer /website \
  --output ndjson \
  > crawl-results.ndjson
```

`--json` is retained as a deprecated hidden alias for `--output ndjson`.

### Cache controls

Disable crawler SQLite cache lookup/storage:

```bash
web-bot crawl -i https://example.com --no-cache
```

This does not necessarily disable Chromium's own browser-profile cache.

Set a cache namespace:

```bash
web-bot crawl -i https://example.com --namespace debug
```

Important: only document/use namespace operationally if the engine cache-key path honors it. The CLI and config can expose the setting, but namespace must be wired into cache-key creation to affect stored artifacts.

### Profile strategy

```bash
web-bot crawl \
  -i https://example.com \
  --profile-strategy by-seed-host \
  --profile default
```

Supported strategies:

| Strategy                    | Meaning                                                   |
| --------------------------- | --------------------------------------------------------- |
| `single`                    | Every request uses the configured fallback profile        |
| `caller-provided-or-single` | Use caller-provided profile keys when available           |
| `by-host`                   | Derive browser profile from requested URL host            |
| `by-seed-host`              | Derive browser profile from original seed URL host        |

Default:

```text
by-seed-host
```

## TOML Settings

A crawl profile is a reusable TOML file passed with `--config`.

```bash
web-bot crawl --config web-bot/config/crawl.quick.toml -i https://example.com
```

Canonical settings format:

```toml
[input]
urls = [
  "https://books.toscrape.com",
  "https://quotes.toscrape.com/js/"
]
format = "text"
url-pointer = "/url"
attach-provenance = false

[tags]
global = ["run:manual-debug"]
pointers = []

[output]
format = "human"

[budget]
pages = 10
total-pages = 50
depth = 1
frontier-items = 100000

[runtime]
jobs = 8
sessions = 4
tabs = 2
cache-jobs = 32
rotate = 150
timeout-secs = 45

[profile]
strategy = "by-seed-host"
key = "default"

[cache]
enabled = true
namespace = "default"
```

### Recommended template profiles

Suggested config directory:

```text
web-bot/config/
  crawl.quick.toml
  crawl.debug.toml
  crawl.batch.toml
  crawl.cache-warm.toml
  crawl.low-resource.toml
```

### `crawl.quick.toml`

Small manual crawl for smoke tests and one-off checks.

```toml
[input]
urls = []
format = "text"
url-pointer = "/url"
attach-provenance = false

[tags]
global = ["run:quick"]
pointers = []

[output]
format = "human"

[budget]
pages = 10
total-pages = 50
depth = 1
frontier-items = 10000

[runtime]
jobs = 4
sessions = 2
tabs = 2
cache-jobs = 16
rotate = 100
timeout-secs = 45

[profile]
strategy = "by-seed-host"
key = "default"

[cache]
enabled = true
namespace = "default"
```

Run:

```bash
web-bot crawl --config web-bot/config/crawl.quick.toml -i https://example.com
```

### `crawl.debug.toml`

Conservative crawl for debugging correctness.

```toml
[input]
urls = []
format = "text"
url-pointer = "/url"
attach-provenance = false

[tags]
global = ["run:debug"]
pointers = []

[output]
format = "human"

[budget]
pages = 20
total-pages = 20
depth = 1
frontier-items = 5000

[runtime]
jobs = 1
sessions = 1
tabs = 1
cache-jobs = 4
rotate = 25
timeout-secs = 60

[profile]
strategy = "single"
key = "debug"

[cache]
enabled = true
namespace = "debug"
```

Run:

```bash
RUST_LOG=debug web-bot crawl \
  --config web-bot/config/crawl.debug.toml \
  -i https://example.com
```

### `crawl.batch.toml`

Workhorse profile for NDJSON entity enrichment.

```toml
[input]
urls = ["-"]
format = "ndjson"
url-pointer = "/website"
attach-provenance = false

[tags]
global = ["run:batch"]
pointers = [
  "entity=/id",
  "category=/categories"
]

[output]
format = "human"

[budget]
pages = 10
total-pages = 5000
depth = 1
frontier-items = 100000

[runtime]
jobs = 8
sessions = 4
tabs = 2
cache-jobs = 32
rotate = 150
timeout-secs = 45

[profile]
strategy = "by-seed-host"
key = "default"

[cache]
enabled = true
namespace = "default"
```

Run:

```bash
cat businesses.ndjson | web-bot crawl --config web-bot/config/crawl.batch.toml
```

### `crawl.cache-warm.toml`

Shallow, broad crawl for preloading reusable page evidence.

```toml
[input]
urls = ["-"]
format = "text"
url-pointer = "/url"
attach-provenance = false

[tags]
global = ["run:cache-warm"]
pointers = []

[output]
format = "human"

[budget]
pages = 3
total-pages = 10000
depth = 0
frontier-items = 50000

[runtime]
jobs = 12
sessions = 4
tabs = 3
cache-jobs = 64
rotate = 250
timeout-secs = 35

[profile]
strategy = "by-host"
key = "default"

[cache]
enabled = true
namespace = "default"
```

Run:

```bash
cat urls.txt | web-bot crawl --config web-bot/config/crawl.cache-warm.toml
```

### `crawl.low-resource.toml`

Laptop-friendly crawl profile.

```toml
[input]
urls = []
format = "text"
url-pointer = "/url"
attach-provenance = false

[tags]
global = ["run:low-resource"]
pointers = []

[output]
format = "human"

[budget]
pages = 10
total-pages = 100
depth = 1
frontier-items = 25000

[runtime]
jobs = 2
sessions = 1
tabs = 2
cache-jobs = 8
rotate = 50
timeout-secs = 60

[profile]
strategy = "single"
key = "low-resource"

[cache]
enabled = true
namespace = "default"
```

Run:

```bash
web-bot crawl \
  --config web-bot/config/crawl.low-resource.toml \
  -i https://example.com
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
  --depth 1 \
  --pages 20 \
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
  --depth 1 \
  --pages 10 \
  --total-pages 5000
```

Then list all cached pages associated with that category:

```bash
web-bot cache list-by-tag category:electricians
```

### Warm cache first, tag later

First prefetch URLs:

```bash
cat urls.txt | web-bot crawl \
  --config web-bot/config/crawl.cache-warm.toml
```

Then run entity-tagged input later. Previously cached pages should be reused where cache keys match:

```bash
cat businesses.ndjson | web-bot crawl \
  --config web-bot/config/crawl.batch.toml
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

| Scope            | Flag                          | TOML setting              | Description                              | Default                            |
| ---------------- | ----------------------------- | ------------------------- | ---------------------------------------- | ---------------------------------- |
| Global           | `--profile-root`              | n/a                       | Browser profile directory                | `.output/web-bot/profiles`         |
| Global           | `--cache-db`                  | n/a                       | SQLite cache database path               | `.output/web-bot/db/cache.sqlite`  |
| `crawl`          | `--config`                    | n/a                       | Load crawl settings from TOML            | none                               |
| `crawl`          | `-i`, `--input`               | `[input].urls`            | URL input or `-` for stdin               | stdin when empty                   |
| `crawl`          | `--format`                    | `[input].format`          | Input format: `text`, `ndjson`, `json`   | `text`                             |
| `crawl`          | `--url-pointer`               | `[input].url-pointer`     | JSON Pointer to extract URL              | none / top-level `url`             |
| `crawl`          | `--tag kind:key`              | `[tags].global`           | Attach global tag to every seed          | none                               |
| `crawl`          | `--tag-pointer kind=/pointer` | `[tags].pointers`         | Attach JSON-derived tags                 | none                               |
| `crawl`          | `--output`                    | `[output].format`         | Output format: `human`, `ndjson`         | `human`                            |
| `crawl`          | `--pages`                     | `[budget].pages`          | Max opened pages per seed                | `10`                               |
| `crawl`          | `--total-pages`               | `[budget].total-pages`    | Global crawl page budget                 | none                               |
| `crawl`          | `--depth`                     | `[budget].depth`          | Max crawl depth                          | `1`                                |
| `crawl`          | `--frontier-items`            | `[budget].frontier-items` | Max retained frontier URLs               | `100000`                           |
| `crawl`          | `--jobs`                      | `[runtime].jobs`          | Global in-flight page jobs               | `8`                                |
| `crawl`          | `--sessions`                  | `[runtime].sessions`      | Max browser sessions                     | `4`                                |
| `crawl`          | `--tabs`                      | `[runtime].tabs`          | Max tabs/pages per session               | `2`                                |
| `crawl`          | `--cache-jobs`                | `[runtime].cache-jobs`    | Max concurrent cache operations          | `32`                               |
| `crawl`          | `--rotate`                    | `[runtime].rotate`        | Pages before browser session rotation    | `150`                              |
| `crawl`          | `--timeout-secs`              | `[runtime].timeout-secs`  | Page open timeout in seconds             | `45`                               |
| `crawl`          | `--profile-strategy`          | `[profile].strategy`      | Browser profile assignment strategy      | `by-seed-host`                     |
| `crawl`          | `--profile`                   | `[profile].key`           | Fallback/single browser profile key      | `default`                          |
| `crawl`          | `--namespace`                 | `[cache].namespace`       | Optional cache namespace                 | none                               |
| `crawl`          | `--no-cache`                  | `[cache].enabled`         | Disable SQLite cache lookup/storage      | `false`                            |
| `cache lookup`   | `--json`                      | n/a                       | Output metadata as JSON                  | `false`                            |
| `cache lookup`   | `--full`                      | n/a                       | Include payload bodies when possible     | `false`                            |
| `cache snapshot` | `-o`, `--output`              | n/a                       | Save snapshot to file                    | stdout                             |
| `cache remove`   | `--force`                     | n/a                       | Skip confirmation                        | `false`                            |
| `cache clear`    | `--force`                     | n/a                       | Skip confirmation                        | `false`                            |
| `cache stats`    | `--json`                      | n/a                       | Output stats as JSON                     | `false`                            |

## Compatibility Notes

Older crawl flags are retained as aliases where supported:

| Older flag                              | Preferred flag      |
| --------------------------------------- | ------------------- |
| `--max-pages`                           | `--pages`           |
| `--max-pages-per-seed`                  | `--pages`           |
| `--max-total-pages`                     | `--total-pages`     |
| `--max-depth`                           | `--depth`           |
| `--max-frontier-items`                  | `--frontier-items`  |
| `--max-concurrent-pages`                | `--jobs`            |
| `--max-sessions`                        | `--sessions`        |
| `--max-concurrent-pages-per-session`    | `--tabs`            |
| `--max-concurrent-cache-ops`            | `--cache-jobs`      |
| `--max-pages-per-session`               | `--rotate`          |
| `--timeout`                             | `--timeout-secs`    |
| `--json`                                | `--output ndjson`   |

Older TOML keys are accepted as compatibility aliases where supported:

| Older TOML key                         | Preferred TOML key        |
| -------------------------------------- | ------------------------- |
| `[input].inputs`                       | `[input].urls`            |
| `[budget].pages-per-seed`              | `[budget].pages`          |
| `[budget].max-pages`                   | `[budget].pages`          |
| `[budget].max-total-pages`             | `[budget].total-pages`    |
| `[budget].max-depth`                   | `[budget].depth`          |
| `[budget].max-frontier-items`          | `[budget].frontier-items` |
| `[runtime].page-jobs`                  | `[runtime].jobs`          |
| `[runtime].browser-sessions`           | `[runtime].sessions`      |
| `[runtime].tabs-per-session`           | `[runtime].tabs`          |
| `[runtime].pages-before-session-rotation` | `[runtime].rotate`     |
| `[runtime].page-open-timeout-secs`     | `[runtime].timeout-secs`  |
| `[output].json = true`                 | `[output].format = "ndjson"` |

`--attach-provenance` is deprecated and ignored. Use tags instead:

```bash
web-bot crawl \
  --format ndjson \
  --url-pointer /website \
  --tag-pointer entity=/id \
  --tag-pointer batch=/batch_id
```

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

