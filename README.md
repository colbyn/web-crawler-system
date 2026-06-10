# Web Crawler System

Rust workspace for crawling websites into a shared SQLite-backed page cache.

## Projects

* [`web-bot`](./web-bot/README.md) — operational CLI for crawling URLs, tagging seed contexts, and inspecting cached artifacts.
* `web-crawler-engine-v3` — crawler orchestration library.
* `web-browser-driver` — browser/CDP wrapper used by the crawler engine.

## Quick Start

Build the CLI:

```bash
cargo build --release -p web-bot
```

Crawl a URL:

```bash
cargo run --release -p web-bot -- crawl \
  -i https://example.com \
  --tag run:manual-debug
```

Crawl with a reusable settings profile:

```bash
cargo run --release -p web-bot -- crawl \
  --config web-bot/config/crawl.quick.toml \
  -i https://example.com
```

Inspect the cache:

```bash
cargo run --release -p web-bot -- cache stats
cargo run --release -p web-bot -- cache list --tag run:manual-debug
cargo run --release -p web-bot -- cache tags --kind run
```

## Configuration Model

`web-bot crawl` uses one vocabulary in two shapes:

```text
TOML setting            CLI flag
[input].format          --format
[budget].pages          --pages
[budget].depth          --depth
[runtime].jobs          --jobs
[runtime].sessions      --sessions
[runtime].tabs          --tabs
[output].format         --output
```

This keeps one-off shell commands and reusable TOML profiles aligned.

Example:

```toml
[input]
urls = ["https://example.com"]
format = "text"

[tags]
global = ["run:manual-debug"]
pointers = []

[output]
format = "human"

[budget]
pages = 10
depth = 1
total-pages = 50
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

## Cache Model

The SQLite cache stores reusable page artifacts. Tags are a secondary association layer over those artifacts.

A cached artifact answers:

```text
what page evidence did we capture?
```

A tag answers:

```text
why does some caller care about this artifact?
```

Tags use this form:

```text
kind:key
```

Examples:

```text
entity:business-123
category:electricians
run:manual-debug
batch:2026-06-09
```

Common cache commands:

```bash
web-bot cache lookup https://example.com
web-bot cache snapshot https://example.com -o snapshot.html

web-bot cache list
web-bot cache list --tag entity:business-123
web-bot cache list --tag-kind entity

web-bot cache tags
web-bot cache tags --kind entity
web-bot cache tags https://example.com

web-bot cache tag https://example.com entity:business-123
web-bot cache untag https://example.com entity:business-123
web-bot cache delete --tag run:manual-debug
```

## Documentation

See the full CLI documentation:

* [`web-bot/README.md`](./web-bot/README.md)

## License

Copyright 2026 Colbyn Wadman

# Announcements

## 2026-06-09

AI: can be pretty dumb. 

Debugging auto generated code is like its own skill set. 

The work continues.
