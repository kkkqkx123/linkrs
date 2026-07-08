#![cfg(feature = "fulltext-search")]

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use super::buffer::OpBatchBuffer;
use super::config::BatchConfig;
use super::error::{BatchError, BatchResult};
use super::trait_def::BatchProcessor;
use crate::search::tantivy_index::TantivySearchEngine;
use crate::sync::types::{ChangeType, IndexOperation};

pub struct FulltextBatchProcessor {
    space_id: u64,
    tag_name: String,
    field_name: String,
    engine: Arc<TantivySearchEngine>,
    config: BatchConfig,
    buffer: Arc<OpBatchBuffer>,
    background_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    immediate_mode: bool,
}

impl std::fmt::Debug for FulltextBatchProcessor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FulltextBatchProcessor")
            .field(
                "location",
                &(self.space_id, &self.tag_name, &self.field_name),
            )
            .field("config", &self.config)
            .field("buffer_count", &self.buffer.total_count())
            .finish_non_exhaustive()
    }
}

impl FulltextBatchProcessor {
    pub fn new(
        space_id: u64,
        tag_name: String,
        field_name: String,
        engine: Arc<TantivySearchEngine>,
        config: BatchConfig,
    ) -> Self {
        Self {
            space_id,
            tag_name,
            field_name,
            engine,
            config,
            buffer: Arc::new(OpBatchBuffer::new()),
            background_task: Mutex::new(None),
            immediate_mode: false,
        }
    }

    pub fn new_immediate(
        space_id: u64,
        tag_name: String,
        field_name: String,
        engine: Arc<TantivySearchEngine>,
    ) -> Self {
        Self {
            space_id,
            tag_name,
            field_name,
            engine,
            config: BatchConfig::default(),
            buffer: Arc::new(OpBatchBuffer::new()),
            background_task: Mutex::new(None),
            immediate_mode: true,
        }
    }

    fn location(&self) -> (u64, String, String) {
        (
            self.space_id,
            self.tag_name.clone(),
            self.field_name.clone(),
        )
    }

    async fn execute_immediate(&self, operation: IndexOperation) -> BatchResult<()> {
        match operation.change_type {
            ChangeType::Insert | ChangeType::Update => {
                if let Some(text) = operation.text() {
                    self.engine
                        .index_batch(vec![(operation.id.clone(), text.to_string())])
                        .await
                        .map_err(BatchError::from)?;
                }
            }
            ChangeType::Delete => {
                self.engine
                    .delete_batch(vec![operation.id.as_str()])
                    .await
                    .map_err(BatchError::from)?;
            }
        }
        self.engine.commit().await.map_err(BatchError::from)?;
        Ok(())
    }

    pub async fn execute_now(&self, operations: Vec<IndexOperation>) -> BatchResult<()> {
        self.execute_now_without_commit(operations).await?;
        self.engine.commit().await.map_err(BatchError::from)?;
        Ok(())
    }

    pub async fn execute_now_without_commit(
        &self,
        operations: Vec<IndexOperation>,
    ) -> BatchResult<()> {
        let mut deletes = Vec::new();
        let mut items = Vec::new();

        for op in operations {
            match op.change_type {
                ChangeType::Delete => deletes.push(op.id.clone()),
                ChangeType::Insert | ChangeType::Update => {
                    if let Some(text) = op.text() {
                        items.push((op.id.clone(), text.to_string()));
                    }
                }
            }
        }

        if !deletes.is_empty() {
            let ids: Vec<&str> = deletes.iter().map(|s| s.as_str()).collect();
            self.engine
                .delete_batch(ids)
                .await
                .map_err(BatchError::from)?;
        }

        if !items.is_empty() {
            self.engine
                .index_batch(items)
                .await
                .map_err(BatchError::from)?;
        }

        Ok(())
    }

    pub fn engine(&self) -> &Arc<TantivySearchEngine> {
        &self.engine
    }

    pub fn buffer(&self) -> &Arc<OpBatchBuffer> {
        &self.buffer
    }

    async fn should_commit(&self, key: &(u64, String, String)) -> bool {
        if self.buffer.count(key) >= self.config.batch_size {
            return true;
        }
        self.buffer.is_timeout(key, self.config.flush_interval)
    }

