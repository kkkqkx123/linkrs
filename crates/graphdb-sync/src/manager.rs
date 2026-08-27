//! Sync Manager
//!
//! Unified synchronization manager using SyncCoordinator.

use crate::core::stats::{OutboxState, StatsManager};
use crate::core::types::{CommitLsn, TransactionContextInfo, TransactionId};
use crate::core::Value;
#[cfg(feature = "fulltext-search")]
use crate::search::SyncConfig;
use crate::checkpoint_manifest::CheckpointManifestManager;
#[cfg(feature = "fulltext-search")]
use crate::coordinator::{CoordinatorError, SyncCoordinator};
use crate::outbox::OutboxPayload;
use crate::sqlite_outbox::{OutboxSnapshot, SqliteOutbox};
use crate::types::ChangeType;
use dashmap::DashMap;
#[cfg(feature = "vector")]
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

type JoinHandleGuard = Mutex<Option<tokio::task::JoinHandle<()>>>;

#[cfg(feature = "vector")]
use crate::vector_sync::VectorSyncCoordinator;

#[cfg(feature = "fulltext-search")]
struct FulltextFieldApply<'a> {
    manager: Arc<crate::search::manager::FulltextIndexManager>,
    mutation: &'a crate::core::wal::IndexMutation,
    commit_lsn: CommitLsn,
    space_id: u64,
    index_name: &'a str,
    entity_id: String,
    properties: Vec<(String, Value)>,
    deleted: bool,
}

#[derive(Debug, Clone)]
pub struct IndexCreateRequest {
    pub space_id: u64,
    pub index_name: String,
    pub schema_name: String,
    pub index_type: String,
    pub fields: Vec<(String, Value)>,
    pub properties: Vec<String>,
}
#[cfg(feature = "vector")]
pub use vector_search::{CollectionConfig, SearchResult};

pub struct SyncManager {
    #[cfg(feature = "fulltext-search")]
    sync_coordinator: Option<Arc<SyncCoordinator>>,
    #[cfg(feature = "vector")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pending_intents: DashMap<TransactionId, Vec<crate::core::wal::OutboxIntent>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    dead_letter_queue: Option<Arc<crate::DeadLetterQueue>>,
    sqlite_outbox: Option<Arc<SqliteOutbox>>,
    #[cfg(feature = "vector")]
    vector_receiver: Option<Arc<crate::VectorReceiver>>,
    outbox_consumer: Arc<OutboxConsumerConfig>,
    stats_manager: Option<Arc<StatsManager>>,
    handle: JoinHandleGuard,
}

/// Delivery policy shared by one or more sync manager workers.
#[derive(Debug, Clone)]
pub struct OutboxConsumerConfig {
    pub consumer_id: String,
    pub batch_size: usize,
    pub lease_duration_ms: u64,
    pub max_retries: u64,
}

impl Default for OutboxConsumerConfig {
    fn default() -> Self {
        Self {
            consumer_id: format!("sync-manager-{}", uuid::Uuid::new_v4()),
            batch_size: 128,
            lease_duration_ms: 30_000,
            max_retries: 16,
        }
    }
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: self.sync_coordinator.clone(),
            #[cfg(feature = "vector")]
            vector_coordinator: self.vector_coordinator.clone(),
            pending_intents: self.pending_intents.clone(),
            running: self.running.clone(),
            dead_letter_queue: self.dead_letter_queue.clone(),
            sqlite_outbox: self.sqlite_outbox.clone(),
            #[cfg(feature = "vector")]
            vector_receiver: self.vector_receiver.clone(),
            outbox_consumer: self.outbox_consumer.clone(),
            stats_manager: self.stats_manager.clone(),
            handle: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SyncManager");
        #[cfg(feature = "fulltext-search")]
        d.field("sync_coordinator", &self.sync_coordinator);
        #[cfg(feature = "vector")]
        d.field("vector_coordinator", &self.vector_coordinator);
        d.field("running", &self.running);
        d.finish_non_exhaustive()
    }
}

