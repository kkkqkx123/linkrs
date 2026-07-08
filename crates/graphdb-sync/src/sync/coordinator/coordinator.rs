#![cfg(feature = "fulltext-search")]

use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use tracing::{debug, warn};

use super::types::{ChangeContext, ChangeData, ChangeType};
use crate::core::stats::StatsManager;
use crate::search::engine::ConsistencyState;
use crate::search::manager::FulltextIndexManager;
use crate::search::SyncFailurePolicy;
use crate::sync::batch::{
    BatchConfig, BatchProcessor, FulltextBatchProcessor, TransactionBatchBuffer,
};

use crate::sync::dead_letter_queue::{DeadLetterEntry, DeadLetterQueue, DeadLetterQueueConfig};
use crate::sync::retry::{default_local_retry_config, with_retry};
use crate::sync::types::{IndexOpKey, IndexOperation};

type FulltextProcessor = FulltextBatchProcessor;

pub struct SyncCoordinator {
    fulltext_manager: Arc<FulltextIndexManager>,
    fulltext_processors: DashMap<(u64, String, String), Arc<FulltextProcessor>>,
    transaction_buffers: DashMap<crate::core::types::TransactionId, Arc<TransactionBatchBuffer>>,
    config: BatchConfig,
    dead_letter_queue: Arc<DeadLetterQueue>,
    stats_manager: Option<Arc<StatsManager>>,
}

impl std::fmt::Debug for SyncCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SyncCoordinator");
        d.field("fulltext_processors_count", &self.fulltext_processors.len());
        d.field("config", &self.config);
        d.finish_non_exhaustive()
    }
}

impl SyncCoordinator {
    pub fn new(fulltext_manager: Arc<FulltextIndexManager>, config: BatchConfig) -> Self {
        let dead_letter_queue = Arc::new(DeadLetterQueue::new(DeadLetterQueueConfig::default()));

        Self {
            fulltext_manager,
            fulltext_processors: DashMap::new(),
            transaction_buffers: DashMap::new(),
            config,
            dead_letter_queue,
            stats_manager: None,
        }
    }

    pub fn with_stats_manager(mut self, stats_manager: Arc<StatsManager>) -> Self {
        self.stats_manager = Some(stats_manager);
        self
    }

    pub fn dead_letter_queue(&self) -> &Arc<DeadLetterQueue> {
        &self.dead_letter_queue
    }

    pub fn fulltext_manager(&self) -> &Arc<FulltextIndexManager> {
        &self.fulltext_manager
    }

    fn get_or_create_fulltext_processor(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<Arc<FulltextProcessor>> {
        let key = (space_id, tag_name.to_string(), field_name.to_string());

        // Use entry API for atomic get-or-create to avoid race conditions
        // where concurrent requests for the same (space, tag, field) could
        // cause duplicate processor creation and multiple background tasks.
        let processor = match self.fulltext_processors.entry(key.clone()) {
            dashmap::mapref::entry::Entry::Occupied(entry) => {
                return Some(entry.get().clone());
            }
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                let engine = self
                    .fulltext_manager
                    .get_engine(space_id, tag_name, field_name)?;

                let processor = Arc::new(FulltextBatchProcessor::new(
                    space_id,
                    tag_name.to_string(),
                    field_name.to_string(),
                    engine,
                    self.config.clone(),
                ));

                entry.insert(processor.clone());
                processor
            }
        };

        // Only spawn background task if we are the thread that created the processor.
        // Other threads will find the existing entry via the Occupied path above.
        let proc_clone = processor.clone();
        tokio::spawn(async move {
            proc_clone.start_background_task().await;
        });

        Some(processor)
    }

