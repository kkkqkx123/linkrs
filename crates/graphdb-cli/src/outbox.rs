//! Outbox operational commands.
//!
//! Provides `graphdb outbox` sub-commands that drive the HTTP endpoints added
//! in `crates/graphdb-server/src/http/handlers/sync.rs`:
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
    /// Trigger retry of all pending outbox entries (legacy)
    Retry,
}

/// HTTP client extensions for outbox operations.
pub mod client_ext {
    use super::*;
    use crate::client::HttpClient;
    use crate::utils::error::{CliError, Result};

    impl HttpClient {
        pub async fn outbox_diagnostics(&self) -> Result<serde_json::Value> {
            let url = format!("{}/sync/outbox/diagnostics", self.base_url());
            let resp = self
                .inner()
                .get(&url)
                .send()
                .await
                .map_err(|e| CliError::connection(e.to_string()))?;
            Self::check_response(resp).await
        }

        pub async fn outbox_dead_letters(
            &self,
            target: Option<&str>,
            index_id: Option<u64>,
            generation: Option<u64>,
            limit: usize,
            offset: usize,
        ) -> Result<serde_json::Value> {
            let mut url = format!("{}/sync/outbox/dead_letters?limit={}&offset={}", self.base_url(), limit, offset);
            if let Some(t) = target {
                url.push_str(&format!("&target={}", t));
            }
            if let Some(id) = index_id {
                url.push_str(&format!("&index_id={}", id));
            }
            if let Some(gen) = generation {
                url.push_str(&format!("&generation={}", gen));
            }
            let resp = self
                .inner()
                .get(&url)
                .send()
                .await
                .map_err(|e| CliError::connection(e.to_string()))?;
            Self::check_response(resp).await
        }

        pub async fn outbox_requeue(
            &self,
            req: &RequeuePayload,
        ) -> Result<serde_json::Value> {
            let url = format!("{}/sync/outbox/requeue", self.base_url());
            let resp = self
                .inner()
                .post(&url)
                .json(req)
                .send()
                .await
                .map_err(|e| CliError::connection(e.to_string()))?;
            Self::check_response(resp).await
        }

        pub async fn outbox_degraded_ranges(
            &self,
            target: Option<&str>,
            index_id: Option<u64>,
            generation: Option<u64>,
        ) -> Result<serde_json::Value> {
            let mut url = format!("{}/sync/outbox/degraded_ranges", self.base_url());
            let mut first = true;
            if let Some(t) = target {
                url.push_str(&format!("{}target={}", if first { "?" } else { "&" }, t));
                first = false;
            }
            if let Some(id) = index_id {
                url.push_str(&format!("{}index_id={}", if first { "?" } else { "&" }, id));
                first = false;
            }
            if let Some(gen) = generation {
                url.push_str(&format!("{}generation={}", if first { "?" } else { "&" }, gen));
            }
            let resp = self
                .inner()
                .get(&url)
                .send()
                .await
                .map_err(|e| CliError::connection(e.to_string()))?;
            Self::check_response(resp).await
        }

        async fn check_response(resp: reqwest::Response) -> Result<serde_json::Value> {
            let status = resp.status();
            let text = resp.text().await.map_err(|e| CliError::connection(e.to_string()))?;
            if !status.is_success() {
                return Err(CliError::connection(format!("HTTP {}: {}", status, text)));
            }
            serde_json::from_str(&text).map_err(|e| CliError::connection(e.to_string()))
        }
    }

    #[derive(serde::Serialize)]
    pub struct RequeuePayload {
        pub target: Option<String>,
        pub index_id: Option<u64>,
        pub generation: Option<u64>,
        pub limit: Option<usize>,
        pub event_ids: Option<Vec<i64>>,
    }
}