#[cfg_attr(
    not(any(feature = "fulltext-search", feature = "vector")),
    allow(unused_variables)
)]
impl SyncManager {
    #[allow(unused_mut)]
    fn delivery_target_names(&self) -> Vec<&'static str> {
        let mut targets = Vec::new();
        #[cfg(feature = "fulltext-search")]
        if self.sync_coordinator.is_some() {
            targets.push("fulltext");
        }
        #[cfg(feature = "vector")]
        if self.vector_coordinator.is_some() {
            targets.push("vector");
        }
        targets
    }

    fn payload_needs_vector(&self, payload: &OutboxPayload) -> bool {
        #[cfg(feature = "vector")]
        {
            let Some(coord) = self.vector_coordinator.as_ref() else {
                return false;
            };
            match payload {
                OutboxPayload::Vertex {
                    space_id,
                    tag_name,
                    properties,
                    change_type,
                    ..
                } => {
                    if matches!(change_type, ChangeType::Delete) {
                        // For deletes with empty properties, still need to fan out
                        // to all vector fields of this tag if any index exists.
                        if properties.is_empty() {
                            return coord
                                .list_indexes()
                                .iter()
                                .any(|m| m.space_id == *space_id && m.tag_name == *tag_name);
                        }
                    }
                    for (field, value) in properties {
                        if value.as_vector().is_some()
                            && coord.index_exists(*space_id, tag_name, field)
                        {
                            return true;
                        }
                    }
                    // Delete case with no matching field still handled above; for
                    // Insert/Update with vector-like values but missing index, the
                    // mutation will be filtered later in apply_vector_mutation, but
                    // we pre-filter here to avoid write amplification. If no
                    // indexed field matched, no vector intent is needed.
                    false
                }
                OutboxPayload::CreateIndex { fields, .. } => {
                    fields.iter().any(|(_, v)| v.as_vector().is_some())
                }
                OutboxPayload::DropIndex { space_id, schema_name, fields, .. } => {
                    // Drop needs vector only if any dropped field is a vector index
                    fields
                        .iter()
                        .any(|field| coord.index_exists(*space_id, schema_name, field))
                }
                OutboxPayload::EdgeInsert { .. } | OutboxPayload::EdgeDelete { .. } => false,
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = payload;
            false
        }
    }

    fn payload_needs_fulltext(&self, payload: &OutboxPayload) -> bool {
        #[cfg(feature = "fulltext-search")]
        {
            let Some(coord) = self.sync_coordinator.as_ref() else {
                return false;
            };
            let manager = coord.fulltext_manager();
            match payload {
                OutboxPayload::Vertex {
                    space_id,
                    tag_name,
                    properties,
                    change_type,
                    ..
                } => {
                    if matches!(change_type, ChangeType::Delete) && properties.is_empty() {
                        return manager
                            .get_space_indexes(*space_id)
                            .into_iter()
                            .any(|meta| meta.tag_name == *tag_name);
                    }
                    for (field, value) in properties {
                        let is_text = matches!(value, Value::String(_) | Value::FixedString(_));
                        if is_text && manager.has_index(*space_id, tag_name, field) {
                            return true;
                        }
                    }
                    false
                }
                OutboxPayload::CreateIndex { fields, .. } => fields.iter().any(|(_, v)| {
                    matches!(v, Value::String(_) | Value::FixedString(_))
                }),
                OutboxPayload::DropIndex {
                    space_id,
                    schema_name,
                    fields,
                    ..
                } => fields.iter().any(|field| {
                    manager
                        .get_space_indexes(*space_id)
                        .iter()
                        .any(|meta| meta.tag_name == *schema_name && meta.field_name == *field)
                }),
                OutboxPayload::EdgeInsert { space_id, edge } => edge.props.iter().any(
                    |(field, value)| {
                        let is_text =
                            matches!(value, Value::String(_) | Value::FixedString(_));
                        is_text
                            && manager.has_index(*space_id, &edge.edge_type, field)
                    },
                ),
                OutboxPayload::EdgeDelete {
                    space_id,
                    edge_type,
                    ..
                } => manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .any(|meta| meta.tag_name == *edge_type),
            }
        }
        #[cfg(not(feature = "fulltext-search"))]
        {
            let _ = payload;
            false
        }
    }

    fn stage_intent(&self, txn_id: TransactionId, payload: OutboxPayload) -> Result<(), SyncError> {
        // Pre-filter by durable outbox requirement: if at least one target
        // actually needs this payload and the outbox is not configured, fail fast.
        let needs_vector = self.payload_needs_vector(&payload);
        let needs_fulltext = self.payload_needs_fulltext(&payload);
        let has_needed_target = needs_vector || needs_fulltext;
        // If neither target needs the payload, skip entirely to avoid write
        // amplification for pure-graph mutations.
        if !has_needed_target {
            // Still validate outbox presence if any target *could* have been
            // needed but was filtered due to missing index: no intent needed.
            return Ok(());
        }
        if has_needed_target && self.sqlite_outbox.is_none() {
            return Err(SyncError::PersistenceError(
                "Synchronized writes require a configured durable outbox".to_string(),
            ));
        }
        let mut intents = self.pending_intents.entry(txn_id).or_default();
        for target_name in self.delivery_target_names() {
            let should_deliver = match target_name {
                "vector" => needs_vector,
                "fulltext" => needs_fulltext,
                _ => true,
            };
            if !should_deliver {
                continue;
            }
            let sequence = u32::try_from(intents.len()).map_err(|_| {
                SyncError::PersistenceError(
                    "Transaction intent count exceeds u32 range".to_string(),
                )
            })?;
            intents.push(payload_to_intent(txn_id, sequence, target_name, &payload)?);
        }
        Ok(())
    }

    pub fn clear_transaction_intents(&self, txn_id: TransactionId) {
        self.pending_intents.remove(&txn_id);
    }

    pub fn attach_transaction_context(&self, txn_id: TransactionId) -> TransactionContextInfo {
        TransactionContextInfo::new(txn_id, 0, false, 0)
    }

    pub fn rollback_transaction_to_sequence_sync(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Result<(), SyncError> {
        if let Some(mut intents) = self.pending_intents.get_mut(&txn_id) {
            intents.retain(|intent| u64::from(intent.intent_sequence) <= sequence);
        }

        Ok(())
    }

    fn new_common() -> Self {
        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: None,
            #[cfg(feature = "vector")]
            vector_coordinator: None,
            pending_intents: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dead_letter_queue: None,
            sqlite_outbox: None,
            #[cfg(feature = "vector")]
            vector_receiver: None,
            outbox_consumer: Arc::new(OutboxConsumerConfig::default()),
            stats_manager: None,
            handle: Mutex::new(None),
        }
    }

    #[cfg(feature = "fulltext-search")]
    pub fn new(sync_coordinator: Arc<SyncCoordinator>) -> Self {
        Self {
            sync_coordinator: Some(sync_coordinator),
            ..Self::new_common()
        }
    }

    pub fn new_without_fulltext() -> Self {
        Self::new_common()
    }

    #[cfg(feature = "vector")]
    pub fn with_vector_coordinator(
        mut self,
        vector_coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        self.vector_coordinator = Some(vector_coordinator);
        self
    }

    #[cfg(feature = "fulltext-search")]
    pub fn with_sync_config(
        sync_coordinator: Arc<SyncCoordinator>,
        _sync_config: SyncConfig,
    ) -> Self {
        Self::new(sync_coordinator)
    }

    pub fn with_dead_letter_queue(
        mut self,
        dead_letter_queue: Arc<crate::DeadLetterQueue>,
    ) -> Self {
        self.dead_letter_queue = Some(dead_letter_queue);
        self
    }

    pub fn configure_outbox(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), SyncError> {
        let path = path.as_ref();
        let sqlite_path = if path
            .extension()
            .is_some_and(|extension| extension == "sqlite")
        {
            path.to_path_buf()
        } else {
            path.with_extension("sqlite")
        };

        let database_parent = sqlite_path.parent().unwrap_or(Path::new("."));
        let snapshot_dir = database_parent
            .parent()
            .unwrap_or(database_parent)
            .join("outbox_snapshots");
        let work_dir = database_parent.parent().unwrap_or(database_parent);
        let preferred_snapshot =
            latest_manifest_outbox_snapshot(work_dir).map_err(SyncError::PersistenceError)?;

        // Validate the live database before opening it. A file can exist while
        // still being an incomplete or corrupt SQLite projection after a crash.
        // Restore the snapshot referenced by the latest valid combined
        // checkpoint first, then fall back to the directory-wide snapshot scan.
        let live_is_healthy = self
            .execute_sync(|| async { Ok(crate::verify_live_database(&sqlite_path).await) })?;
        if !live_is_healthy {
            match restore_outbox_from_candidates(
                &sqlite_path,
                &snapshot_dir,
                preferred_snapshot.as_ref(),
            ) {
                Ok(Some(lsn)) => {
                    log::info!("Recovered outbox from snapshot at LSN {}", lsn.get());
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn!(
                        "Outbox recovery attempted but failed: {}. Starting fresh.",
                        error
                    );
                }
            }
        }

        let open_outbox = || {
            self.execute_sync(|| async {
                SqliteOutbox::open(&sqlite_path)
                    .await
                    .map_err(SyncError::PersistenceError)
            })
        };
        let outbox = match open_outbox() {
            Ok(outbox) => outbox,
            Err(error) if snapshot_dir.is_dir() => {
                log::warn!(
                    "Failed to open outbox {}: {}. Restoring the latest snapshot.",
                    sqlite_path.display(),
                    error
                );
                restore_outbox_from_candidates(
                    &sqlite_path,
                    &snapshot_dir,
                    preferred_snapshot.as_ref(),
                )
                .map_err(SyncError::PersistenceError)?
                .ok_or_else(|| {
                    SyncError::PersistenceError(format!(
                        "No valid outbox snapshot found in {}",
                        snapshot_dir.display()
                    ))
                })?;
                open_outbox()?
            }
            Err(error) => return Err(error),
        };
        let outbox_arc = Arc::new(outbox);
        self.sqlite_outbox = Some(outbox_arc.clone());
        #[cfg(feature = "vector")]
        {
            self.vector_receiver = Some(Arc::new(crate::VectorReceiver::open(
                work_dir.join("vector_receiver"),
            )));
            if let Some(coord) = self.vector_coordinator.as_ref() {
                coord.set_outbox(outbox_arc.clone());
            }
        }
        Ok(())
    }

    pub fn configure_outbox_consumer(&mut self, config: OutboxConsumerConfig) {
        self.outbox_consumer = Arc::new(OutboxConsumerConfig {
            consumer_id: if config.consumer_id.is_empty() {
                OutboxConsumerConfig::default().consumer_id
            } else {
                config.consumer_id
            },
            batch_size: config.batch_size.max(1),
            lease_duration_ms: config.lease_duration_ms.max(1_000),
            max_retries: config.max_retries.max(1),
        });
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn set_stats_manager(&mut self, stats_manager: Arc<StatsManager>) {
        self.stats_manager = Some(stats_manager);
    }

    pub fn outbox_stats(&self) -> crate::OutboxStats {
        if let Some(outbox) = &self.sqlite_outbox {
            let outbox = outbox.clone();
            let stats = std::thread::Builder::new()
                .name("outbox-stats".to_string())
                .spawn(move || {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| SyncError::Internal(error.to_string()))?;
                    runtime.block_on(async move {
                        outbox.stats().await.map_err(SyncError::PersistenceError)
                    })
                })
                .ok()
                .and_then(|handle| handle.join().ok())
                .and_then(Result::ok);
            if let Some(stats) = stats {
                return stats;
            }
        }

        crate::OutboxStats {
            pending: 0,
            ..Default::default()
        }
    }

    pub fn retry_outbox_sync(&self) -> Result<usize, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        let stats_manager = self.stats_manager.clone();
        self.execute_sync(|| async {
            let targets = outbox
                .delivery_targets()
                .await
                .map_err(SyncError::PersistenceError)?;
            let mut processed = 0usize;
            for target in targets {
                // Each delivery target owns its full batch budget so a hot
                // target cannot starve the others until the next poll cycle.
                let mut target_processed = 0usize;
                while target_processed < self.outbox_consumer.batch_size {
                    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
                    let Some(event) = outbox
                        .claim_next(
                            &target,
                            &self.outbox_consumer.consumer_id,
                            now,
                            self.outbox_consumer.lease_duration_ms,
                        )
                        .await
                        .map_err(SyncError::PersistenceError)?
                    else {
                        break;
                    };
                    let apply_started = std::time::Instant::now();
                    match self
                        .apply_index_mutation(&event.mutation, event.commit_lsn)
                        .await
                    {
                        Ok(()) => {
                            if let Some(stats) = &stats_manager {
                                stats.record_transport_latency(
                                    apply_started.elapsed().as_millis() as u64
                                );
                            }
                            outbox
                                .acknowledge(&event)
                                .await
                                .map_err(SyncError::PersistenceError)?;
                        }
                        Err(error) => {
                            if let Some(stats) = &stats_manager {
                                stats.record_transport_latency(
                                    apply_started.elapsed().as_millis() as u64
                                );
                            }
                            // Qdrant disabled events are retained for retry after
                            // engine recovery and must not be dead-lettered even after
                            // max_retries. Detect by the EngineDisabled message.
                            let is_disabled_error = error.contains("Vector engine is disabled")
                                || error.contains("EngineDisabled");
                            if is_disabled_error {
                                // Fixed 5s backoff for disabled, not counting toward
                                // dead-letter threshold.
                                let backoff = 5_000u64;
                                outbox
                                    .retry(&event, now.saturating_add(backoff), &error)
                                    .await
                                    .map_err(SyncError::PersistenceError)?;
                            } else {
                                let retry_count = outbox
                                    .retry_count(event.event_id)
                                    .await
                                    .map_err(SyncError::PersistenceError)?;
                                if retry_count.saturating_add(1)
                                    >= self.outbox_consumer.max_retries
                                {
                                    outbox
                                        .dead_letter(&event, now, &error)
                                        .await
                                        .map_err(SyncError::PersistenceError)?;
                                } else {
                                    let backoff = 100u64
                                        .saturating_mul(1u64 << retry_count.min(16))
                                        .min(300_000);
                                    outbox
                                        .retry(&event, now.saturating_add(backoff), &error)
                                        .await
                                        .map_err(SyncError::PersistenceError)?;
                                }
                            }
                        }
                    }
                    processed = processed.saturating_add(1);
                    target_processed = target_processed.saturating_add(1);
                }
            }
            if let Some(stats) = &stats_manager {
                let durable = outbox.stats().await.map_err(SyncError::PersistenceError)?;
                let diagnostics = outbox
                    .diagnostics()
                    .await
                    .map_err(SyncError::PersistenceError)?;
                let frontier_lag = diagnostics
                    .targets
                    .iter()
                    .map(|target| target.frontier_lag)
                    .chain(diagnostics.indexes.iter().map(|index| index.frontier_lag))
                    .max()
                    .unwrap_or(0);
                let degraded = diagnostics.targets.iter().any(|target| target.degraded)
                    || diagnostics.indexes.iter().any(|index| index.degraded);
                stats.record_outbox_state(OutboxState {
                    pending: durable.pending as u64,
                    retries: durable.retries,
                    dead_lettered: durable.dead_lettered as u64,
                    leased: durable.leased as u64,
                    oldest_event_age_ms: durable.oldest_event_age_ms,
                    frontier_lag,
                    degraded,
                });
                // Per-target lag for granular alerting.
                for target in &diagnostics.targets {
                    stats.record_target_frontier_lag(&target.target, target.frontier_lag);
                }
                for index in &diagnostics.indexes {
                    let label = format!("{}:{}", index.target, index.index_id);
                    stats.record_target_frontier_lag(&label, index.frontier_lag);
                }
                #[cfg(feature = "vector")]
                if let Some(coord) = self.vector_coordinator.as_ref() {
                    stats.record_vector_disabled_skips(coord.disabled_skip_count());
                }
            }
            Ok(processed)
        })
    }

    async fn apply_index_mutation(
        &self,
        mutation: &crate::core::wal::IndexMutation,
        commit_lsn: CommitLsn,
    ) -> Result<(), String> {
        let payload: OutboxPayload = postcard::from_bytes(&mutation.document_or_vector)
            .map_err(|error| format!("Failed to decode index mutation: {}", error))?;
        match mutation.target.as_str() {
            #[cfg(feature = "fulltext-search")]
            "fulltext" => {
                self.apply_fulltext_mutation(mutation, commit_lsn, &payload)
                    .await
            }
            #[cfg(feature = "vector")]
            "vector" => {
                self.apply_vector_mutation(mutation, commit_lsn, &payload)
                    .await
            }
            _ => self.apply_payload(&payload),
        }
    }

    fn apply_payload(&self, payload: &OutboxPayload) -> Result<(), String> {
        match payload {
            OutboxPayload::CreateIndex { .. } | OutboxPayload::DropIndex { .. } => Ok(()),
            OutboxPayload::Vertex { .. }
            | OutboxPayload::EdgeInsert { .. }
            | OutboxPayload::EdgeDelete { .. } => Err(
                "Outbox mutation has no registered target receiver; direct delivery is disabled"
                    .to_string(),
            ),
        }
    }

    #[cfg(feature = "fulltext-search")]
    async fn apply_fulltext_mutation(
        &self,
        mutation: &crate::core::wal::IndexMutation,
        commit_lsn: CommitLsn,
        payload: &OutboxPayload,
    ) -> Result<(), String> {
        let manager = self
            .sync_coordinator
            .as_ref()
            .ok_or_else(|| "fulltext target is not configured".to_string())?
            .fulltext_manager()
            .clone();

        match payload {
            OutboxPayload::Vertex {
                space_id,
                tag_name,
                vertex_id,
                properties,
                change_type,
            } => {
                let properties =
                    if matches!(change_type, ChangeType::Delete) && properties.is_empty() {
                        manager
                            .get_space_indexes(*space_id)
                            .into_iter()
                            .filter(|metadata| metadata.tag_name == *tag_name)
                            .map(|metadata| (metadata.field_name, Value::string("")))
                            .collect()
                    } else {
                        properties.clone()
                    };
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager: manager.clone(),
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: tag_name,
                    entity_id: format!("{}", vertex_id),
                    properties,
                    deleted: matches!(change_type, ChangeType::Delete),
                })
                .await
            }
            OutboxPayload::EdgeInsert { space_id, edge } => {
                let properties = edge
                    .props
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager: manager.clone(),
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: &edge.edge_type,
                    entity_id: edge_entity_id(edge.src, edge.dst, edge.ranking),
                    properties,
                    deleted: false,
                })
                .await
            }
            OutboxPayload::EdgeDelete {
                space_id,
                src,
                dst,
                edge_type,
                ranking,
            } => {
                let properties = manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .filter(|metadata| metadata.tag_name == *edge_type)
                    .map(|metadata| (metadata.field_name, Value::string("")))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(FulltextFieldApply {
                    manager,
                    mutation,
                    commit_lsn,
                    space_id: *space_id,
                    index_name: edge_type,
                    entity_id: edge_entity_id(src, dst, *ranking),
                    properties,
                    deleted: true,
                })
                .await
            }
            OutboxPayload::CreateIndex {
                space_id,
                index_name,
                schema_name,
                fields,
                ..
            } => {
                for (field_name, value_type) in fields {
                    if !matches!(value_type, Value::String(_) | Value::FixedString(_)) {
                        continue;
                    }
                    if manager
                        .create_index(*space_id, schema_name, field_name, None)
                        .await
                        .is_err()
                        && !manager.has_index(*space_id, schema_name, field_name)
                    {
                        return Err(format!(
                            "Failed to create fulltext receiver index {}.{}.{}",
                            space_id, index_name, field_name
                        ));
                    }
                }
                Ok(())
            }
            OutboxPayload::DropIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                ..
            } => {
                let indexes = manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .filter(|metadata| metadata.tag_name == *schema_name)
                    .map(|metadata| metadata.field_name)
                    .collect::<Vec<_>>();
                for field_name in indexes {
                    manager
                        .drop_index(*space_id, schema_name, &field_name)
                        .await
                        .map_err(|error| error.to_string())?;
                }
                Ok(())
            }
        }
    }

    #[cfg(feature = "fulltext-search")]
    async fn apply_fulltext_fields(request: FulltextFieldApply<'_>) -> Result<(), String> {
        for (field_name, value) in request.properties {
            let Some(engine) =
                request
                    .manager
                    .get_engine(request.space_id, request.index_name, &field_name)
            else {
                continue;
            };
            let document = if request.deleted {
                Vec::new()
            } else if let Value::String(text) = value {
                text.as_bytes().to_vec()
            } else {
                continue;
            };
            let mut field_mutation = request.mutation.clone();
            field_mutation.index_id = stable_hash(
                format!("{}:{}:{}", request.space_id, request.index_name, field_name).as_bytes(),
            );
            field_mutation.document_or_vector = document;
            field_mutation.idempotency_key = crate::core::types::IdempotencyKey::new(format!(
                "{}:{}",
                request.mutation.idempotency_key.as_str(),
                field_name
            ))?;
            let receiver = crate::receiver::FulltextReceiver::new(engine);
            let late_arrival = receiver
                .check_late_arrival(request.commit_lsn, field_mutation.idempotency_key.as_str())
                .await;
            if !late_arrival.accepted && !late_arrival.reason.contains("duplicate") {
                return Err(late_arrival.reason);
            }
            receiver
                .apply_index_batch(&[(&field_mutation, request.commit_lsn)])
                .await?;
            log::debug!(
                "Applied fulltext mutation for {} at {}",
                request.entity_id.as_str(),
                request.commit_lsn
            );
        }
        Ok(())
    }

    #[cfg(feature = "vector")]
    async fn apply_vector_mutation(
        &self,
        mutation: &crate::core::wal::IndexMutation,
        commit_lsn: CommitLsn,
        payload: &OutboxPayload,
    ) -> Result<(), String> {
        let Some(coordinator) = self.vector_coordinator.as_ref() else {
            return Err("vector target is not configured".to_string());
        };
        let Some(receiver) = self.vector_receiver.as_ref() else {
            return Err("vector receiver is not configured".to_string());
        };
        let late_arrival = receiver
            .check_late_arrival(commit_lsn, mutation.idempotency_key.as_str())
            .await;
        if !late_arrival.accepted && !late_arrival.reason.contains("duplicate") {
            return Err(late_arrival.reason);
        }
        if !late_arrival.accepted {
            return Ok(());
        }

        let mut contexts = Vec::new();
        match payload {
            OutboxPayload::Vertex {
                space_id,
                tag_name,
                vertex_id,
                properties,
                change_type,
            } => {
                for (field_name, value) in properties {
                    if !coordinator.index_exists(*space_id, tag_name, field_name) {
                        continue;
                    }
                    let (vector, vector_change_type) = match (value.as_vector(), change_type) {
                        (Some(vector), ChangeType::Insert | ChangeType::Update) => (
                            vector.to_vec(),
                            crate::vector_sync::VectorChangeType::Insert,
                        ),
                        _ => (
                            Vec::new(),
                            crate::vector_sync::VectorChangeType::Delete,
                        ),
                    };
                    let payload = properties
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<HashMap<_, _>>();
                    contexts.push(crate::vector_sync::VectorChangeContext::new(
                        *space_id,
                        tag_name,
                        field_name,
                        vector_change_type,
                        crate::vector_sync::VectorPointData {
                            id: format_vector_point_id(vertex_id, tag_name, field_name),
                            vector,
                            payload,
                        },
                    ));
                }
                if matches!(change_type, ChangeType::Delete) {
                    let staged_fields = properties
                        .iter()
                        .map(|(field_name, _)| field_name.as_str())
                        .collect::<std::collections::HashSet<_>>();
                    for metadata in coordinator.list_indexes() {
                        if metadata.space_id == *space_id
                            && metadata.tag_name == *tag_name
                            && !staged_fields.contains(metadata.field_name.as_str())
                        {
                            contexts.push(crate::vector_sync::VectorChangeContext::new(
                                *space_id,
                                tag_name,
                                &metadata.field_name,
                                crate::vector_sync::VectorChangeType::Delete,
                                crate::vector_sync::VectorPointData {
                                    id: format_vector_point_id(
                                        vertex_id,
                                        tag_name,
                                        &metadata.field_name,
                                    ),
                                    vector: Vec::new(),
                                    payload: HashMap::new(),
                                },
                            ));
                        }
                    }
                }
            }
            OutboxPayload::CreateIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                fields,
                ..
            } => {
                for (field_name, value_type) in fields {
                    let Some(vector_size) = value_type.as_vector().map(|vector| vector.len())
                    else {
                        continue;
                    };
                    coordinator
                        .create_vector_index(
                            *space_id,
                            schema_name,
                            field_name,
                            vector_size,
                            vector_search::DistanceMetric::default(),
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            OutboxPayload::DropIndex {
                space_id,
                index_name: _index_name,
                schema_name,
                fields,
                ..
            } => {
                for field_name in fields {
                    coordinator
                        .drop_vector_index(*space_id, schema_name, field_name)
                        .await
                        .map_err(|error| error.to_string())?;
                }
            }
            OutboxPayload::EdgeInsert { .. } => {
                // Edge mutations do not trigger vector index updates.
                // Vector indexes are maintained per-vertex; edge-only changes
                // do not carry vector fields and are not indexed.
            }
            OutboxPayload::EdgeDelete { .. } => {
                // Same as EdgeInsert — edge deletions do not affect vector indexes.
            }
        }
        if !contexts.is_empty() {
            coordinator
                .on_vector_change_batch(contexts)
                .await
                .map_err(|error| error.to_string())?;
        }
        receiver
            .record_application(commit_lsn, mutation.idempotency_key.as_str())
            .await
    }

    pub async fn start(&self) -> Result<(), SyncError> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        if self.sqlite_outbox.is_some() {
            let mut handle = self.handle.lock().await;
            if handle.is_none() {
                let manager = self.clone();
                *handle = Some(tokio::spawn(async move {
                    let retry_interval = std::time::Duration::from_secs(5);
                    while manager.running.load(std::sync::atomic::Ordering::SeqCst) {
                        if let Err(error) = manager.retry_outbox_sync() {
                            tracing::warn!("Outbox delivery attempt failed: {}", error);
                        }
                        tokio::time::sleep(retry_interval).await;
                    }
                }));
            }
        }

        Ok(())
    }

    pub async fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        if let Some(handle) = self.handle.lock().await.take() {
            let _ = handle.await;
        }
    }

    pub fn on_vertex_change_with_txn(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        tag_name: &str,
        vertex_id: &Value,
        properties: &[(String, Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::Vertex {
                space_id,
                tag_name: tag_name.to_string(),
                vertex_id: vertex_id.clone(),
                properties: properties.to_vec(),
                change_type,
            },
        )?;
        Ok(())
    }

    pub fn on_index_create(
        &self,
        txn_id: TransactionId,
        request: IndexCreateRequest,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::CreateIndex {
                space_id: request.space_id,
                index_name: request.index_name,
                schema_name: request.schema_name,
                index_type: request.index_type,
                fields: request.fields,
                properties: request.properties,
            },
        )?;
        Ok(())
    }

    pub fn on_index_drop(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        index_name: &str,
        schema_name: &str,
        index_type: &str,
        fields: &[String],
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::DropIndex {
                space_id,
                index_name: index_name.to_string(),
                schema_name: schema_name.to_string(),
                index_type: index_type.to_string(),
                fields: fields.to_vec(),
            },
        )?;
        Ok(())
    }

    pub fn on_edge_insert(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::EdgeInsert {
                space_id,
                edge: edge.clone(),
            },
        )?;
        Ok(())
    }

    pub fn on_edge_delete(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
        ranking: i64,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::EdgeDelete {
                space_id,
                src: src.clone(),
                dst: dst.clone(),
                edge_type: edge_type.to_string(),
                ranking,
            },
        )?;
        Ok(())
    }

    pub fn on_edge_update(
        &self,
        _txn_id: TransactionId,
        _space_id: u64,
        _edge: EdgeRef<'_>,
        _props: EdgeProps<'_>,
    ) -> Result<(), SyncError> {
        Ok(())
    }

    pub async fn rollback_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.rollback_transaction_to_sequence_sync(txn_id, 0)?;
        self.pending_intents.remove(&txn_id);
        Ok(())
    }

    pub fn rollback_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.rollback_transaction(txn_id))
    }

    pub fn pending_transaction_intents(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<Vec<crate::core::wal::OutboxIntent>, SyncError> {
        Ok(self
            .pending_intents
            .get(&txn_id)
            .map(|intents| intents.clone())
            .unwrap_or_default())
    }

    /// Return the last staged intent sequence for a transaction.
    ///
    /// Savepoints retain intents whose sequence is less than or equal to the
    /// saved boundary, so an empty transaction maps to boundary zero.
    pub fn pending_transaction_intent_sequence(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> u64 {
        self.pending_intents
            .get(&txn_id)
            .and_then(|intents| {
                intents
                    .last()
                    .map(|intent| u64::from(intent.intent_sequence))
            })
            .unwrap_or(0)
    }

    pub fn materialize_committed_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
        commit_lsn: crate::core::types::CommitLsn,
        intents: &[crate::core::wal::OutboxIntent],
    ) -> Result<(), SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(());
        };
        let mut targets = intents
            .iter()
            .map(|intent| intent.mutation.target.clone())
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        let materializer_started = std::time::Instant::now();
        self.execute_sync(|| async {
            outbox
                .materialize_commit(commit_lsn, intents, &targets)
                .await
                .map_err(SyncError::PersistenceError)
        })?;
        if let Some(stats) = &self.stats_manager {
            stats.record_materializer_latency(materializer_started.elapsed().as_millis() as u64);
        }
        log::debug!(
            "Materialized transaction {} at commit LSN {}",
            txn_id,
            commit_lsn
        );
        Ok(())
    }

    /// Return the durable projection frontier used as the lower bound for
    /// committed-WAL replay after an outbox restore.
    pub fn outbox_materialized_lsn(
        &self,
    ) -> Result<Option<crate::core::types::CommitLsn>, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(None);
        };
        self.execute_sync(|| async {
            outbox
                .materialized_lsn()
                .await
                .map(Some)
                .map_err(SyncError::PersistenceError)
        })
    }

    /// Return durable outbox delivery and index-generation diagnostics.
    pub fn sync_diagnostics(&self) -> Result<crate::SyncDiagnostics, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let mut diagnostics = self.execute_sync(|| async {
            outbox
                .diagnostics()
                .await
                .map_err(SyncError::PersistenceError)
        })?;
        #[cfg(feature = "vector")]
        if let Some(coord) = self.vector_coordinator.as_ref() {
            diagnostics.vector_disabled_skips = coord.disabled_skip_count();
        }
        Ok(diagnostics)
    }

    /// Create a crash-safe immutable snapshot of the SQLite projection.
    pub fn create_outbox_snapshot(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<crate::OutboxSnapshot, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let destination = destination.as_ref().to_path_buf();
        self.execute_sync(|| async {
            outbox
                .create_snapshot(destination)
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    pub fn wait_for_minimum_lsn(
        &self,
        target: &crate::core::types::TargetId,
        index_id: u64,
        generation: u64,
        minimum_lsn: crate::core::types::CommitLsn,
        timeout_ms: u64,
    ) -> Result<bool, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        self.execute_sync(|| async {
            outbox
                .wait_for_minimum_lsn(target, index_id, generation, minimum_lsn, timeout_ms)
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    pub fn create_checkpoint_outbox_snapshot(
        &self,
    ) -> Result<crate::OutboxSnapshot, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let materialized_lsn = self.execute_sync(|| async {
            outbox
                .materialized_lsn()
                .await
                .map_err(SyncError::PersistenceError)
        })?;
        let database_parent = outbox.path().parent().ok_or_else(|| {
            SyncError::PersistenceError("SQLite outbox path has no parent".to_string())
        })?;
        let work_dir = database_parent.parent().unwrap_or(database_parent);
        let destination = work_dir
            .join("outbox_snapshots")
            .join(format!("outbox_snapshot_{}.sqlite", materialized_lsn.get()));
        self.create_outbox_snapshot(destination)
    }

    pub fn verify_outbox_snapshot(snapshot: &crate::OutboxSnapshot) -> Result<(), SyncError> {
        crate::SqliteOutbox::verify_snapshot(snapshot).map_err(SyncError::PersistenceError)
    }

    fn execute_sync<F, Fut, T>(&self, f: F) -> Result<T, SyncError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, SyncError>> + Send,
        T: Send,
    {
        crate::runtime::block_on_ambient(f())
            .map_err(|error| SyncError::Internal(error.to_string()))?
    }

    #[cfg(feature = "fulltext-search")]
    pub fn sync_coordinator(&self) -> &Arc<SyncCoordinator> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext-search feature")
    }

    #[cfg(feature = "vector")]
    pub fn vector_coordinator(&self) -> Option<&Arc<VectorSyncCoordinator>> {
        self.vector_coordinator.as_ref()
    }

    #[cfg(feature = "fulltext-search")]
    pub fn fulltext_manager(&self) -> Arc<crate::search::manager::FulltextIndexManager> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext-search feature")
            .fulltext_manager()
            .clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_dead_letter_entries(&self) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all()
        } else {
            vec![]
        }
    }

    pub fn get_unrecovered_entries(&self) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered()
        } else {
            vec![]
        }
    }

    pub fn get_old_dead_letter_entries(
        &self,
        age: std::time::Duration,
    ) -> Vec<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_old_entries(age)
        } else {
            vec![]
        }
    }

    pub fn remove_dead_letter_entry(&self, index: usize) -> Option<crate::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.remove(index)
        } else {
            None
        }
    }

    pub fn get_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all().len()
        } else {
            0
        }
    }

    pub fn get_unrecovered_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered().len()
        } else {
            0
        }
    }

    #[cfg(feature = "vector")]
    pub fn vector_index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord.index_exists(space_id, tag_name, field_name)
        } else {
            false
        }
    }

    #[cfg(feature = "vector")]
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_search::DistanceMetric,
    ) -> Result<String, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "vector")]
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .drop_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "vector")]
    pub async fn search_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            let options = crate::vector_sync::SearchOptions::new(
                space_id,
                tag_name,
                field_name,
                vector.to_vec(),
                top_k,
            );
            vector_coord
                .search_with_options(options)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }
}