    pub async fn on_change(&self, ctx: ChangeContext) -> Result<(), SyncCoordinatorError> {
        let start = Instant::now();
        let operation = self.create_operation(&ctx)?;

        let result = async {
            if let Some(processor) =
                self.get_or_create_fulltext_processor(ctx.space_id, &ctx.tag_name, &ctx.field_name)
            {
                processor.add(operation).await?;
            }
            Ok::<(), SyncCoordinatorError>(())
        }
        .await;

        if let Some(ref sm) = self.stats_manager {
            let latency_ms = start.elapsed().as_millis() as u64;
            sm.record_sync_operation(latency_ms, result.is_ok());
            if result.is_err() {
                sm.record_sync_error();
            }
        }

        result
    }

    fn create_operation(
        &self,
        ctx: &ChangeContext,
    ) -> Result<IndexOperation, SyncCoordinatorError> {
        let ChangeData::Fulltext(ref text) = ctx.data;
        let key = IndexOpKey::new(ctx.space_id, &ctx.tag_name, &ctx.field_name);

        let text = if ctx.change_type == ChangeType::Delete {
            None
        } else {
            Some(text.clone())
        };

        Ok(IndexOperation::new_fulltext(
            key,
            ctx.change_type,
            ctx.vertex_id.clone(),
            text,
        ))
    }

