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

Inspect the cache:

```bash
cargo run --release -p web-bot -- cache stats
cargo run --release -p web-bot -- cache list-by-tag run:manual-debug
```

See the full CLI documentation:

* [`web-bot/README.md`](./web-bot/README.md)

