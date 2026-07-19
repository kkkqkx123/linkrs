//! Sync Manager
//!
//! Unified synchronization manager using SyncCoordinator.

use crate::core::types::{CommitLsn, TransactionContextInfo, TransactionId};
use crate::core::Value;
#[cfg(feature = "fulltext-search")]
use crate::search::SyncConfig;
use crate::sync::checkpoint_manifest::CheckpointManifestManager;
#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::{ChangeContext, CoordinatorError, SyncCoordinator};
use crate::sync::outbox::OutboxPayload;
use crate::sync::sqlite_outbox::{OutboxSnapshot, SqliteOutbox};
use crate::sync::types::ChangeType;
use dashmap::DashMap;
#[cfg(feature = "qdrant")]
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "qdrant")]
use crate::sync::vector_sync::VectorSyncCoordinator;
#[cfg(feature = "qdrant")]
pub use vector_client::{CollectionConfig, SearchResult};

pub struct SyncManager {
    #[cfg(feature = "fulltext-search")]
    sync_coordinator: Option<Arc<SyncCoordinator>>,
    #[cfg(feature = "qdrant")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    pending_intents: DashMap<TransactionId, Vec<crate::core::wal::OutboxIntent>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    dead_letter_queue: Option<Arc<crate::sync::DeadLetterQueue>>,
    sqlite_outbox: Option<Arc<SqliteOutbox>>,
    #[cfg(feature = "qdrant")]
    vector_receiver: Option<Arc<crate::sync::VectorReceiver>>,
    outbox_consumer: Arc<OutboxConsumerConfig>,
    #[allow(clippy::type_complexity)]
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
            #[cfg(feature = "qdrant")]
            vector_coordinator: self.vector_coordinator.clone(),
            pending_intents: self.pending_intents.clone(),
            running: self.running.clone(),
            dead_letter_queue: self.dead_letter_queue.clone(),
            sqlite_outbox: self.sqlite_outbox.clone(),
            #[cfg(feature = "qdrant")]
            vector_receiver: self.vector_receiver.clone(),
            outbox_consumer: self.outbox_consumer.clone(),
            handle: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SyncManager");
        #[cfg(feature = "fulltext-search")]
        d.field("sync_coordinator", &self.sync_coordinator);
        #[cfg(feature = "qdrant")]
        d.field("vector_coordinator", &self.vector_coordinator);
        d.field("running", &self.running);
        d.finish_non_exhaustive()
    }
}

