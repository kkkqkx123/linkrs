//! Outbox operational commands.
//!
//! Provides `linkrs outbox` sub-commands that drive the HTTP endpoints added
//! in `crates/linkrs-server/src/http/handlers/sync.rs`:
//! diagnostics / dead_letters / requeue / degraded_ranges / degraded_clear.
//! The module is intentionally self-contained so the interactive CLI can evolve
//! without coupling to the wire crate.

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
pub struct OutboxArgs {
    #[command(subcommand)]
    pub command: OutboxCommand,
}

#[derive(Subcommand, Debug)]
pub enum OutboxCommand {
    /// Show outbox diagnostics (frontier lag, degraded, dead-letter counts)
    Diagnostics {
        #[arg(long)]
        json: bool,
    },
    /// List dead letters with optional filters
    DeadLetters {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        index_id: Option<u64>,
        #[arg(long)]
        generation: Option<u64>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long, default_value_t = 0)]
        offset: usize,
    },
    /// Requeue dead letters in batch
    Requeue {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        index_id: Option<u64>,
        #[arg(long)]
        generation: Option<u64>,
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Comma-separated event IDs; when present, target/index filters are ignored
        #[arg(long, value_delimiter = ',')]
        event_ids: Option<Vec<i64>>,
    },
    /// List degraded ranges
    DegradedRanges {
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        index_id: Option<u64>,
        #[arg(long)]
        generation: Option<u64>,
    },
    /// Clear a degraded range after offline repair
    DegradedClear {
        #[arg(long)]
        target: String,
        #[arg(long)]
        index_id: u64,
        #[arg(long)]
        generation: u64,
        #[arg(long)]
        start_lsn: u64,
        #[arg(long)]
        end_lsn: u64,
    },
    /// Trigger retry of all pending outbox entries
    Retry,
}

#[derive(serde::Serialize)]
pub struct RequeuePayload {
    pub target: Option<String>,
    pub index_id: Option<u64>,
    pub generation: Option<u64>,
    pub limit: Option<usize>,
    pub event_ids: Option<Vec<i64>>,
}

#[derive(serde::Serialize)]
pub struct DegradedClearPayload {
    pub target: String,
    pub index_id: u64,
    pub generation: u64,
    pub start_lsn: u64,
    pub end_lsn: u64,
}