fn payload_to_intent(
    txn_id: crate::core::types::TransactionId,
    intent_sequence: u32,
    target_name: &str,
    payload: &OutboxPayload,
) -> Result<crate::core::wal::OutboxIntent, SyncError> {
    use crate::core::types::{IdempotencyKey, IndexGeneration, OrderingKey, TargetId, VertexId};
    use crate::core::wal::{EntityRef, IndexMutation, IndexOperation, WAL_SYNC_WIRE_VERSION};

    let (space_id, index_name, entity_ref, operation) = match payload {
        OutboxPayload::Vertex {
            space_id,
            tag_name,
            vertex_id,
            change_type,
            ..
        } => (
            *space_id,
            tag_name.as_str(),
            EntityRef::Vertex(VertexId::try_from(vertex_id).map_err(|error| {
                SyncError::PersistenceError(format!("Invalid vertex ID in outbox event: {}", error))
            })?),
            match change_type {
                ChangeType::Insert | ChangeType::Update => IndexOperation::Upsert,
                ChangeType::Delete => IndexOperation::Delete,
            },
        ),
        OutboxPayload::EdgeInsert { space_id, edge } => (
            *space_id,
            edge.edge_type.as_str(),
            EntityRef::Edge {
                src: edge.src,
                dst: edge.dst,
                edge_type: stable_hash(edge.edge_type.as_bytes()) as u32,
                ranking: edge.ranking,
            },
            IndexOperation::Upsert,
        ),
        OutboxPayload::EdgeDelete {
            space_id,
            src,
            dst,
            edge_type,
            ranking,
            ..
        } => (
            *space_id,
            edge_type.as_str(),
            EntityRef::Edge {
                src: VertexId::try_from(src).map_err(|error| {
                    SyncError::PersistenceError(format!(
                        "Invalid edge source ID in outbox event: {}",
                        error
                    ))
                })?,
                dst: VertexId::try_from(dst).map_err(|error| {
                    SyncError::PersistenceError(format!(
                        "Invalid edge destination ID in outbox event: {}",
                        error
                    ))
                })?,
                edge_type: stable_hash(edge_type.as_bytes()) as u32,
                ranking: *ranking,
            },
            IndexOperation::Delete,
        ),
        OutboxPayload::CreateIndex {
            space_id,
            index_name,
            ..
        } => (
            *space_id,
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_int64(0)),
            IndexOperation::Upsert,
        ),
        OutboxPayload::DropIndex {
            space_id,
            index_name,
            ..
        } => (
            *space_id,
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_int64(0)),
            IndexOperation::Delete,
        ),
    };
    let sequence = u64::from(intent_sequence).saturating_add(1);
    let id = format!("{}:{}:{}", txn_id.0, target_name, sequence);
    let target = TargetId::new(target_name.to_string()).map_err(SyncError::PersistenceError)?;
    // Entity-scoped ordering key so concurrent updates to the same graph
    // entity are serialized by the SQLite `NOT EXISTS (ordering_key)` fence
    // in `claim_next`. The previous per-event `default:{txn}:{seq}` made every
    // ordering_key unique, so the fence was vacuously true. Now all events
    // for the same logical entity share one key: {target}:{space}:{index}:{entity}.
    let ordering_key = {
        let entity_str = match payload {
            OutboxPayload::Vertex { vertex_id, .. } => format!("{}", vertex_id),
            OutboxPayload::EdgeInsert { edge, .. } => {
                format!("{}->{}#{}", edge.src, edge.dst, edge.ranking)
            }
            OutboxPayload::EdgeDelete {
                src,
                dst,
                ranking,
                ..
            } => format!("{}->{}#{}", src, dst, ranking),
            OutboxPayload::CreateIndex { index_name, .. }
            | OutboxPayload::DropIndex { index_name, .. } => {
                // DDL is per-index; serialize all DDL for the same index.
                format!("ddl:{}", index_name)
            }
        };
        let key = format!("{}:{}:{}:{}", target_name, space_id, index_name, entity_str);
        OrderingKey::new(key).map_err(SyncError::PersistenceError)?
    };
    let idempotency_key = IdempotencyKey::new(id).map_err(SyncError::PersistenceError)?;
    Ok(crate::core::wal::OutboxIntent {
        wire_version: WAL_SYNC_WIRE_VERSION,
        transaction_id: txn_id,
        intent_sequence,
        mutation: IndexMutation {
            wire_version: WAL_SYNC_WIRE_VERSION,
            target,
            index_id: stable_hash(
                format!("{}:{}:{}", target_name, space_id, index_name).as_bytes(),
            ),
            index_generation: IndexGeneration::new(1),
            entity_ref,
            operation,
            document_or_vector: postcard::to_allocvec(payload).map_err(|error| {
                SyncError::PersistenceError(format!(
                    "Failed to serialize target mutation: {}",
                    error
                ))
            })?,
            idempotency_key,
            ordering_key,
        },
    })
}

