#[cfg(any(feature = "fulltext-search", feature = "qdrant"))]
use std::collections::HashSet;
#[cfg(any(feature = "fulltext-search", feature = "qdrant"))]
use std::sync::Arc;
#[cfg(any(feature = "fulltext-search", feature = "qdrant"))]
use tokio::sync::RwLock;

use crate::core::types::CommitLsn;
#[cfg(feature = "fulltext-search")]
use crate::core::wal::IndexMutation;

pub struct ApplyReceipt {
    pub commit_lsn: CommitLsn,
    pub idempotency_key: String,
    pub applied: bool,
}

pub struct LateArrivalResult {
    pub accepted: bool,
    pub reason: String,
}

#[cfg(feature = "fulltext-search")]
pub struct FulltextReceiver {
    engine: Arc<crate::search::tantivy_index::TantivySearchEngine>,
    receipts: Arc<RwLock<HashSet<String>>>,
    applied_lsn: Arc<RwLock<CommitLsn>>,
}

#[cfg(feature = "fulltext-search")]
impl FulltextReceiver {
    pub fn new(engine: Arc<crate::search::tantivy_index::TantivySearchEngine>) -> Self {
        Self {
            engine,
            receipts: Arc::new(RwLock::new(HashSet::new())),
            applied_lsn: Arc::new(RwLock::new(CommitLsn::ZERO)),
        }
    }

    pub async fn apply_index_batch(
        &self,
        mutations: &[(&IndexMutation, CommitLsn)],
    ) -> Result<Vec<ApplyReceipt>, String> {
        let mut deletes = Vec::new();
        let mut items = Vec::new();
        let mut receipts = Vec::new();
        let receipts_guard = self.receipts.read().await;

        for (mutation, lsn) in mutations {
            let idempotency_key = mutation.idempotency_key.as_str().to_string();
            if receipts_guard.contains(&idempotency_key) {
                receipts.push(ApplyReceipt {
                    commit_lsn: *lsn,
                    idempotency_key,
                    applied: true,
                });
                continue;
            }

            match mutation.operation {
                crate::core::wal::IndexOperation::Delete => {
                    let entity_id = match &mutation.entity_ref {
                        crate::core::wal::EntityRef::Vertex(vid) => vid.to_string(),
                        crate::core::wal::EntityRef::Edge { src, dst, .. } => {
                            format!("{}->{}", src, dst)
                        }
                    };
                    deletes.push(entity_id);
                }
                crate::core::wal::IndexOperation::Upsert => {
                    let entity_id = match &mutation.entity_ref {
                        crate::core::wal::EntityRef::Vertex(vid) => vid.to_string(),
                        crate::core::wal::EntityRef::Edge { src, dst, .. } => {
                            format!("{}->{}", src, dst)
                        }
                    };
                    let document = String::from_utf8(mutation.document_or_vector.clone())
                        .map_err(|e| format!("invalid utf-8 in fulltext document: {}", e))?;
                    items.push((entity_id, document));
                }
            }
            receipts.push(ApplyReceipt {
                commit_lsn: *lsn,
                idempotency_key,
                applied: false,
            });
        }
        drop(receipts_guard);

        if !deletes.is_empty() {
            let ids: Vec<&str> = deletes.iter().map(|s| s.as_str()).collect();
            self.engine
                .delete_batch(ids)
                .await
                .map_err(|e| format!("fulltext delete batch failed: {}", e))?;
        }

        if !items.is_empty() {
            self.engine
                .index_batch(items)
                .await
                .map_err(|e| format!("fulltext index batch failed: {}", e))?;
        }

        self.engine
            .commit()
            .await
            .map_err(|e| format!("fulltext commit failed: {}", e))?;

        let mut receipts_guard = self.receipts.write().await;
        let mut max_lsn = *self.applied_lsn.read().await;
        for receipt in &mut receipts {
            if !receipt.applied {
                receipts_guard.insert(receipt.idempotency_key.clone());
                receipt.applied = true;
            }
            if receipt.commit_lsn > max_lsn {
                max_lsn = receipt.commit_lsn;
            }
        }
        *self.applied_lsn.write().await = max_lsn;

        Ok(receipts)
    }

    pub async fn applied_lsn(&self) -> CommitLsn {
        *self.applied_lsn.read().await
    }

    pub async fn is_idempotent(&self, key: &str) -> bool {
        self.receipts.read().await.contains(key)
    }
}

#[cfg(feature = "qdrant")]
pub struct VectorReceiver {
    applied_lsn: Arc<RwLock<CommitLsn>>,
    idempotency_keys: Arc<RwLock<HashSet<String>>>,
}

#[cfg(feature = "qdrant")]
impl VectorReceiver {
    pub fn new() -> Self {
        Self {
            applied_lsn: Arc::new(RwLock::new(CommitLsn::ZERO)),
            idempotency_keys: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    pub async fn check_late_arrival(
        &self,
        commit_lsn: CommitLsn,
        idempotency_key: &str,
    ) -> LateArrivalResult {
        if self.idempotency_keys.read().await.contains(idempotency_key) {
            return LateArrivalResult {
                accepted: false,
                reason: "duplicate idempotency key".to_string(),
            };
        }

        let current = *self.applied_lsn.read().await;
        if commit_lsn < current {
            return LateArrivalResult {
                accepted: false,
                reason: format!(
                    "late arrival: commit_lsn {} < applied_lsn {}",
                    commit_lsn, current
                ),
            };
        }

        LateArrivalResult {
            accepted: true,
            reason: String::new(),
        }
    }

    pub async fn record_application(&self, commit_lsn: CommitLsn, idempotency_key: &str) {
        self.idempotency_keys
            .write()
            .await
            .insert(idempotency_key.to_string());
        let mut guard = self.applied_lsn.write().await;
        if commit_lsn > *guard {
            *guard = commit_lsn;
        }
    }

    pub async fn applied_lsn(&self) -> CommitLsn {
        *self.applied_lsn.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(feature = "qdrant")]
    async fn vector_receiver_rejects_late_arrival() {
        let receiver = VectorReceiver::new();
        receiver
            .record_application(CommitLsn::new(100), "key-1")
            .await;

        let result = receiver
            .check_late_arrival(CommitLsn::new(50), "key-2")
            .await;
        assert!(!result.accepted);
        assert!(result.reason.contains("late arrival"));

        let result = receiver
            .check_late_arrival(CommitLsn::new(100), "key-3")
            .await;
        assert!(result.accepted);
        receiver
            .record_application(CommitLsn::new(100), "key-3")
            .await;

        let result = receiver
            .check_late_arrival(CommitLsn::new(200), "key-3")
            .await;
        assert!(!result.accepted);
        assert!(result.reason.contains("duplicate"));
    }
}