    pub async fn on_vertex_change(
        &self,
        space_id: u64,
        tag_name: &str,
        vertex_id: &crate::core::Value,
        properties: &[(String, crate::core::Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncCoordinatorError> {
        let vid_str = format!("{}", vertex_id);

        for (field_name, value) in properties {
            if self
                .fulltext_manager
                .get_engine(space_id, tag_name, field_name)
                .is_some()
            {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        tag_name,
                        field_name,
                        change_type,
                        vid_str.clone(),
                        text.clone(),
                    );
                    self.on_change(ctx).await?;
                }
            }
        }

        Ok(())
    }

    /// Buffered index operations
    pub fn buffer_operation(
        &self,
        txn_id: crate::core::types::TransactionId,
        ctx: ChangeContext,
    ) -> Result<(), SyncCoordinatorError> {
        self.buffer_operation_with_sequence(txn_id, 0, ctx)
    }

    pub fn buffer_operation_with_sequence(
        &self,
        txn_id: crate::core::types::TransactionId,
        sequence: u64,
        ctx: ChangeContext,
    ) -> Result<(), SyncCoordinatorError> {
        // Creating Index Operations
        let operation = self.create_operation(&ctx)?;

        // Getting or creating a transaction buffer
        let buffer = self
            .transaction_buffers
            .entry(txn_id)
            .or_insert_with(|| Arc::new(TransactionBatchBuffer::new()))
            .clone();

        // Adding operations to the buffer
        buffer
            .prepare_with_sequence(txn_id, sequence, operation)
            .map_err(SyncCoordinatorError::BatchError)?;

        if let Some(ref sm) = self.stats_manager {
            let total_depth: u64 = self
                .transaction_buffers
                .iter()
                .map(|entry| entry.value().pending_count(*entry.key()) as u64)
                .sum();
            sm.set_sync_queue_depth(total_depth);
        }

        Ok(())
    }

    pub fn current_sequence(&self, txn_id: crate::core::types::TransactionId) -> u64 {
        self.transaction_buffers
            .get(&txn_id)
            .map(|buffer| buffer.pending_sequence(txn_id))
            .unwrap_or(0)
    }

    /// Get the count of buffered operations for a transaction
    pub fn transaction_buffer_count(&self, txn_id: crate::core::types::TransactionId) -> usize {
        if let Some(buffer) = self.transaction_buffers.get(&txn_id) {
            buffer.pending_count(txn_id)
        } else {
            0
        }
    }

    /// Prepare phase: validate all target engines are consistent
    pub async fn prepare_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncCoordinatorError> {
        if let Some(buffer) = self.transaction_buffers.get(&txn_id) {
            let count = buffer.pending_count(txn_id);

            let grouped_ops = buffer
                .peek_operations(txn_id)
                .map_err(SyncCoordinatorError::BatchError)?;

            for (key, _) in &grouped_ops {
                if let Some(engine) =
                    self.fulltext_manager
                        .get_engine(key.space_id, &key.tag_name, &key.field_name)
                {
                    if engine.consistency_state() == ConsistencyState::Inconsistent {
                        return Err(SyncCoordinatorError::InvalidOperation(format!(
                            "Engine for {}.{}.{} is marked Inconsistent. Repair before committing.",
                            key.space_id, key.tag_name, key.field_name
                        )));
                    }
                }
            }

            log::debug!(
                "Transaction {:?} prepared with {} operations across {} indexes (engines consistent)",
                txn_id,
                count,
                grouped_ops.len()
            );
        }
        Ok(())
    }

    /// Commit phase: Apply all operations with batch optimization.
    ///
    /// Uses deferred-commit pattern for atomicity:
    ///   1. Pre-validate all target engines are consistent
    ///   2. Apply all operations without committing (in-memory)
    ///   3. Commit all engines at once
    ///   4. On any failure, mark affected engines as Inconsistent
    pub async fn commit_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncCoordinatorError> {
        let result = if let Some((_, buffer)) = self.transaction_buffers.remove(&txn_id) {
            let grouped_ops = buffer
                .take_operations(txn_id)
                .map_err(SyncCoordinatorError::BatchError)?;

            // Pre-commit validation: check consistency of all involved engines
            for (key, _) in &grouped_ops {
                if let Some(engine) =
                    self.fulltext_manager
                        .get_engine(key.space_id, &key.tag_name, &key.field_name)
                {
                    if engine.consistency_state() == ConsistencyState::Inconsistent {
                        return Err(SyncCoordinatorError::InvalidOperation(format!(
                            "Engine for {}.{}.{} is marked Inconsistent. Repair required before committing.",
                            key.space_id, key.tag_name, key.field_name
                        )));
                    }
                }
            }

            // Apply fulltext operations WITHOUT committing (deferred commit)
            let mut fulltext_failed = false;
            let mut failed_keys: Vec<IndexOpKey> = Vec::new();

            for (key, operations) in &grouped_ops {
                let retry_config = default_local_retry_config();

                if let Some(processor) = self.get_or_create_fulltext_processor(
                    key.space_id,
                    &key.tag_name,
                    &key.field_name,
                ) {
                    let ops_clone = operations.clone();
                    let retry_config_clone = retry_config.clone();
                    let dlq_clone = self.dead_letter_queue.clone();

                    match with_retry(
                        || async {
                            processor
                                .execute_now_without_commit(ops_clone.clone())
                                .await
                        },
                        &retry_config_clone,
                    )
                    .await
                    {
                        Ok(_) => {
                            debug!("Fulltext batch applied (pending commit)");
                        }
                        Err(e) => {
                            warn!("Fulltext batch failed after retries: {:?}", e);
                            for op in operations {
                                let entry = DeadLetterEntry::new(
                                    op.clone(),
                                    format!("Local index sync failed after retries: {:?}", e),
                                    retry_config_clone.max_retries,
                                );
                                dlq_clone.add(entry);
                            }
                            fulltext_failed = true;
                            failed_keys.push(key.clone());
                        }
                    }
                }
            }

            if fulltext_failed {
                match self.config.failure_policy {
                    SyncFailurePolicy::FailClosed => {
                        return Err(SyncCoordinatorError::BatchError(
                            crate::sync::batch::BatchError::InvalidOperation(format!(
                                "Fulltext commit failed for {} indexes under FailClosed policy",
                                failed_keys.len()
                            )),
                        ));
                    }
                    SyncFailurePolicy::FailOpen => {
                        // Mark failed engines as inconsistent so they can be rebuilt
                        for key in &failed_keys {
                            if let Some(engine) = self.fulltext_manager.get_engine(
                                key.space_id,
                                &key.tag_name,
                                &key.field_name,
                            ) {
                                engine.mark_inconsistent();
                                warn!(
                                    "Marked engine {}.{}.{} as Inconsistent due to commit failure",
                                    key.space_id, key.tag_name, key.field_name
                                );
                            }
                        }
                        // Still try to commit the successful batch processors
                        self.commit_all().await.ok();
                        return Err(SyncCoordinatorError::BatchError(
                            crate::sync::batch::BatchError::InvalidOperation(format!(
                                "Fulltext commit failed for {} indexes, marked as Inconsistent. Repair with rebuild_index.",
                                failed_keys.len()
                            )),
                        ));
                    }
                }
            }

            // Commit all fulltext engines at once
            self.commit_all().await?;

            log::debug!(
                "Transaction {:?} committed successfully ({} fulltext batches)",
                txn_id,
                grouped_ops.len(),
            );
            if let Some(ref sm) = self.stats_manager {
                let total_depth: u64 = self
                    .transaction_buffers
                    .iter()
                    .map(|entry| entry.value().pending_count(*entry.key()) as u64)
                    .sum();
                sm.set_sync_queue_depth(total_depth);
            }
            Ok(())
        } else {
            Ok(())
        };

        result
    }

    pub async fn commit_all(&self) -> Result<(), SyncCoordinatorError> {
        let processors: Vec<Arc<FulltextProcessor>> = self
            .fulltext_processors
            .iter()
            .map(|e| e.value().clone())
            .collect();

        for processor in &processors {
            processor.commit_all().await.map_err(|e| {
                SyncCoordinatorError::BatchError(crate::sync::batch::BatchError::InvalidOperation(
                    format!("commit_all failed: {:?}", e),
                ))
            })?;
        }

        Ok(())
    }

    /// Rollback transaction: discard buffered operations
    pub fn rollback_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncCoordinatorError> {
        if let Some(buffer) = self.transaction_buffers.get(&txn_id) {
            buffer.rollback(txn_id)?;
        }
        self.transaction_buffers.remove(&txn_id);
        Ok(())
    }

    pub fn truncate_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
        sequence: u64,
    ) -> Result<(), SyncCoordinatorError> {
        if let Some(buffer) = self.transaction_buffers.get(&txn_id) {
            buffer
                .truncate_operations(txn_id, sequence)
                .map_err(SyncCoordinatorError::BatchError)?;
        }
        Ok(())
    }