#[cfg(feature = "fulltext-search")]
fn edge_entity_id(
    src: impl std::fmt::Display,
    dst: impl std::fmt::Display,
    ranking: i64,
) -> String {
    format!("{}->{}#{}", src, dst, ranking)
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Index IDs are persisted in SQLite INTEGER columns, so keep the
    // deterministic hash within the signed 64-bit range.
    hash & (i64::MAX as u64)
}

fn format_vector_point_id(vertex_id: &crate::core::Value, tag: &str, field: &str) -> String {
    let raw = format!("{}", vertex_id);
    // Escape the delimiter '#' and the escape char '%' inside the vertex id
    // so that decoding remains unambiguous across Local and Qdrant backends.
    // Percent-encoding is minimal and deterministic; '%' is escaped first to
    // avoid double-encoding.
    let encoded = raw.replace('%', "%25").replace('#', "%23");
    format!("{}#{}#{}", encoded, tag, field)
}

fn latest_manifest_outbox_snapshot(work_dir: &Path) -> Result<Option<OutboxSnapshot>, String> {
    let manifest_manager =
        CheckpointManifestManager::new(work_dir.join("checkpoint").join("manifests"));
    let Some(manifest) = manifest_manager.load_latest()? else {
        return Ok(None);
    };
    Ok(manifest.outbox_snapshot.map(|snapshot| OutboxSnapshot {
        path: snapshot.path,
        size_bytes: snapshot.size_bytes,
        checksum: snapshot.checksum,
        materialized_lsn: snapshot.materialized_lsn,
    }))
}

