//! Request payloads for outbox/sync operations.
//!
//! The interactive `\sync` commands build these payloads and send them
//! through `linkrs-client`. The one-shot Clap command tree that used to live
//! here was removed: it duplicated the `\sync` surface without ever being
//! wired into the CLI, so all outbox access goes through `\sync`.

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
