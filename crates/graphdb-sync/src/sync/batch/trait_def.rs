use async_trait::async_trait;
use std::sync::Arc;

use super::error::BatchResult;
use crate::sync::types::IndexOperation;

#[async_trait]
pub trait BatchProcessor {
    async fn add(&self, operation: IndexOperation) -> BatchResult<()>;
    async fn add_batch(&self, operations: Vec<IndexOperation>) -> BatchResult<()>;
    async fn commit_all(&self) -> BatchResult<()>;
    async fn commit_timeout(&self) -> BatchResult<()>;
    async fn start_background_task(self: Arc<Self>);
    async fn stop_background_task(&self);
}

pub trait BatchBuffer<K, V> {
    fn add(&self, key: &K, value: V);
    fn drain(&self, key: &K) -> Vec<V>;
    fn peek(&self, key: &K) -> Vec<V>;
    fn count(&self, key: &K) -> usize;
    fn is_empty(&self, key: &K) -> bool;
    fn keys(&self) -> Vec<K>;
    fn clear(&self);
    fn total_count(&self) -> usize;
}