fn restore_outbox_from_candidates(
    live_path: &Path,
    snapshot_dir: &Path,
    preferred_snapshot: Option<&OutboxSnapshot>,
) -> Result<Option<CommitLsn>, String> {
    let mut preferred_error = None;
    if let Some(snapshot) = preferred_snapshot {
        match crate::restore_snapshot_sync(snapshot, live_path) {
            Ok(()) => return Ok(Some(snapshot.materialized_lsn)),
            Err(error) => {
                log::warn!(
                    "Failed to restore manifest-referenced outbox snapshot {}: {}",
                    snapshot.path.display(),
                    error
                );
                preferred_error = Some(error);
            }
        }
    }

    if snapshot_dir.is_dir() {
        return crate::restore_latest_snapshot(live_path, snapshot_dir).map(Some);
    }

    preferred_error.map_or(Ok(None), Err)
}

#[derive(Debug, Clone)]
pub struct EdgeRef<'a> {
    pub src: &'a Value,
    pub dst: &'a Value,
    pub edge_type: &'a str,
}

impl<'a> EdgeRef<'a> {
    pub fn new(src: &'a Value, dst: &'a Value, edge_type: &'a str) -> Self {
        Self {
            src,
            dst,
            edge_type,
        }
    }

    pub fn id(&self) -> String {
        format!("{}->{}", self.src, self.dst)
    }
}