    pub async fn stop_background_tasks(&self) {
        let processors: Vec<Arc<FulltextProcessor>> = self
            .fulltext_processors
            .iter()
            .map(|e| e.value().clone())
            .collect();

        for processor in &processors {
            processor.stop_background_task().await;
        }
    }

    pub async fn start_background_tasks(&self) {
        let processors: Vec<Arc<FulltextProcessor>> = self
            .fulltext_processors
            .iter()
            .map(|e| e.value().clone())
            .collect();

        for processor in &processors {
            let p = processor.clone();
            tokio::spawn(async move {
                p.start_background_task().await;
            });
        }
    }

    /// Recover operations from the dead letter queue.
    pub async fn recover_dead_letter(&self) -> Result<RecoveryResult, SyncCoordinatorError> {
        let dead_letter_entries = self.dead_letter_queue.get_all();
        let mut result = RecoveryResult {
            total: dead_letter_entries.len(),
            ..Default::default()
        };

        for (index, entry) in dead_letter_entries.iter().enumerate() {
            match self.recover_operation(&entry.operation).await {
                Ok(()) => {
                    self.dead_letter_queue.mark_recovered(index);
                    result.recovered += 1;
                }
                Err(e) => {
                    warn!(
                        "Failed to recover dead letter entry at index {}: {:?}",
                        index, e
                    );
                    result.failed += 1;
                }
            }
        }

        result.total_attempted = result.total;
        Ok(result)
    }

