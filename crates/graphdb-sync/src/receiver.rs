#[cfg(any(feature = "fulltext", feature = "vector"))]
use std::collections::HashSet;
#[cfg(any(feature = "fulltext", feature = "vector"))]
use std::sync::Arc;
#[cfg(any(feature = "fulltext", feature = "vector"))]
use tokio::sync::{Mutex, RwLock};

use graphdb_core::types::CommitLsn;
#[cfg(feature = "fulltext")]
use graphdb_core::wal::IndexMutation;

pub struct ApplyReceipt {
    pub commit_lsn: CommitLsn,
    pub idempotency_key: String,
    pub applied: bool,
}

pub struct LateArrivalResult {
    pub accepted: bool,
    pub reason: String,
}

#[cfg(feature = "fulltext")]
pub struct FulltextReceiver {
    engine: Arc<dyn graphdb_fulltext::engine::FulltextSearchEngine>,
    receipts: Arc<RwLock<HashSet<String>>>,
    applied_lsn: Arc<RwLock<CommitLsn>>,
    apply_lock: Mutex<()>,
}

// FulltextReceiver keeps JSON because Tantivy commit payloads accept only UTF-8
// strings; this intentional exception is documented.
#[cfg(feature = "fulltext")]
#[derive(serde::Serialize, serde::Deserialize)]
struct FulltextCommitState {
    applied_lsn: u64,
    receipts: HashSet<String>,
}

#[cfg(feature = "fulltext")]
impl FulltextReceiver {
    pub fn new(engine: Arc<dyn graphdb_fulltext::engine::FulltextSearchEngine>) -> Self {
        let state = engine
            .commit_payload()
            .ok()
            .flatten()
            .and_then(|payload| serde_json::from_str::<FulltextCommitState>(&payload).ok())
            .unwrap_or_else(|| FulltextCommitState {
                applied_lsn: 0,
                receipts: HashSet::new(),
            });
        Self {
            engine,
            receipts: Arc::new(RwLock::new(state.receipts)),
            applied_lsn: Arc::new(RwLock::new(CommitLsn::new(state.applied_lsn))),
            apply_lock: Mutex::new(()),
        }
    }