    async fn execute_batch(&self, key: &(u64, String, String)) -> BatchResult<()> {
        let entry = self.buffer.peek_entry(key);

        if entry.deletes.is_empty() && entry.inserts.is_empty() {
            self.buffer.update_commit_time(key);
            return Ok(());
        }

        if !entry.deletes.is_empty() {
            let ids: Vec<&str> = entry.deletes.iter().map(|s| s.as_str()).collect();
            self.engine.delete_batch(ids).await.map_err(|e| {
                self.buffer.re_enqueue(key, entry.clone());
                BatchError::from(e)
            })?;
        }

        if !entry.inserts.is_empty() {
            let items: Vec<(String, String)> = entry
                .inserts
                .iter()
                .filter_map(|op| match op.change_type {
                    ChangeType::Insert | ChangeType::Update => {
                        op.text().map(|text| (op.id.clone(), text.to_string()))
                    }
                    _ => None,
                })
                .collect();

            if !items.is_empty() {
                self.engine.index_batch(items).await.map_err(|e| {
                    self.buffer.re_enqueue(key, entry.clone());
                    BatchError::from(e)
                })?;
            }
        }

        self.engine.commit().await.map_err(|e| {
            self.buffer.re_enqueue(key, entry.clone());
            BatchError::from(e)
        })?;

        self.buffer.drain_all(key);
        self.buffer.update_commit_time(key);

        Ok(())
    }
}

impl Drop for FulltextBatchProcessor {
    fn drop(&mut self) {
        if let Ok(handle) = self.background_task.try_lock() {
            if let Some(task) = handle.as_ref() {
                task.abort();
            }
        }
    }
}

#[async_trait]
impl BatchProcessor for FulltextBatchProcessor {
    async fn add(&self, operation: IndexOperation) -> BatchResult<()> {
        if self.immediate_mode {
            return self.execute_immediate(operation).await;
        }

        let key = self.location();

        match &operation.change_type {
            ChangeType::Insert | ChangeType::Update => {
                self.buffer.add_insert(&key, operation);
            }
            ChangeType::Delete => {
                self.buffer.add_delete(&key, operation.id.clone());
            }
        }

        if self.should_commit(&key).await {
            self.execute_batch(&key).await?;
        }

        Ok(())
    }

    async fn add_batch(&self, operations: Vec<IndexOperation>) -> BatchResult<()> {
        if self.immediate_mode {
            for operation in operations {
                self.execute_immediate(operation).await?;
            }
            return Ok(());
        }

        let key = self.location();

        for operation in operations {
            match &operation.change_type {
                ChangeType::Insert | ChangeType::Update => {
                    self.buffer.add_insert(&key, operation);
                }
                ChangeType::Delete => {
                    self.buffer.add_delete(&key, operation.id.clone());
                }
            }
        }

        if self.should_commit(&key).await {
            self.execute_batch(&key).await?;
        }

        Ok(())
    }

    async fn commit_all(&self) -> BatchResult<()> {
        let keys = self.buffer.keys();
        for key in &keys {
            self.execute_batch(key).await?;
        }
        if keys.is_empty() {
            self.engine.commit().await.map_err(BatchError::from)?;
        }
        Ok(())
    }

    async fn commit_timeout(&self) -> BatchResult<()> {
        let keys = self.buffer.keys();
        for key in keys {
            if self.buffer.is_timeout(&key, self.config.flush_interval) {
                self.execute_batch(&key).await?;
            }
        }
        Ok(())
    }

    async fn start_background_task(self: Arc<Self>) {
        if self.immediate_mode {
            return;
        }

        let mut handle = self.background_task.lock().await;
        if handle.is_some() {
            return;
        }

        let processor = self.clone();
        let interval = processor.config.flush_interval;

        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Err(e) = processor.commit_timeout().await {
                    tracing::error!("Background batch commit failed: {:?}", e);
                }
            }
        });

        *handle = Some(task);
    }

    async fn stop_background_task(&self) {
        let mut handle = self.background_task.lock().await;
        if let Some(task) = handle.take() {
            task.abort();
        }
    }
}
