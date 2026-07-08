use dashmap::DashMap;
use std::time::Instant;

use super::trait_def::BatchBuffer;
use crate::sync::types::IndexOperation;

type IndexKey = (u64, String, String);

#[derive(Debug, Default, Clone)]
pub struct BufferEntry {
    pub inserts: Vec<IndexOperation>,
    pub deletes: Vec<String>,
}

#[derive(Debug)]
pub struct OpBatchBuffer {
    buffers: DashMap<IndexKey, BufferEntry>,
    last_commit: DashMap<IndexKey, Instant>,
}

impl Default for OpBatchBuffer {
    fn default() -> Self {
        Self::new()
    }
}

impl OpBatchBuffer {
    pub fn new() -> Self {
        Self {
            buffers: DashMap::new(),
            last_commit: DashMap::new(),
        }
    }

    pub fn add_insert(&self, key: &IndexKey, operation: IndexOperation) {
        let mut entry = self.buffers.entry(key.clone()).or_default();
        entry.inserts.push(operation);
        self.last_commit
            .entry(key.clone())
            .or_insert_with(Instant::now);
    }

    pub fn add_delete(&self, key: &IndexKey, id: String) {
        let mut entry = self.buffers.entry(key.clone()).or_default();
        entry.deletes.push(id);
        self.last_commit
            .entry(key.clone())
            .or_insert_with(Instant::now);
    }

    pub fn drain_inserts(&self, key: &IndexKey) -> Vec<IndexOperation> {
        if let Some(mut entry) = self.buffers.get_mut(key) {
            std::mem::take(&mut entry.inserts)
        } else {
            Vec::new()
        }
    }

    pub fn drain_deletes(&self, key: &IndexKey) -> Vec<String> {
        if let Some(mut entry) = self.buffers.get_mut(key) {
            std::mem::take(&mut entry.deletes)
        } else {
            Vec::new()
        }
    }

    pub fn drain_all(&self, key: &IndexKey) -> BufferEntry {
        self.buffers.remove(key).map(|(_, v)| v).unwrap_or_default()
    }

    pub fn peek_entry(&self, key: &IndexKey) -> BufferEntry {
        self.buffers
            .get(key)
            .map(|entry| BufferEntry {
                inserts: entry.inserts.clone(),
                deletes: entry.deletes.clone(),
            })
            .unwrap_or_default()
    }

    pub fn re_enqueue(&self, key: &IndexKey, entry: BufferEntry) {
        if entry.inserts.is_empty() && entry.deletes.is_empty() {
            return;
        }
        let mut buffer = self.buffers.entry(key.clone()).or_default();
        buffer.inserts.extend(entry.inserts);
        buffer.deletes.extend(entry.deletes);
    }

    pub fn count(&self, key: &IndexKey) -> usize {
        self.buffers
            .get(key)
            .map(|e| e.inserts.len() + e.deletes.len())
            .unwrap_or(0)
    }

    pub fn insert_count(&self, key: &IndexKey) -> usize {
        self.buffers.get(key).map(|e| e.inserts.len()).unwrap_or(0)
    }

    pub fn delete_count(&self, key: &IndexKey) -> usize {
        self.buffers.get(key).map(|e| e.deletes.len()).unwrap_or(0)
    }

    pub fn is_timeout(&self, key: &IndexKey, timeout: std::time::Duration) -> bool {
        self.last_commit
            .get(key)
            .map(|last| last.elapsed() >= timeout)
            .unwrap_or(false)
    }

    pub fn update_commit_time(&self, key: &IndexKey) {
        self.last_commit.insert(key.clone(), Instant::now());
    }

    pub fn keys(&self) -> Vec<IndexKey> {
        self.buffers.iter().map(|e| e.key().clone()).collect()
    }

    pub fn total_count(&self) -> usize {
        self.buffers
            .iter()
            .map(|e| e.inserts.len() + e.deletes.len())
            .sum()
    }

    pub fn clear(&self) {
        self.buffers.clear();
        self.last_commit.clear();
    }

    pub fn remove(&self, key: &IndexKey) -> Option<BufferEntry> {
        self.buffers.remove(key).map(|(_, v)| v)
    }
}

impl BatchBuffer<IndexKey, IndexOperation> for OpBatchBuffer {
    fn add(&self, key: &IndexKey, value: IndexOperation) {
        self.add_insert(key, value);
    }

    fn drain(&self, key: &IndexKey) -> Vec<IndexOperation> {
        self.drain_inserts(key)
    }

    fn peek(&self, key: &IndexKey) -> Vec<IndexOperation> {
        self.peek_entry(key).inserts
    }

    fn count(&self, key: &IndexKey) -> usize {
        self.insert_count(key)
    }

    fn is_empty(&self, key: &IndexKey) -> bool {
        self.insert_count(key) == 0
    }

    fn keys(&self) -> Vec<IndexKey> {
        self.keys()
    }

    fn clear(&self) {
        self.clear();
    }

    fn total_count(&self) -> usize {
        self.total_count()
    }
}