#[derive(Debug, Clone)]
pub struct EdgeProps<'a> {
    pub old: &'a [(String, Value)],
    pub new: &'a [(String, Value)],
}

impl<'a> EdgeProps<'a> {
    pub fn new(old: &'a [(String, Value)], new: &'a [(String, Value)]) -> Self {
        Self { old, new }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[cfg(feature = "fulltext-search")]
    #[error("Coordinator error: {0}")]
    CoordinatorError(#[from] CoordinatorError),

    #[cfg(feature = "fulltext-search")]
    #[error("Sync coordinator error: {0}")]
    SyncCoordinatorError(#[from] crate::coordinator::SyncCoordinatorError),

    #[error("Buffer error: {0}")]
    BufferError(String),

    #[error("Vector error: {0}")]
    VectorError(String),

    #[error("Persistence error: {0}")]
    PersistenceError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CheckpointManifest;
    use tempfile::TempDir;

    async fn create_test_snapshots(root: &Path) -> Vec<OutboxSnapshot> {
        let source = root.join("source.sqlite");
        let outbox = SqliteOutbox::open(&source)
            .await
            .expect("source outbox should open");
        let snapshot_dir = root.join("outbox_snapshots");
        let mut snapshots = Vec::new();
        for lsn in [100, 200] {
            outbox
                .materialize_commit(CommitLsn::new(lsn), &[], &[])
                .await
                .expect("source outbox should materialize");
            snapshots.push(
                outbox
                    .create_snapshot(snapshot_dir.join(format!("outbox_snapshot_{lsn}.sqlite")))
                    .await
                    .expect("outbox snapshot should be created"),
            );
        }
        snapshots
    }

    fn publish_test_manifest(root: &Path, snapshot: &OutboxSnapshot) {
        let storage_path = root.join("checkpoint/checkpoint_1");
        std::fs::create_dir_all(&storage_path).expect("storage checkpoint should exist");
        std::fs::write(storage_path.join("checkpoint.meta"), b"checkpoint")
            .expect("storage checkpoint metadata should exist");
        let manifest = CheckpointManifest::new(
            1,
            snapshot.materialized_lsn,
            CheckpointManifest::storage_snapshot_from_directory(&storage_path, 1, 0, 0)
                .expect("storage snapshot reference should be created"),
            Some(CheckpointManifest::outbox_snapshot_from(snapshot)),
            Vec::new(),
        )
        .expect("checkpoint manifest should be created");
        CheckpointManifestManager::new(root.join("checkpoint/manifests"))
            .publish(&manifest)
            .expect("checkpoint manifest should publish");
    }

    #[test]
    fn pending_intents_are_available_before_commit() {
        let manager = SyncManager::new_without_fulltext();
        let txn_id = TransactionId::new(77);
        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[],
                ChangeType::Insert,
            )
            .expect("event should stage");
        let intents = manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available");
        assert!(intents.is_empty());
        assert_eq!(manager.outbox_stats().pending, 0);
    }

    #[test]
    fn pending_intents_are_cleared_on_rollback() {
        let manager = SyncManager::new_without_fulltext();
        let txn_id = TransactionId::new(88);
        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[],
                ChangeType::Insert,
            )
            .expect("event should stage");
        assert!(manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available")
            .is_empty());
        manager.clear_transaction_intents(txn_id);
        assert_eq!(manager.outbox_stats().pending, 0);
    }

    #[test]
    fn edge_delete_intent_preserves_parallel_edge_ranking() {
        let payload = OutboxPayload::EdgeDelete {
            space_id: 1,
            src: Value::string("src"),
            dst: Value::string("dst"),
            edge_type: "KNOWS".to_string(),
            ranking: 7,
        };
        let intent = payload_to_intent(TransactionId::new(101), 0, "fulltext", &payload)
            .expect("edge delete intent should be serializable");

        match intent.mutation.entity_ref {
            crate::core::wal::EntityRef::Edge { ranking, .. } => assert_eq!(ranking, 7),
            other => panic!("expected edge entity reference, got {other:?}"),
        }
    }

    #[cfg(feature = "fulltext-search")]
    #[test]
    fn fulltext_changes_use_only_the_fulltext_target() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let config = crate::search::FulltextConfig {
            index_path: directory.path().to_path_buf(),
            ..Default::default()
        };
        let fulltext_manager = Arc::new(
            crate::search::FulltextIndexManager::new(config)
                .expect("fulltext manager should be created"),
        );
        let coordinator = Arc::new(crate::SyncCoordinator::new(
            fulltext_manager,
            crate::BatchConfig::default(),
        ));
        let manager = SyncManager::new(coordinator);
        let outbox_path = directory.path().join("outbox/outbox.sqlite");
        let mut manager = manager;
        manager
            .configure_outbox(outbox_path)
            .expect("test outbox should be configured");
        let txn_id = TransactionId::new(99);

        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::string("v1"),
                &[("text".to_string(), Value::string("hello"))],
                ChangeType::Insert,
            )
            .expect("change should stage");

