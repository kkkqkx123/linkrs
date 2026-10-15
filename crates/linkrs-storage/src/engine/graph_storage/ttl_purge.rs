//! Background sweep that periodically reclaims TTL-expired rows.
//!
//! The sweep is opt-in: it only runs when a caller starts it with a positive
//! interval. Each pass enumerates every space and calls the shared purge
//! entry without a target schema, so it deletes exactly what
//! `PURGE EXPIRED IN <space>` deletes — same bounded batches, same per-schema
//! row counts — without waiting for an operator to issue the statement.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::thread_pool::{BackgroundTaskHandle, StorageThreadPool};

use super::context::GraphStorageContext;
use super::GraphStorage;

/// Periodic physical cleanup of TTL-expired rows.
pub(crate) struct TtlPurgeScheduler {
    thread_pool: Arc<StorageThreadPool>,
    handle: Option<BackgroundTaskHandle>,
}

impl TtlPurgeScheduler {
    pub(crate) fn new(thread_pool: Arc<StorageThreadPool>) -> Self {
        Self {
            thread_pool,
            handle: None,
        }
    }

    /// Start the periodic sweep at `interval`. A zero interval leaves it
    /// unstarted, and a second call keeps the running sweep: the first
    /// interval wins.
    pub(crate) fn start(&mut self, interval: Duration, ctx: &GraphStorageContext) {
        if self.handle.is_some() || interval.is_zero() {
            return;
        }
        // The sweep needs a storage handle, but the storage context owns
        // this scheduler. Sweeping a context clone that owns no scheduler
        // breaks that cycle, so stopping and dropping the scheduler never
        // leaves the sweep pinning the storage context alive.
        let storage = GraphStorage {
            ctx: Arc::new(ctx.clone_for_ttl_purge()),
        };
        let running = Arc::new(AtomicBool::new(false));
        let handle = self.thread_pool.spawn_periodic(
            running,
            interval,
            Duration::from_millis(10),
            move || purge_all_spaces(&storage),
        );
        self.handle = Some(handle);
    }

    pub(crate) fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.stop();
            handle.join();
        }
    }

    pub(crate) fn is_running(&self) -> bool {
        self.handle.as_ref().is_some_and(|h| h.is_running())
    }
}

impl Drop for TtlPurgeScheduler {
    fn drop(&mut self) {
        self.stop();
    }
}

/// One sweep across every space. A failing space is reported and skipped so
/// the remaining spaces still get reclaimed.
fn purge_all_spaces(storage: &GraphStorage) {
    let spaces = match storage.ctx.schema_manager().list_spaces() {
        Ok(spaces) => spaces,
        Err(error) => {
            log::warn!("TTL background purge failed to list spaces: {}", error);
            return;
        }
    };
    let mut removed_total: u64 = 0;
    for space in &spaces {
        match storage.purge_expired(&space.space_name, None) {
            Ok(entries) => {
                for entry in &entries {
                    if entry.removed > 0 {
                        removed_total += entry.removed as u64;
                        log::info!(
                            "TTL background purge removed {} row(s) from {} '{}' in space '{}'",
                            entry.removed,
                            if entry.is_edge { "edge type" } else { "tag" },
                            entry.schema_name,
                            space.space_name,
                        );
                    }
                }
            }
            Err(error) => log::warn!(
                "TTL background purge failed for space '{}': {}",
                space.space_name,
                error
            ),
        }
    }
    if let Some(stats) = storage.ctx.stats_manager() {
        stats.record_ttl_purge(1, removed_total);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PropertyGraphConfig, StorageReader, StorageSchemaOps, StorageWriter};
    use linkrs_core::types::{PropertyDef, SpaceInfo, TagInfo, VertexId};
    use linkrs_core::vertex_edge_path::Vertex;
    use linkrs_core::{DataType, Tag, Value};
    use std::collections::HashMap;

    fn ttl_storage() -> GraphStorage {
        GraphStorage::new_with_config(PropertyGraphConfig::test()).expect("in-memory storage")
    }

    fn tagged_vertex(vid: i64, ts: i64) -> Vertex {
        // `sid` is the primary key column and must mirror the vertex id.
        let mut props: HashMap<std::sync::Arc<str>, Value> = HashMap::new();
        props.insert(std::sync::Arc::<str>::from("sid"), Value::BigInt(vid));
        props.insert(std::sync::Arc::<str>::from("ts"), Value::BigInt(ts));
        Vertex::new(
            VertexId::try_from_int64(vid).expect("vertex id"),
            Tag::new("S".to_string(), props),
        )
    }

    #[test]
    fn sweep_reclaims_expired_rows_and_reports_running_state() {
        let storage = ttl_storage();
        let mut handle = storage.clone();
        let mut space = SpaceInfo::new("sweep".to_string()).with_vid_type(DataType::BigInt);
        StorageSchemaOps::create_space(&mut handle, &mut space).expect("create space");
        let tag = TagInfo::new("S".to_string())
            .with_properties(vec![
                PropertyDef::new("sid".to_string(), DataType::BigInt),
                PropertyDef::new("ts".to_string(), DataType::BigInt),
            ])
            .with_ttl(Some(100), Some("ts".to_string()));
        StorageSchemaOps::create_tag(&mut handle, "sweep", &tag).expect("create tag");
        StorageWriter::insert_vertex(&mut handle, "sweep", tagged_vertex(1, 0))
            .expect("insert expired vertex");
        StorageWriter::insert_vertex(&mut handle, "sweep", tagged_vertex(2, i64::MAX))
            .expect("insert fresh vertex");

        let ctx = &storage.ctx;
        assert!(!ctx.is_ttl_purge_running());
        // A zero interval never starts a sweep.
        ctx.start_ttl_purge(Duration::ZERO);
        assert!(!ctx.is_ttl_purge_running());

        ctx.start_ttl_purge(Duration::from_millis(20));
        assert!(ctx.is_ttl_purge_running());
        let mut visible = Vec::new();
        for _ in 0..100 {
            visible = storage
                .scan_vertices_by_tag("sweep", "S")
                .expect("scan tag");
            if visible.len() == 1 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        ctx.stop_ttl_purge();
        assert!(!ctx.is_ttl_purge_running());

        assert_eq!(visible.len(), 1, "only the expired row may be reclaimed");
        assert_eq!(visible[0].vid, tagged_vertex(2, i64::MAX).vid);
    }
}