#[cfg_attr(
    not(any(feature = "fulltext-search", feature = "qdrant")),
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
        #[cfg(feature = "qdrant")]
        if self.vector_coordinator.is_some() {
            targets.push("vector");
        }
        targets
    }

    fn stage_intent(&self, txn_id: TransactionId, payload: OutboxPayload) -> Result<(), SyncError> {
        let mut intents = self.pending_intents.entry(txn_id).or_default();
        for target_name in self.delivery_target_names() {
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
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            pending_intents: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dead_letter_queue: None,
            sqlite_outbox: None,
            #[cfg(feature = "qdrant")]
            vector_receiver: None,
            outbox_consumer: Arc::new(OutboxConsumerConfig::default()),
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

    #[cfg(feature = "qdrant")]
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
        dead_letter_queue: Arc<crate::sync::DeadLetterQueue>,
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
            .execute_sync(|| async { Ok(crate::sync::verify_live_database(&sqlite_path).await) })?;
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
        self.sqlite_outbox = Some(Arc::new(outbox));
        #[cfg(feature = "qdrant")]
        {
            self.vector_receiver = Some(Arc::new(crate::sync::VectorReceiver::open(
                work_dir.join("vector_receiver"),
            )));
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

    pub fn outbox_stats(&self) -> crate::sync::OutboxStats {
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

        crate::sync::OutboxStats {
            pending: 0,
            ..Default::default()
        }
    }

    pub fn retry_outbox_sync(&self) -> Result<usize, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        self.execute_sync(|| async {
            let targets = outbox
                .delivery_targets()
                .await
                .map_err(SyncError::PersistenceError)?;
            let mut processed = 0usize;
            for target in targets {
                while processed < self.outbox_consumer.batch_size {
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
                    match self
                        .apply_index_mutation(&event.mutation, event.commit_lsn)
                        .await
                    {
                        Ok(()) => {
                            outbox
                                .acknowledge(&event)
                                .await
                                .map_err(SyncError::PersistenceError)?;
                        }
                        Err(error) => {
                            let retry_count = outbox
                                .retry_count(event.event_id)
                                .await
                                .map_err(SyncError::PersistenceError)?;
                            if retry_count.saturating_add(1) >= self.outbox_consumer.max_retries {
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
                    processed = processed.saturating_add(1);
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
            #[cfg(feature = "qdrant")]
            "vector" => {
                self.apply_vector_mutation(mutation, commit_lsn, &payload)
                    .await
            }
            _ => self.apply_payload(&payload),
        }
    }

    fn apply_payload(&self, payload: &OutboxPayload) -> Result<(), String> {
        match payload {
            OutboxPayload::Vertex {
                space_id,
                tag_name,
                vertex_id,
                properties,
                change_type,
            } => self.execute_sync(|| {
                self.apply_vertex_mutation(*space_id, tag_name, vertex_id, properties, *change_type)
            }),
            OutboxPayload::EdgeInsert { space_id, edge } => {
                self.execute_sync(|| self.apply_edge_insert_mutation(*space_id, edge))
            }
            OutboxPayload::EdgeDelete {
                space_id,
                src,
                dst,
                edge_type,
                ranking,
            } => self.execute_sync(|| {
                self.apply_edge_delete_mutation(*space_id, src, dst, edge_type, *ranking)
            }),
            OutboxPayload::CreateIndex { .. } => {
                // DDL already executed locally before the outbox event was staged.
                // Delivery acknowledges the event without reapplying it.
                Ok(())
            }
            OutboxPayload::DropIndex { .. } => {
                // Delivery is a no-op for the same reason as CreateIndex.
                Ok(())
            }
        }
        .map_err(|error| error.to_string())
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
                Self::apply_fulltext_fields(
                    manager.clone(),
                    mutation,
                    commit_lsn,
                    *space_id,
                    tag_name,
                    format!("{}", vertex_id),
                    properties.clone(),
                    matches!(change_type, ChangeType::Delete),
                )
                .await
            }
            OutboxPayload::EdgeInsert { space_id, edge } => {
                let properties = edge
                    .props
                    .iter()
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(
                    manager.clone(),
                    mutation,
                    commit_lsn,
                    *space_id,
                    &edge.edge_type,
                    edge_entity_id(&edge.src, &edge.dst, edge.ranking),
                    properties,
                    false,
                )
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
                    .map(|metadata| (metadata.field_name, Value::String(String::new())))
                    .collect::<Vec<_>>();
                Self::apply_fulltext_fields(
                    manager,
                    mutation,
                    commit_lsn,
                    *space_id,
                    edge_type,
                    edge_entity_id(src, dst, *ranking),
                    properties,
                    true,
                )
                .await
            }
            OutboxPayload::CreateIndex { .. } | OutboxPayload::DropIndex { .. } => Ok(()),
        }
    }

    #[cfg(feature = "fulltext-search")]
    async fn apply_fulltext_fields(
        manager: Arc<crate::search::manager::FulltextIndexManager>,
        mutation: &crate::core::wal::IndexMutation,
        commit_lsn: CommitLsn,
        space_id: u64,
        index_name: &str,
        entity_id: String,
        properties: Vec<(String, Value)>,
        deleted: bool,
    ) -> Result<(), String> {
        for (field_name, value) in properties {
            let Some(engine) = manager.get_engine(space_id, index_name, &field_name) else {
                continue;
            };
            let document = if deleted {
                Vec::new()
            } else if let Value::String(text) = value {
                text.as_bytes().to_vec()
            } else {
                continue;
            };
            let mut field_mutation = mutation.clone();
            field_mutation.index_id =
                stable_hash(format!("{}:{}:{}", space_id, index_name, field_name).as_bytes());
            field_mutation.document_or_vector = document;
            field_mutation.idempotency_key = crate::core::types::IdempotencyKey::new(format!(
                "{}:{}",
                mutation.idempotency_key.as_str(),
                field_name
            ))?;
            let receiver = crate::sync::receiver::FulltextReceiver::new(engine);
            let late_arrival = receiver
                .check_late_arrival(commit_lsn, field_mutation.idempotency_key.as_str())
                .await;
            if !late_arrival.accepted && !late_arrival.reason.contains("duplicate") {
                return Err(late_arrival.reason);
            }
            receiver
                .apply_index_batch(&[(&field_mutation, commit_lsn)])
                .await?;
            log::debug!(
                "Applied fulltext mutation for {} at {}",
                entity_id,
                commit_lsn
            );
        }
        Ok(())
    }

    #[cfg(feature = "qdrant")]
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
                    let (vector, vector_change_type) = match (value.as_vector(), change_type) {
                        (Some(vector), ChangeType::Insert | ChangeType::Update) => (
                            vector.to_vec(),
                            crate::sync::vector_sync::VectorChangeType::Insert,
                        ),
                        _ => (
                            Vec::new(),
                            crate::sync::vector_sync::VectorChangeType::Delete,
                        ),
                    };
                    let payload = properties
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<HashMap<_, _>>();
                    contexts.push(crate::sync::vector_sync::VectorChangeContext::new(
                        *space_id,
                        tag_name,
                        field_name,
                        vector_change_type,
                        crate::sync::vector_sync::VectorPointData {
                            id: format!("{}_{}_{}", vertex_id, tag_name, field_name),
                            vector,
                            payload,
                        },
                    ));
                }
            }
            OutboxPayload::CreateIndex { .. }
            | OutboxPayload::DropIndex { .. }
            | OutboxPayload::EdgeInsert { .. }
            | OutboxPayload::EdgeDelete { .. } => {}
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

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.start_background_tasks().await;
        }

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

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.stop_background_tasks().await;
        }

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
        space_id: u64,
        index_name: &str,
        index_type: &str,
        fields: &[(String, Value)],
        properties: &[String],
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::CreateIndex {
                space_id,
                index_name: index_name.to_string(),
                index_type: index_type.to_string(),
                fields: fields.to_vec(),
                properties: properties.to_vec(),
            },
        )?;
        Ok(())
    }

    pub fn on_index_drop(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        index_name: &str,
        index_type: &str,
    ) -> Result<(), SyncError> {
        self.stage_intent(
            txn_id,
            OutboxPayload::DropIndex {
                space_id,
                index_name: index_name.to_string(),
                index_type: index_type.to_string(),
            },
        )?;
        Ok(())
    }

    pub async fn apply_vertex_mutation(
        &self,
        space_id: u64,
        tag_name: &str,
        vertex_id: &crate::core::Value,
        properties: &[(String, crate::core::Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        #[cfg(not(feature = "fulltext-search"))]
        let _ = (space_id, tag_name, vertex_id, properties, change_type);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            for (field_name, value) in properties {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        tag_name,
                        field_name,
                        change_type,
                        format!("{}", vertex_id),
                        text.clone(),
                    );
                    coord.on_change(ctx).await.map_err(SyncError::from)?;
                }
            }
        }

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

    pub async fn apply_edge_insert_mutation(
        &self,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        let props: Vec<(String, crate::core::Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        #[cfg(not(feature = "fulltext-search"))]
        let _ = (space_id, edge);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            for (field_name, value) in &props {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        &edge.edge_type,
                        field_name,
                        ChangeType::Insert,
                        edge_entity_id(&edge.src, &edge.dst, edge.ranking),
                        text.clone(),
                    );
                    coord.on_change(ctx).await.map_err(SyncError::from)?;
                }
            }
        }

        Ok(())
    }

    pub async fn apply_edge_delete_mutation(
        &self,
        space_id: u64,
        src: &crate::core::Value,
        dst: &crate::core::Value,
        edge_type: &str,
        ranking: i64,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        let edge_id = edge_entity_id(src, dst, ranking);

        #[cfg(not(feature = "fulltext-search"))]
        let _ = (space_id, src, dst, edge_type, ranking);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            let indexes = coord
                .fulltext_manager()
                .get_space_indexes(space_id)
                .into_iter()
                .filter(|m| m.tag_name == edge_type);

            for metadata in indexes {
                let ctx = ChangeContext::new_fulltext(
                    space_id,
                    edge_type,
                    &metadata.field_name,
                    ChangeType::Delete,
                    edge_id.clone(),
                    String::new(),
                );
                coord.on_change(ctx).await.map_err(SyncError::from)?;
            }
        }

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
        self.execute_sync(|| async {
            outbox
                .materialize_commit(commit_lsn, intents, &targets)
                .await
                .map_err(SyncError::PersistenceError)
        })?;
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
    pub fn sync_diagnostics(&self) -> Result<crate::sync::SyncDiagnostics, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        self.execute_sync(|| async {
            outbox
                .diagnostics()
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    /// Create a crash-safe immutable snapshot of the SQLite projection.
    pub fn create_outbox_snapshot(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<crate::sync::OutboxSnapshot, SyncError> {
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
    ) -> Result<crate::sync::OutboxSnapshot, SyncError> {
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

    pub fn verify_outbox_snapshot(snapshot: &crate::sync::OutboxSnapshot) -> Result<(), SyncError> {
        crate::sync::SqliteOutbox::verify_snapshot(snapshot).map_err(SyncError::PersistenceError)
    }

    fn execute_sync<F, Fut, T>(&self, f: F) -> Result<T, SyncError>
    where
        F: FnOnce() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, SyncError>> + Send,
        T: Send,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            return match handle.runtime_flavor() {
                tokio::runtime::RuntimeFlavor::MultiThread => {
                    tokio::task::block_in_place(|| handle.block_on(f()))
                }
                tokio::runtime::RuntimeFlavor::CurrentThread => std::thread::scope(|scope| {
                    let join = scope.spawn(|| {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|error| SyncError::Internal(error.to_string()))?;
                        runtime.block_on(f())
                    });
                    join.join().map_err(|_| {
                        SyncError::Internal("Synchronous async operation panicked".to_string())
                    })?
                }),
                _ => handle.block_on(f()),
            };
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                SyncError::Internal(format!("Failed to create sync runtime: {}", error))
            })?;
        runtime.block_on(f())
    }

    #[cfg(feature = "fulltext-search")]
    pub fn sync_coordinator(&self) -> &Arc<SyncCoordinator> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext-search feature")
    }

    #[cfg(feature = "qdrant")]
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

    pub fn get_dead_letter_entries(&self) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all()
        } else {
            vec![]
        }
    }

    pub fn get_unrecovered_entries(&self) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered()
        } else {
            vec![]
        }
    }

    pub fn get_old_dead_letter_entries(
        &self,
        age: std::time::Duration,
    ) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_old_entries(age)
        } else {
            vec![]
        }
    }

    pub fn remove_dead_letter_entry(&self, index: usize) -> Option<crate::sync::DeadLetterEntry> {
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

    #[cfg(feature = "qdrant")]
    pub fn vector_index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord.index_exists(space_id, tag_name, field_name)
        } else {
            false
        }
    }

    #[cfg(feature = "qdrant")]
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_client::DistanceMetric,
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

    #[cfg(feature = "qdrant")]
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

    #[cfg(feature = "qdrant")]
    pub async fn search_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            let options = crate::sync::vector_sync::SearchOptions::new(
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

    let (index_name, entity_ref, operation) = match payload {
        OutboxPayload::Vertex {
            tag_name,
            vertex_id,
            change_type,
            ..
        } => (
            tag_name.as_str(),
            EntityRef::Vertex(VertexId::try_from(vertex_id).map_err(|error| {
                SyncError::PersistenceError(format!("Invalid vertex ID in outbox event: {}", error))
            })?),
            match change_type {
                ChangeType::Insert | ChangeType::Update => IndexOperation::Upsert,
                ChangeType::Delete => IndexOperation::Delete,
            },
        ),
        OutboxPayload::EdgeInsert { edge, .. } => (
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
            src,
            dst,
            edge_type,
            ranking,
            ..
        } => (
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
            index_name,
            index_type: _,
            ..
        } => (
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_int64(0)),
            IndexOperation::Upsert,
        ),
        OutboxPayload::DropIndex { index_name, .. } => (
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_int64(0)),
            IndexOperation::Delete,
        ),
    };
    let sequence = u64::from(intent_sequence).saturating_add(1);
    let id = format!("{}:{}:{}", txn_id.0, target_name, sequence);
    let target = TargetId::new(target_name.to_string()).map_err(SyncError::PersistenceError)?;
    let ordering_key = OrderingKey::new(format!("{}:default:{}", target_name, id))
        .map_err(SyncError::PersistenceError)?;
    let idempotency_key = IdempotencyKey::new(id).map_err(SyncError::PersistenceError)?;
    Ok(crate::core::wal::OutboxIntent {
        wire_version: WAL_SYNC_WIRE_VERSION,
        transaction_id: txn_id,
        intent_sequence,
        mutation: IndexMutation {
            wire_version: WAL_SYNC_WIRE_VERSION,
            target,
            index_id: stable_hash(index_name.as_bytes()),
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

fn edge_entity_id(src: impl std::fmt::Display, dst: impl std::fmt::Display, ranking: i64) -> String {
    format!("{}->{}#{}", src, dst, ranking)
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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
        match crate::sync::restore_snapshot_sync(snapshot, live_path) {
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
        return crate::sync::restore_latest_snapshot(live_path, snapshot_dir).map(Some);
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
    SyncCoordinatorError(#[from] crate::sync::coordinator::SyncCoordinatorError),

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
    use crate::sync::CheckpointManifest;
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
        );
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
                &Value::String("v1".to_string()),
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
                &Value::String("v1".to_string()),
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
            src: Value::String("src".to_string()),
            dst: Value::String("dst".to_string()),
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
        let mut config = crate::search::FulltextConfig::default();
        config.index_path = directory.path().to_path_buf();
        let fulltext_manager = Arc::new(
            crate::search::FulltextIndexManager::new(config)
                .expect("fulltext manager should be created"),
        );
        let coordinator = Arc::new(crate::sync::SyncCoordinator::new(
            fulltext_manager,
            crate::sync::BatchConfig::default(),
        ));
        let manager = SyncManager::new(coordinator);
        let txn_id = TransactionId::new(99);

        manager
            .on_vertex_change_with_txn(
                txn_id,
                1,
                "Node",
                &Value::String("v1".to_string()),
                &[("text".to_string(), Value::String("hello".to_string()))],
                ChangeType::Insert,
            )
            .expect("change should stage");

        let intents = manager
            .pending_transaction_intents(txn_id)
            .expect("intents should be available");
        assert_eq!(intents.len(), 1);
        assert_eq!(intents[0].mutation.target.as_str(), "fulltext");
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
