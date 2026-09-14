//! Durable outbox administration: configuration, snapshots, dead letters,
//! degraded ranges and retention.
use super::*;
use crate::checkpoint_manifest::CheckpointManifestManager;
use crate::sqlite_outbox::OutboxSnapshot;
use graphdb_core::types::CommitLsn;
use std::path::Path;
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
#[cfg_attr(
    not(any(feature = "fulltext", feature = "vector")),
    allow(unused_variables)
)]
impl super::SyncManager {
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
        let live_is_healthy =
            self.execute_sync(|| async { Ok(crate::verify_live_database(&sqlite_path).await) })?;
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
            max_concurrency: config.max_concurrency.max(1),
        });
    }

    /// Inject a backend-aware delivery policy (Local vs Qdrant) from startup.
    ///
    /// The policy replaces the raw `OutboxConsumerConfig` with backend-tuned
    /// values and records the chosen policy for observability.
    #[cfg(feature = "vector")]
    pub fn configure_backend_policy(&mut self, policy: crate::backend::BackendDeliveryPolicy) {
        self.backend_policy = Some(Arc::new(policy.clone()));
        self.configure_outbox_consumer(policy.to_consumer_config());
        // Align the per-target concurrency with the policy.
        let mut consumer = (*self.outbox_consumer).clone();
        consumer.max_concurrency = policy.max_concurrency.max(1);
        self.outbox_consumer = Arc::new(consumer);
    }

    /// Override the backpressure limits (defaults: 10k per txn, 100k total).
    pub fn configure_backpressure(&mut self, config: OutboxBackpressureConfig) {
        self.backpressure = Arc::new(OutboxBackpressureConfig {
            max_pending_per_txn: config.max_pending_per_txn.max(1),
            max_pending_total: config.max_pending_total.max(1),
        });
    }

    /// Current in-memory staged intent count across all transactions.
    pub fn staged_pending_len(&self) -> usize {
        self.pending_intents.iter().map(|e| e.value().len()).sum()
    }

    /// Expose the current durable + staged pending depth for storage-layer
    /// flow control. Returns `0` when no outbox is configured.
    pub fn outbox_pending(&self) -> usize {
        let staged = self.staged_pending_len();
        if let Some(outbox) = self.sqlite_outbox.clone() {
            let durable = crate::runtime::block_on_ambient(async move {
                outbox.stats().await.ok().map(|s| s.pending).unwrap_or(0)
            })
            .unwrap_or(0);
            staged + durable
        } else {
            staged
        }
    }

    pub fn outbox_stats(&self) -> crate::OutboxStats {
        let Some(outbox) = self.sqlite_outbox.clone() else {
            return crate::OutboxStats {
                pending: 0,
                ..Default::default()
            };
        };
        match crate::runtime::block_on_ambient(async move { outbox.stats().await }) {
            Ok(Ok(stats)) => stats,
            _ => crate::OutboxStats {
                pending: 0,
                ..Default::default()
            },
        }
    }

    /// Return durable outbox delivery and index-generation diagnostics.
    pub fn sync_diagnostics(&self) -> Result<crate::SyncDiagnostics, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        #[allow(unused_mut)]
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
        target: &graphdb_core::types::TargetId,
        index_id: u64,
        generation: u64,
        minimum_lsn: graphdb_core::types::CommitLsn,
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

    pub fn create_checkpoint_outbox_snapshot(&self) -> Result<crate::OutboxSnapshot, SyncError> {
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

    pub fn requeue_dead_letter(&self, event_id: i64) -> Result<bool, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        self.execute_sync(|| async {
            outbox
                .requeue_dead_letter(event_id)
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    pub fn requeue_dead_letters_batch(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
    ) -> Result<usize, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let outbox = outbox.clone();
        let target = target.cloned();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            let target = target.clone();
            async move {
                outbox
                    .requeue_dead_letters_batch(target.as_ref(), index_id, generation, limit)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn list_dead_letters(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<crate::DeadLetterRow>, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let outbox = outbox.clone();
        let target = target.cloned();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            let target = target.clone();
            async move {
                outbox
                    .list_dead_letters(target.as_ref(), index_id, generation, limit, offset)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn list_degraded_ranges(
        &self,
        target: Option<&graphdb_core::types::TargetId>,
        index_id: Option<u64>,
        generation: Option<u64>,
    ) -> Result<Vec<crate::DegradedRangeRow>, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let outbox = outbox.clone();
        let target = target.cloned();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            let target = target.clone();
            async move {
                outbox
                    .list_degraded_ranges(target.as_ref(), index_id, generation)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn clear_degraded_range(
        &self,
        target: &graphdb_core::types::TargetId,
        index_id: u64,
        generation: u64,
        start_lsn: graphdb_core::types::CommitLsn,
        end_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<bool, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let outbox = outbox.clone();
        let target = target.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            let target = target.clone();
            async move {
                outbox
                    .clear_degraded_range(&target, index_id, generation, start_lsn, end_lsn)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn retention_lsn(&self) -> Result<graphdb_core::types::CommitLsn, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        self.execute_sync(|| async {
            outbox
                .retention_lsn()
                .await
                .map_err(SyncError::PersistenceError)
        })
    }

    pub fn update_retention_lsn(
        &self,
        retention_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<(), SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Err(SyncError::PersistenceError(
                "SQLite outbox is not configured".to_string(),
            ));
        };
        let outbox = outbox.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            async move {
                outbox
                    .update_retention_lsn(retention_lsn)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn prune_applied_events(
        &self,
        retention_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<u64, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        let outbox = outbox.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            async move {
                outbox
                    .prune_applied_events(retention_lsn)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn archive_dead_letters(
        &self,
        retention_lsn: graphdb_core::types::CommitLsn,
    ) -> Result<u64, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        let outbox = outbox.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            async move {
                outbox
                    .archive_dead_letters(retention_lsn)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    pub fn prune_degraded_ranges(&self, max_age_ms: u64) -> Result<u64, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(0);
        };
        let outbox = outbox.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            async move {
                outbox
                    .prune_degraded_ranges(max_age_ms)
                    .await
                    .map_err(SyncError::PersistenceError)
            }
        })
    }

    /// Run one retention cycle: prune applied, archive dead letters, update retention watermark.
    pub fn run_retention_once(
        &self,
        grace_lsn_distance: u64,
        max_age_ms: u64,
    ) -> Result<(u64, u64, u64), SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok((0, 0, 0));
        };
        let outbox = outbox.clone();
        self.execute_sync(move || {
            let outbox = outbox.clone();
            async move {
                let safe = outbox
                    .compute_safe_retention_lsn(grace_lsn_distance)
                    .await
                    .map_err(SyncError::PersistenceError)?;
                let pruned = outbox
                    .prune_applied_events(safe)
                    .await
                    .map_err(SyncError::PersistenceError)?;
                let archived = outbox
                    .archive_dead_letters(safe)
                    .await
                    .map_err(SyncError::PersistenceError)?;
                outbox
                    .update_retention_lsn(safe)
                    .await
                    .map_err(SyncError::PersistenceError)?;
                let _ = outbox
                    .prune_degraded_ranges(max_age_ms)
                    .await
                    .map_err(SyncError::PersistenceError)?;
                Ok((pruned, archived, safe.get()))
            }
        })
    }
}