    pub async fn apply_index_batch(
        &self,
        mutations: &[(&IndexMutation, CommitLsn)],
    ) -> Result<Vec<ApplyReceipt>, String> {
        let _apply_guard = self.apply_lock.lock().await;
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
            let applied_lsn = *self.applied_lsn.read().await;
            if *lsn < applied_lsn {
                return Err(format!(
                    "late fulltext mutation: commit LSN {} is below applied LSN {}",
                    lsn, applied_lsn
                ));
            }

            match mutation.operation {
                graphdb_core::wal::IndexOperation::Delete => {
                    let entity_id = match &mutation.entity_ref {
                        graphdb_core::wal::EntityRef::Vertex(vid) => vid.to_string(),
                        graphdb_core::wal::EntityRef::Edge { src, dst, .. } => {
                            format!("{}->{}", src, dst)
                        }
                    };
                    deletes.push(entity_id);
                }
                graphdb_core::wal::IndexOperation::Upsert => {
                    let entity_id = match &mutation.entity_ref {
                        graphdb_core::wal::EntityRef::Vertex(vid) => vid.to_string(),
                        graphdb_core::wal::EntityRef::Edge { src, dst, .. } => {
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

        let mut persisted_receipts = self.receipts.read().await.clone();
        let mut max_lsn = *self.applied_lsn.read().await;
        for receipt in &mut receipts {
            if !receipt.applied {
                persisted_receipts.insert(receipt.idempotency_key.clone());
                receipt.applied = true;
            }
            if receipt.commit_lsn > max_lsn {
                max_lsn = receipt.commit_lsn;
            }
        }
        let payload = serde_json::to_string(&FulltextCommitState {
            applied_lsn: max_lsn.get(),
            receipts: persisted_receipts.clone(),
        })
        .map_err(|error| format!("serialize fulltext commit receipt: {error}"))?;
        self.engine
            .commit_with_payload(payload)
            .await
            .map_err(|e| format!("fulltext commit failed: {}", e))?;
        *self.receipts.write().await = persisted_receipts;
        *self.applied_lsn.write().await = max_lsn;

        Ok(receipts)
    }

    pub async fn check_late_arrival(
        &self,
        commit_lsn: CommitLsn,
        idempotency_key: &str,
    ) -> LateArrivalResult {
        let receipts = self.receipts.read().await;
        if receipts.contains(idempotency_key) {
            return LateArrivalResult {
                accepted: false,
                reason: "duplicate idempotency key".to_string(),
            };
        }
        let applied_lsn = *self.applied_lsn.read().await;
        if commit_lsn < applied_lsn {
            return LateArrivalResult {
                accepted: false,
                reason: format!(
                    "late arrival: commit_lsn {} < applied_lsn {}",
                    commit_lsn, applied_lsn
                ),
            };
        }
        LateArrivalResult {
            accepted: true,
            reason: String::new(),
        }
    }

    pub async fn applied_lsn(&self) -> CommitLsn {
        *self.applied_lsn.read().await
    }

    pub async fn is_idempotent(&self, key: &str) -> bool {
        self.receipts.read().await.contains(key)
    }
}

#[cfg(feature = "vector")]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct VectorCommitState {
    applied_lsn: u64,
    receipts: std::collections::HashMap<String, u64>,
}

#[cfg(feature = "vector")]
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct VectorCommitStateLegacy {
    applied_lsn: u64,
    receipts: HashSet<String>,
}

#[cfg(feature = "vector")]
const VECTOR_RECEIPT_MAX_ENTRIES: usize = 8192;
#[cfg(feature = "vector")]
const VECTOR_RECEIPT_RETENTION_LSN_WINDOW: u64 = 100_000;

#[cfg(feature = "vector")]
pub struct VectorReceiver {
    state: Arc<RwLock<VectorCommitState>>,
    apply_lock: Mutex<()>,
    recovery_path: Arc<std::path::PathBuf>,
}

#[cfg(feature = "vector")]
impl Default for VectorReceiver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "vector")]
impl VectorReceiver {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(VectorCommitState {
                applied_lsn: CommitLsn::ZERO.get(),
                receipts: std::collections::HashMap::new(),
            })),
            apply_lock: Mutex::new(()),
            recovery_path: Arc::new(std::path::PathBuf::new()),
        }
    }

    pub fn with_recovery_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.recovery_path = Arc::new(path.into());
        self
    }

    pub fn open(path: impl Into<std::path::PathBuf>) -> Self {
        let path: std::path::PathBuf = path.into();
        let state_path = Self::state_file_path(&path);
        let state = std::fs::read(&state_path)
            .ok()
            .and_then(|bytes| {
                if let Ok(state) = postcard::from_bytes::<VectorCommitState>(&bytes) {
                    return Some(state);
                }
                // Backward compatibility: legacy file stored HashSet
                if let Ok(legacy) = postcard::from_bytes::<VectorCommitStateLegacy>(&bytes) {
                    let mut receipts = std::collections::HashMap::new();
                    for key in legacy.receipts {
                        receipts.insert(key, legacy.applied_lsn);
                    }
                    return Some(VectorCommitState {
                        applied_lsn: legacy.applied_lsn,
                        receipts,
                    });
                }
                None
            })
            .unwrap_or(VectorCommitState {
                applied_lsn: 0,
                receipts: std::collections::HashMap::new(),
            });
        Self {
            state: Arc::new(RwLock::new(state)),
            apply_lock: Mutex::new(()),
            recovery_path: Arc::new(path),
        }
    }

    fn state_file_path(recovery_dir: &std::path::Path) -> std::path::PathBuf {
        recovery_dir.join("vector_receiver_state.bin")
    }

    async fn persist_state(&self, state: &VectorCommitState) -> Result<(), String> {
        let path = Self::state_file_path(&self.recovery_path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let temporary = path.with_extension("tmp");
        let bytes = postcard::to_allocvec(state).map_err(|e| e.to_string())?;
        std::fs::write(&temporary, &bytes).map_err(|e| e.to_string())?;
        std::fs::File::open(&temporary)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn check_late_arrival(
        &self,
        commit_lsn: CommitLsn,
        idempotency_key: &str,
    ) -> LateArrivalResult {
        let state = self.state.read().await;
        if state.receipts.contains_key(idempotency_key) {
            return LateArrivalResult {
                accepted: false,
                reason: "duplicate idempotency key".to_string(),
            };
        }

        let current = CommitLsn::new(state.applied_lsn);
        if commit_lsn < current {
            // Below the water-level: considered late. Even if the key was
            // pruned from the LRU window, the SQLite idempotency table still
            // protects against re-application (see SqliteOutbox::materialize).
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

    /// Persist an applied mutation receipt before exposing it to later deliveries.
    ///
    /// The caller must invoke this only after the vector mutation itself has
    /// completed.  A persistence failure is returned so delivery remains
    /// retryable instead of silently losing the idempotency receipt on restart.
    pub async fn record_application(
        &self,
        commit_lsn: CommitLsn,
        idempotency_key: &str,
    ) -> Result<(), String> {
        let _apply_guard = self.apply_lock.lock().await;
        let mut next = self.state.read().await.clone();
        next.receipts
            .insert(idempotency_key.to_string(), commit_lsn.get());
        next.applied_lsn = next.applied_lsn.max(commit_lsn.get());
        // LRU + water-level pruning: keep receipt set bounded. Entries whose
        // LSN is far below the current water-level are dropped; the SQLite
        // outbox `idempotency` table still guards against replay after a
        // restart for those older LSNs.
        if next.receipts.len() > VECTOR_RECEIPT_MAX_ENTRIES {
            let water_level = next
                .applied_lsn
                .saturating_sub(VECTOR_RECEIPT_RETENTION_LSN_WINDOW);
            next.receipts.retain(|_, lsn| *lsn >= water_level);
            // If still over capacity (e.g. bursty LSNs within window), drop
            // the oldest entries by LSN until under the limit.
            if next.receipts.len() > VECTOR_RECEIPT_MAX_ENTRIES {
                let mut entries: Vec<(String, u64)> =
                    next.receipts.iter().map(|(k, v)| (k.clone(), *v)).collect();
                entries.sort_by_key(|(_, lsn)| *lsn);
                let to_remove = entries.len() - VECTOR_RECEIPT_MAX_ENTRIES;
                for (key, _) in entries.into_iter().take(to_remove) {
                    next.receipts.remove(&key);
                }
            }
        }
        if !self.recovery_path.as_os_str().is_empty() {
            self.persist_state(&next).await?;
        }
        *self.state.write().await = next;
        Ok(())
    }

    pub async fn applied_lsn(&self) -> CommitLsn {
        CommitLsn::new(self.state.read().await.applied_lsn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[cfg(feature = "vector")]
    async fn vector_receiver_rejects_late_arrival() {
        let receiver = VectorReceiver::new();
        receiver
            .record_application(CommitLsn::new(100), "key-1")
            .await
            .expect("record receipt");

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
            .await
            .expect("record receipt");

        let result = receiver
            .check_late_arrival(CommitLsn::new(200), "key-3")
            .await;
        assert!(!result.accepted);
        assert!(result.reason.contains("duplicate"));
    }

    #[tokio::test]
    #[cfg(feature = "vector")]
    async fn vector_receiver_restores_persisted_receipts() {
        let directory = tempfile::tempdir().expect("create temporary recovery directory");
        let receiver = VectorReceiver::open(directory.path());
        receiver
            .record_application(CommitLsn::new(100), "key-1")
            .await
            .expect("persist receipt");

        let recovered = VectorReceiver::open(directory.path());
        assert_eq!(recovered.applied_lsn().await, CommitLsn::new(100));
        let duplicate = recovered
            .check_late_arrival(CommitLsn::new(100), "key-1")
            .await;
        assert!(!duplicate.accepted);
    }
}