    /// Recover operations from the dead letter queue by retrying each one.
    ///
    /// This retries each failed operation through the normal batch processing pipeline.
    /// Successfully retried operations are marked as recovered in the DLQ.
    pub async fn retry_dead_letter(
        &self,
        max_entries: Option<usize>,
    ) -> Result<RecoveryResult, SyncCoordinatorError> {
        let all_entries = self.dead_letter_queue.get_all();
        let mut result = RecoveryResult::default();
        let mut processed = 0;

        for (index, entry) in all_entries.iter().enumerate() {
            if entry.recovered {
                continue;
            }
            if let Some(limit) = max_entries {
                if processed >= limit {
                    break;
                }
            }
            result.total += 1;

            match self.recover_operation(&entry.operation).await {
                Ok(()) => {
                    self.dead_letter_queue.mark_recovered(index);
                    result.recovered += 1;
                }
                Err(e) => {
                    warn!("Dead letter retry failed at index {}: {:?}", index, e);
                    result.failed += 1;
                }
            }
            processed += 1;
        }

        result.total_attempted = processed;
        Ok(result)
    }

    /// Recover operations from specific indexes
    pub async fn recover_operations_for_indexes(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<RecoveryResult, SyncCoordinatorError> {
        let target_key = IndexOpKey::new(space_id, tag_name, field_name);
        let entries = self.dead_letter_queue.get_all();
        let mut result = RecoveryResult::default();

        for (index, entry) in entries.iter().enumerate() {
            let matches = entry.operation.key == target_key;

            if matches {
                result.total += 1;
                match self.recover_operation(&entry.operation).await {
                    Ok(()) => {
                        self.dead_letter_queue.mark_recovered(index);
                        result.recovered += 1;
                    }
                    Err(e) => {
                        warn!(
                            "Failed to recover operation for {}.{}.{}: {:?}",
                            space_id, tag_name, field_name, e
                        );
                        result.failed += 1;
                    }
                }
            }
        }

        result.total_attempted = result.total;
        Ok(result)
    }

    /// Recover a single operation
    async fn recover_operation(
        &self,
        operation: &IndexOperation,
    ) -> Result<(), SyncCoordinatorError> {
        let key = &operation.key;
        if let Some(processor) =
            self.get_or_create_fulltext_processor(key.space_id, &key.tag_name, &key.field_name)
        {
            match operation.change_type {
                ChangeType::Insert => {
                    processor.add(operation.clone()).await.map_err(|e| {
                        SyncCoordinatorError::BatchError(
                            crate::sync::batch::BatchError::InvalidOperation(format!(
                                "Recovery failed: {:?}",
                                e
                            )),
                        )
                    })?;
                }
                ChangeType::Update | ChangeType::Delete => {
                    processor
                        .execute_now(vec![operation.clone()])
                        .await
                        .map_err(|e| {
                            SyncCoordinatorError::BatchError(
                                crate::sync::batch::BatchError::InvalidOperation(format!(
                                    "Recovery failed: {:?}",
                                    e
                                )),
                            )
                        })?;
                }
            }
        }

        Ok(())
    }
}

/// Result of a recovery operation
#[derive(Debug, Default)]
pub struct RecoveryResult {
    pub total: usize,
    pub recovered: usize,
    pub failed: usize,
    pub total_attempted: usize,
}

impl RecoveryResult {
    pub fn total(&self) -> usize {
        self.total
    }

    pub fn recovered(&self) -> usize {
        self.recovered
    }

    pub fn failed(&self) -> usize {
        self.failed
    }

    pub fn is_complete(&self) -> bool {
        self.failed == 0
    }
}

/// Error types for the sync coordinator
#[derive(Debug, thiserror::Error)]
pub enum SyncCoordinatorError {
    #[error("Batch error: {0}")]
    BatchError(crate::sync::batch::BatchError),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Operation not supported: {0}")]
    NotSupported(String),

    #[error("Recovery error: {0}")]
    RecoveryError(String),
}

impl From<crate::sync::batch::BatchError> for SyncCoordinatorError {
    fn from(e: crate::sync::batch::BatchError) -> Self {
        SyncCoordinatorError::BatchError(e)
    }
}
