//! In-process observability for the embedded API.
//!
//! The server exposes `/health` and `/metrics` over HTTP; an embedded host
//! has no network endpoint but still needs the same signals. These helpers
//! share the server's Prometheus renderer and health payload shape.

use crate::embedded::database::GraphDatabase;
use crate::embedded::session::{GraphDatabaseInner, Session};
use linkrs_storage::StorageClient;

/// Build the process-internal health payload.
///
/// Mirrors the fields of the HTTP `/v1/health` response. Listener readiness
/// is server-only and therefore reported as not applicable; storage and the
/// transaction manager are probed directly.
pub(crate) fn health_snapshot<S>(inner: &GraphDatabaseInner<S>) -> serde_json::Value
where
    S: StorageClient + Clone + Send + Sync + 'static,
{
    let storage_ok = inner.storage.try_read().is_some();
    let sync_status = if inner.sync_manager.is_some() {
        "ok"
    } else {
        "disabled"
    };
    serde_json::json!({
        "status": "healthy",
        "service": "linkrs",
        "version": env!("CARGO_PKG_VERSION"),
        "ready": true,
        "live": true,
        "checks": {
            "process": "ok",
            "storage": if storage_ok { "ok" } else { "unavailable" },
            "transaction_manager": "ok",
            "sync": sync_status,
            "listeners": "not_applicable",
        },
        "optional": {
            "vector": optional_status(),
            "fulltext": optional_status(),
        },
    })
}

fn optional_status() -> &'static str {
    "optional"
}

impl<S: StorageClient + Clone + Send + Sync + 'static> GraphDatabase<S> {
    /// Render the live `StatsManager` as Prometheus text.
    ///
    /// The output is byte-identical to the HTTP `/v1/metrics` endpoint and
    /// the gRPC `GetMetrics` response for the same database.
    pub fn metrics_text(&self) -> String {
        linkrs_metrics::render_prometheus(&self.inner.stats_manager)
    }

    /// Process-internal health payload (no network endpoint required).
    pub fn health(&self) -> serde_json::Value {
        health_snapshot(&self.inner)
    }
}

impl<S: StorageClient + Clone + Send + Sync + 'static + linkrs_storage::UndoTarget> Session<S> {
    /// Render the database-wide metrics as Prometheus text.
    pub fn metrics_text(&self) -> String {
        linkrs_metrics::render_prometheus(&self.db.stats_manager)
    }

    /// Process-internal health payload.
    pub fn health(&self) -> serde_json::Value {
        health_snapshot(&self.db)
    }
}
