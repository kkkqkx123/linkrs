//! Sync Manager
//!
//! Unified synchronization manager using SyncCoordinator.

use crate::core::types::{TransactionContextInfo, TransactionId};
use crate::core::Value;
#[cfg(feature = "fulltext-search")]
use crate::search::SyncConfig;
#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::{ChangeContext, CoordinatorError, SyncCoordinator};
use crate::sync::outbox::{OutboxEvent, OutboxPayload};
use crate::sync::sqlite_outbox::SqliteOutbox;
use crate::sync::types::ChangeType;
use dashmap::DashMap;
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
    staged_events: DashMap<TransactionId, Vec<OutboxEvent>>,
    running: Arc<std::sync::atomic::AtomicBool>,
    dead_letter_queue: Option<Arc<crate::sync::DeadLetterQueue>>,
    sqlite_outbox: Option<Arc<SqliteOutbox>>,
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
            staged_events: self.staged_events.clone(),
            running: self.running.clone(),
            dead_letter_queue: self.dead_letter_queue.clone(),
            sqlite_outbox: self.sqlite_outbox.clone(),
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
    fn stage_event(
        &self,
        txn_id: TransactionId,
        payload: OutboxPayload,
    ) -> Result<(), SyncError> {
        let target = "sync".to_string();
        let mut events = self.staged_events.entry(txn_id).or_default();
        let sequence = (events.len() as u64).saturating_add(1);
        let id = format!("{}:{}", txn_id.0, sequence);
        let ordering_key = format!("{}:default:{}", target, id);
        events.push(OutboxEvent {
            id: id.clone(),
            transaction_id: Some(txn_id),
            sequence,
            committed: false,
            retries: 0,
            created_at_ms: chrono::Utc::now().timestamp_millis().max(0) as u64,
            payload,
            target,
            partition: "default".to_string(),
            idempotency_key: id,
            enqueue_sequence: sequence,
            ordering_key,
            next_attempt_at_ms: 0,
            lease_owner: None,
            lease_until_ms: 0,
            lease_epoch: 0,
            dead_lettered: false,
            last_error: None,
        });
        Ok(())
    }

    pub fn clear_staged_transaction(&self, txn_id: TransactionId) {
        self.staged_events.remove(&txn_id);
    }

    pub fn attach_transaction_context(&self, txn_id: TransactionId) -> TransactionContextInfo {
        TransactionContextInfo::new(txn_id, 0, false, 0)
    }

    pub fn rollback_transaction_to_sequence_sync(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord
                .truncate_transaction(txn_id, sequence)
                .map_err(SyncError::from)?;
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .truncate_transaction(txn_id, sequence)
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }

        if let Some(mut events) = self.staged_events.get_mut(&txn_id) {
            events.retain(|event| event.sequence <= sequence);
        }

        Ok(())
    }

    fn new_common() -> Self {
        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            staged_events: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dead_letter_queue: None,
            sqlite_outbox: None,
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
        self.sqlite_outbox = Some(Arc::new(self.execute_sync(|| async {
            SqliteOutbox::open(&sqlite_path)
                .await
                .map_err(SyncError::PersistenceError)
        })?));
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
        crate::sync::OutboxStats {
            pending: self
                .staged_events
                .iter()
                .map(|entry| entry.value().len())
                .sum(),
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
                    match self.apply_index_mutation(&event.mutation) {
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

    fn apply_index_mutation(
        &self,
        mutation: &crate::core::wal::IndexMutation,
    ) -> Result<(), String> {
        let payload: OutboxPayload = postcard::from_bytes(&mutation.document_or_vector)
            .map_err(|error| format!("Failed to decode index mutation: {}", error))?;
        self.apply_payload(&payload)
    }

    fn apply_payload(&self, payload: &OutboxPayload) -> Result<(), String> {
        let result = match payload {
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
            } => self
                .execute_sync(|| self.apply_edge_delete_mutation(*space_id, src, dst, edge_type)),
        };
        result.map_err(|error| error.to_string())
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
        self.stage_event(
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

    pub async fn apply_vertex_mutation(
        &self,
        space_id: u64,
        tag_name: &str,
        vertex_id: &crate::core::Value,
        properties: &[(String, crate::core::Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
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

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            for (field_name, value) in properties {
                if let Some(vector) = value.as_vector() {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        tag_name,
                        field_name,
                        crate::sync::vector_sync::VectorChangeType::from(change_type),
                        crate::sync::vector_sync::VectorPointData {
                            id: format!("{}", vertex_id),
                            vector: vector.clone(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .on_vector_change(ctx)
                        .await
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
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
        self.stage_event(
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
    ) -> Result<(), SyncError> {
        self.stage_event(
            txn_id,
            OutboxPayload::EdgeDelete {
                space_id,
                src: src.clone(),
                dst: dst.clone(),
                edge_type: edge_type.to_string(),
            },
        )?;
        Ok(())
    }

    pub async fn apply_edge_insert_mutation(
        &self,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        let props: Vec<(String, crate::core::Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            for (field_name, value) in &props {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        &edge.edge_type,
                        field_name,
                        ChangeType::Insert,
                        format!("{}->{}", edge.src, edge.dst),
                        text.clone(),
                    );
                    coord.on_change(ctx).await.map_err(SyncError::from)?;
                }
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            for (field_name, value) in &props {
                if let Some(vector) = value.as_vector() {
                    if vector_coord.index_exists(space_id, &edge.edge_type, field_name) {
                        let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                            space_id,
                            &edge.edge_type,
                            field_name,
                            crate::sync::vector_sync::VectorChangeType::from(ChangeType::Insert),
                            crate::sync::vector_sync::VectorPointData {
                                id: format!("{}->{}", edge.src, edge.dst),
                                vector: vector.clone(),
                                payload: std::collections::HashMap::new(),
                            },
                        );
                        vector_coord
                            .on_vector_change(ctx)
                            .await
                            .map_err(|e| SyncError::VectorError(e.to_string()))?;
                    }
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
    ) -> Result<(), SyncError> {
        let edge_id = format!("{}->{}", src, dst);

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

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            let vector_indexes = vector_coord.list_indexes();
            for idx in vector_indexes {
                if idx.space_id == space_id && idx.tag_name == edge_type {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        edge_type,
                        &idx.field_name,
                        crate::sync::vector_sync::VectorChangeType::Delete,
                        crate::sync::vector_sync::VectorPointData {
                            id: edge_id.clone(),
                            vector: Vec::new(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .on_vector_change(ctx)
                        .await
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
                }
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

    #[cfg(feature = "qdrant")]
    pub fn on_vector_change_with_context_buffered(
        &self,
        txn_id: crate::core::types::TransactionId,
        ctx: crate::sync::vector_sync::VectorChangeContext,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .buffer_vector_change(txn_id, ctx)
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }
        Ok(())
    }

    #[cfg(feature = "qdrant")]
    pub async fn on_vector_change_with_context(
        &self,
        ctx: crate::sync::vector_sync::VectorChangeContext,
    ) -> Result<(), SyncError> {
        if self.vector_coordinator.is_none() {
            return Ok(());
        }

        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .on_vector_change(ctx)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }

        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub async fn commit_all(&self) -> Result<(), SyncError> {
        if let Some(ref coord) = self.sync_coordinator {
            coord.commit_all().await?;
        }
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub async fn prepare_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        if let Some(ref coord) = self.sync_coordinator {
            coord.prepare_transaction(txn_id).await?;
        }
        Ok(())
    }

    /// Commit transaction: flush buffered operations to external indexes.
    ///
    /// Uses a commit order that minimizes inconsistency on partial failure:
    /// 1. Validate both coordinators (prepare phase)
    /// 2. Commit vector first (external system, harder to recover from)
    /// 3. Commit fulltext second (local system, easier to rebuild)
    /// 4. If vector fails, fulltext buffer is still intact for rollback.
    /// 5. If fulltext fails after vector succeeds, vector is committed but
    ///    fulltext can be rebuilt from storage.
    pub async fn commit_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.prepare_transaction(txn_id).await?;
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .commit_transaction(txn_id)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.commit_transaction(txn_id).await?;
        }

        Ok(())
    }

    pub async fn rollback_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.rollback_transaction_to_sequence_sync(txn_id, 0)?;
        self.staged_events.remove(&txn_id);
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub fn prepare_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.prepare_transaction(txn_id))
    }

    pub fn commit_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.commit_transaction(txn_id))
    }

    pub fn rollback_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.rollback_transaction(txn_id))
    }

    pub fn transaction_intents(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<Vec<crate::core::wal::OutboxIntent>, SyncError> {
        let events = self
            .staged_events
            .get(&txn_id)
            .map(|events| events.clone())
            .unwrap_or_default();
        events
            .iter()
            .enumerate()
            .map(|(sequence, event)| event_to_intent(txn_id, sequence, event))
            .collect()
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
            for intent in intents {
                outbox
                    .activate_generation(
                        &intent.mutation.target,
                        intent.mutation.index_id,
                        intent.mutation.index_generation.get(),
                        commit_lsn,
                    )
                    .await
                    .map_err(SyncError::PersistenceError)?;
            }
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
        self.execute_sync(|| async {
            outbox
                .create_snapshot(destination)
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    pub fn verify_outbox_snapshot(snapshot: &crate::sync::OutboxSnapshot) -> Result<(), SyncError> {
        crate::sync::SqliteOutbox::verify_snapshot(snapshot).map_err(SyncError::PersistenceError)
    }

    fn execute_sync<F, Fut, T>(&self, f: F) -> Result<T, SyncError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, SyncError>>,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            return tokio::task::block_in_place(|| handle.block_on(f()));
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| {
                SyncError::Internal(format!("Failed to create sync runtime: {}", error))
            })?;
        runtime.block_on(f())
    }

    #[cfg(feature = "qdrant")]
    pub async fn commit_vector_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .commit_transaction(txn_id)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }
        Ok(())
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

fn event_to_intent(
    txn_id: crate::core::types::TransactionId,
    sequence: usize,
    event: &OutboxEvent,
) -> Result<crate::core::wal::OutboxIntent, SyncError> {
    use crate::core::types::{IdempotencyKey, IndexGeneration, OrderingKey, TargetId, VertexId};
    use crate::core::wal::{EntityRef, IndexMutation, IndexOperation, WAL_SYNC_WIRE_VERSION};

    let (index_name, entity_ref, operation) = match &event.payload {
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
                ranking: 0,
            },
            IndexOperation::Delete,
        ),
    };
    let target = TargetId::new(event.target.clone()).map_err(SyncError::PersistenceError)?;
    let ordering_key =
        OrderingKey::new(event.ordering_key.clone()).map_err(SyncError::PersistenceError)?;
    let idempotency_key =
        IdempotencyKey::new(event.idempotency_key.clone()).map_err(SyncError::PersistenceError)?;
    let intent_sequence = u32::try_from(sequence).map_err(|_| {
        SyncError::PersistenceError("Transaction intent count exceeds u32 range".to_string())
    })?;
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
            document_or_vector: postcard::to_allocvec(&event.payload).map_err(|error| {
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

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
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

    #[test]
    fn staged_events_are_accessible_via_transaction_intents() {
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
            .transaction_intents(txn_id)
            .expect("intents should be available");
        assert_eq!(intents.len(), 1);
        assert_eq!(manager.outbox_stats().pending, 1);
    }

    #[test]
    fn staged_events_cleared_on_rollback() {
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
        assert_eq!(manager.outbox_stats().pending, 1);
        manager.clear_staged_transaction(txn_id);
        assert_eq!(manager.outbox_stats().pending, 0);
    }
}
