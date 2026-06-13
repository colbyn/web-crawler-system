//! Cache command dispatcher.
//!
//! This module owns the public `cache` subcommand shape and delegates actual
//! behavior to focused submodules.
//!
//! Keep this file thin:
//!
//! - CLI vocabulary lives here,
//! - shared parsing/output helpers live in `cache/common.rs`,
//! - metadata and payload inspection lives in `cache/lookup.rs`,
//! - snapshot export lives in `cache/snapshot.rs`,
//! - large result queries live in `cache/list.rs` and `cache/tags.rs`,
//! - destructive or mutating operations live in `cache/mutate.rs`,
//! - aggregate database facts live in `cache/stats.rs`.
//!
//! The cache command intentionally supports operator-scale output. List-style
//! commands should always expose pagination and deterministic sorting.

mod common;
mod list;
mod lookup;
mod mutate;
mod snapshot;
mod stats;
mod tags;

use clap::Subcommand;
use std::path::PathBuf;

use self::common::{
    EntryFilterArgs,
    PageArgs,
    TagFilterArgs,
};
use self::list::EntrySortArgs;
use self::tags::TagSortArgs;

#[derive(Subcommand, Debug)]
pub enum CacheCommands {
    /// Show metadata for one cached URL.
    #[command(visible_alias = "get")]
    Lookup {
        url: String,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        /// Output as JSON.
        #[arg(long)]
        json: bool,

        /// Include uncompressed payload body in JSON output.
        #[arg(long)]
        full: bool,
    },

    /// Print or save the cached HTML snapshot for one URL.
    #[command(visible_alias = "html")]
    Snapshot {
        url: String,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        /// Write the snapshot body to a file instead of stdout.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// List cached entries.
    ///
    /// Supports filtering, sorting, and pagination because this table can grow
    /// very large during broad crawler runs.
    List {
        #[command(flatten)]
        filters: EntryFilterArgs,

        #[command(flatten)]
        page: PageArgs,

        #[command(flatten)]
        sort: EntrySortArgs,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// List known tags or tags attached to one cached URL.
    Tags {
        /// Optional cached URL. If omitted, lists the global tag registry.
        url: Option<String>,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        #[command(flatten)]
        filters: TagFilterArgs,

        #[command(flatten)]
        page: PageArgs,

        #[command(flatten)]
        sort: TagSortArgs,

        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },

    /// Add tags to one cached URL.
    Tag {
        url: String,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        /// Tags in `kind:key` form.
        tags: Vec<String>,
    },

    /// Remove tags from one cached URL.
    Untag {
        url: String,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        /// Remove all tag links from this URL.
        #[arg(long)]
        all: bool,

        /// Tags in `kind:key` form.
        tags: Vec<String>,
    },

    /// Remove cached entries by URL, exact tag, or tag kind.
    #[command(visible_aliases = ["rm", "remove"])]
    Delete {
        url: Option<String>,

        /// Delete entries carrying this exact `kind:key` tag.
        #[arg(long)]
        tag: Option<String>,

        /// Delete entries carrying any tag of this kind.
        #[arg(long = "tag-kind")]
        tag_kind: Option<String>,

        /// Optional logical cache namespace.
        ///
        /// The current Postgres cache key model ignores this value.
        #[arg(long)]
        namespace: Option<String>,

        /// Skip confirmation prompt.
        #[arg(short, long)]
        force: bool,
    },

    /// Remove one exact tag association from every entry.
    ///
    /// Entries stay in the cache.
    RemoveTag {
        tag: String,

        /// Skip confirmation prompt.
        #[arg(short, long)]
        force: bool,
    },

    /// Remove all associations of one tag kind.
    ///
    /// Entries stay in the cache.
    RemoveTagKind {
        kind: String,

        /// Skip confirmation prompt.
        #[arg(short, long)]
        force: bool,
    },

    /// Clear the entire cache database.
    Clear {
        /// Skip confirmation prompt.
        #[arg(short, long)]
        force: bool,
    },

    /// Show cache statistics.
    Stats {
        /// Output as JSON.
        #[arg(long)]
        json: bool,
    },
}

pub async fn run(action: CacheCommands, database_url: &str) -> anyhow::Result<()> {
    let cache = common::CacheHandle::connect(database_url).await?;

    match action {
        CacheCommands::Lookup {
            url,
            namespace,
            json,
            full,
        } => {
            lookup::lookup_metadata(&cache, &url, namespace, json, full).await?;
        }
        CacheCommands::Snapshot {
            url,
            namespace,
            output,
        } => {
            snapshot::get_snapshot(&cache, &url, namespace, output).await?;
        }
        CacheCommands::List {
            filters,
            page,
            sort,
            json,
        } => {
            list::list_entries(&cache, filters, page, sort, json).await?;
        }
        CacheCommands::Tags {
            url,
            namespace,
            filters,
            page,
            sort,
            json,
        } => {
            tags::list_tags(&cache, url, namespace, filters, page, sort, json).await?;
        }
        CacheCommands::Tag {
            url,
            namespace,
            tags,
        } => {
            mutate::tag_url(&cache, &url, namespace, tags).await?;
        }
        CacheCommands::Untag {
            url,
            namespace,
            all,
            tags,
        } => {
            mutate::untag_url(&cache, &url, namespace, all, tags).await?;
        }
        CacheCommands::Delete {
            url,
            tag,
            tag_kind,
            namespace,
            force,
        } => {
            mutate::delete_entries(&cache, url, tag, tag_kind, namespace, force).await?;
        }
        CacheCommands::RemoveTag { tag, force } => {
            mutate::remove_tag_from_all(&cache, &tag, force).await?;
        }
        CacheCommands::RemoveTagKind { kind, force } => {
            mutate::remove_tag_kind_from_all(&cache, &kind, force).await?;
        }
        CacheCommands::Clear { force } => {
            mutate::clear_cache(&cache, database_url, force).await?;
        }
        CacheCommands::Stats { json } => {
            stats::show_stats(&cache, database_url, json).await?;
        }
    }

    Ok(())
}