        let intents = manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available");
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].mutation.target.as_str(), "fulltext");
    }

    #[cfg(feature = "fulltext-search")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fulltext_outbox_claim_apply_and_restart_receipt_are_end_to_end() {
        let directory = TempDir::new().expect("temporary index directory should be created");
        let config = crate::search::FulltextConfig {
            index_path: directory.path().join("indexes"),
            ..Default::default()
        };
        let fulltext_manager = Arc::new(
            crate::search::FulltextIndexManager::new(config)
                .expect("fulltext manager should be created"),
        );
        fulltext_manager
            .create_index(1, "Node", "text", None)
            .await
            .expect("receiver index should be created");
        let coordinator = Arc::new(crate::SyncCoordinator::new(
            fulltext_manager.clone(),
            crate::BatchConfig::default(),
        ));
        let mut manager = SyncManager::new(coordinator);
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should be configured");
        let transaction_id = TransactionId::new(1001);
        manager
            .on_vertex_change_with_txn(
                transaction_id,
                1,
                "Node",
                &Value::string("node-1"),
                &[("text".to_string(), Value::string("durable graph event"))],
                ChangeType::Insert,
            )
            .expect("change should stage");
        let intents = manager
            .pending_transaction_intents(transaction_id)
            .expect("staged intents should load");
        manager
            .materialize_committed_transaction(transaction_id, CommitLsn::new(10), &intents)
            .expect("commit should materialize");
        manager.clear_transaction_intents(transaction_id);
        assert_eq!(manager.retry_outbox_sync().expect("claim should apply"), 1);
        let results = fulltext_manager
            .search(1, "Node", "text", "durable", 10)
            .await
            .expect("search should see the applied event");
        assert_eq!(results.len(), 1);

        let recovered = crate::receiver::FulltextReceiver::new(
            fulltext_manager
                .get_engine(1, "Node", "text")
                .expect("receiver engine should exist"),
        );
        let duplicate = recovered
            .check_late_arrival(CommitLsn::new(10), "1001:fulltext:1:text")
            .await;
        assert!(
            !duplicate.accepted,
            "field-specific receipt should reject a duplicate"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_prefers_manifest_snapshot_over_newer_directory_snapshot() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        publish_test_manifest(directory.path(), &snapshots[0]);

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should recover from the manifest snapshot");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(100))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_verifies_and_restores_corrupt_live_database() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        publish_test_manifest(directory.path(), &snapshots[0]);
        let live_path = directory.path().join("outbox/outbox.sqlite");
        std::fs::create_dir_all(live_path.parent().expect("live parent should exist"))
            .expect("live parent should be created");
        std::fs::write(&live_path, b"corrupt sqlite").expect("live database should be corrupt");

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(&live_path)
            .expect("corrupt outbox should be restored");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(100))
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configure_outbox_falls_back_when_manifest_snapshot_checksum_is_invalid() {
        let directory = TempDir::new().expect("temporary directory should be created");
        let snapshots = create_test_snapshots(directory.path()).await;
        std::fs::write(&snapshots[0].path, b"corrupt snapshot")
            .expect("manifest snapshot should be corrupt");
        publish_test_manifest(directory.path(), &snapshots[0]);

        let mut manager = SyncManager::new_without_fulltext();
        manager
            .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
            .expect("outbox should fall back to a valid snapshot");

        assert_eq!(
            manager
                .outbox_materialized_lsn()
                .expect("outbox frontier should load"),
            Some(CommitLsn::new(200))
        );
    }
}
