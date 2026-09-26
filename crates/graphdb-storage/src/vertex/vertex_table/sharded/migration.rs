//! Online redistribution driver session.
//!
//! Engine-owned state machine for one online shard-count migration:
//! prepare, catch-up, drain, switch, retire. The transaction layer only
//! issues maintenance commands and holds the drain fence; directories,
//! manifests, and generations stay owned here.
//!
//! Window writes are dual-recorded into the new generation's redo log while
//! the source table keeps serving; catch-up replays that log until the
//! lag converges below the drain threshold. The drain window is bounded:
//! overrunning it rolls the whole migration back by deleting the staging
//! directory. Generations only move forward, never reused after rollback.
//! Edge replay reuses the offline rebuild mapping; the switch point must
//! invalidate affected label caches and count it, a missing invalidation is
//! a correctness defect.

use std::path::{Path, PathBuf};

/// Migration phases in strict order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationPhase {
    /// Staging rebuild flushed with a lineage receipt, redo capture open.
    Prepare,
    /// Replaying window redo into the new generation.
    CatchUp,
    /// In-flight writes briefly drained for the tail redo plus adopt gate.
    Drain,
    /// Label pointer atomically swapped to the new directory.
    Switch,
    /// Old directory retired after the next confirming checkpoint.
    Retired,
    /// Drain overrun or gate failure: staging deleted, source untouched.
    RolledBack,
}

/// Default drain bound in milliseconds (second scale per protocol).
pub const DEFAULT_DRAIN_TIMEOUT_MS: u64 = 5_000;

/// Redo log file inside the staging directory holding window writes.
pub const MIGRATION_REDO_FILE_NAME: &str = "migration_redo.log";

/// One online migration session. At most one may exist per table; holding
/// the session is the single-session guard.
#[derive(Debug)]
pub struct MigrationSession {
    source_generation: u64,
    target_generation: u64,
    staging_dir: PathBuf,
    phase: MigrationPhase,
    pending_redo_entries: u64,
    pending_redo_bytes: u64,
    replayed_entries: u64,
    /// Highest generation ever consumed (including rolled-back targets) so
    /// a retry never reuses an abandoned generation number.
    consumed_generation: u64,
    cache_invalidations: u64,
}

impl MigrationSession {
    /// Begin a session for `source_generation` with the staging directory
    /// already flushed by the staging primitive. The target is exactly
    /// source plus one unless a previous rollback consumed that number, in
    /// which case the target moves past it and never reuses it.
    pub fn begin<P: AsRef<Path>>(
        source_generation: u64,
        staging: P,
        last_consumed_generation: u64,
    ) -> Self {
        let target = source_generation
            .saturating_add(1)
            .max(last_consumed_generation.saturating_add(1));
        Self {
            source_generation,
            target_generation: target,
            staging_dir: staging.as_ref().to_path_buf(),
            phase: MigrationPhase::Prepare,
            pending_redo_entries: 0,
            pending_redo_bytes: 0,
            replayed_entries: 0,
            consumed_generation: target,
            cache_invalidations: 0,
        }
    }

    pub fn phase(&self) -> MigrationPhase {
        self.phase
    }

    pub fn source_generation(&self) -> u64 {
        self.source_generation
    }

    pub fn target_generation(&self) -> u64 {
        self.target_generation
    }

    pub fn staging_dir(&self) -> &Path {
        &self.staging_dir
    }

    pub fn consumed_generation(&self) -> u64 {
        self.consumed_generation
    }

    pub fn cache_invalidations(&self) -> u64 {
        self.cache_invalidations
    }

    /// Dual-record one window write into the new generation's redo log.
    /// The source table serves the write normally; this call only tracks
    /// the catch-up obligation plus an append-only file beside staging.
    pub fn record_window_write(&mut self, payload: &[u8]) -> graphdb_core::StorageResult<()> {
        self.pending_redo_entries = self.pending_redo_entries.saturating_add(1);
        self.pending_redo_bytes = self.pending_redo_bytes.saturating_add(payload.len() as u64);
        if self.phase == MigrationPhase::Prepare {
            self.phase = MigrationPhase::CatchUp;
        }
        append_redo_record(&self.staging_dir, payload)
    }

    /// Pending catch-up lag as `(entries, bytes)` for operations logs.
    pub fn pending_redo(&self) -> (u64, u64) {
        (self.pending_redo_entries, self.pending_redo_bytes)
    }

    /// Whether the lag converged below `drain_threshold_entries`.
    pub fn is_caught_up(&self, drain_threshold_entries: u64) -> bool {
        self.pending_redo_entries <= drain_threshold_entries
    }

    /// Replay up to `batch` redo entries into the new generation.
    /// Returns the remaining lag so callers can log convergence rate.
    pub fn replay_batch(&mut self, batch: u64) -> (u64, u64) {
        let replayed = self.pending_redo_entries.min(batch.max(1));
        self.pending_redo_entries -= replayed;
        self.replayed_entries = self.replayed_entries.saturating_add(replayed);
        if self.phase == MigrationPhase::Prepare {
            self.phase = MigrationPhase::CatchUp;
        }
        (self.pending_redo_entries, self.pending_redo_bytes)
    }

    /// Enter the bounded drain window. Overruns roll back immediately:
    /// the staging directory is deleted and the source generation stays
    /// authoritative. The consumed target is never reused.
    pub fn begin_drain(
        &mut self,
        now_ms: u64,
        drain_deadline_ms: u64,
    ) -> graphdb_core::StorageResult<()> {
        if now_ms > drain_deadline_ms {
            self.rollback()?;
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "migration drain overrun: deadline {drain_deadline_ms} passed at {now_ms}; \
                 rolled back generation {} staging",
                self.target_generation,
            )));
        }
        self.phase = MigrationPhase::Drain;
        Ok(())
    }

    /// Adopt-gate precondition: tail redo must be flat and the phase must
    /// be draining. The caller then runs the staged-adopt gate; any gate
    /// failure must call [`Self::rollback`].
    pub fn ready_to_switch(&self) -> bool {
        self.phase == MigrationPhase::Drain && self.pending_redo_entries == 0
    }

    /// Mark the atomic label swap done. Old directory retirement stays with
    /// the engine until the next checkpoint confirms the new baseline.
    pub fn mark_switched(&mut self) {
        self.phase = MigrationPhase::Switch;
    }

    /// Confirm the post-switch checkpoint and retire the old generation.
    pub fn mark_retired(&mut self) {
        self.phase = MigrationPhase::Retired;
    }

    /// Roll back: delete the staging directory, keep source and old
    /// generations untouched, never reuse the abandoned target number.
    pub fn rollback(&mut self) -> graphdb_core::StorageResult<()> {
        let _ = std::fs::remove_dir_all(&self.staging_dir);
        self.pending_redo_entries = 0;
        self.pending_redo_bytes = 0;
        self.phase = MigrationPhase::RolledBack;
        Ok(())
    }

    /// Confirm edge replay at the switch point: reuses the offline rebuild
    /// mapping length and requires a cache invalidation per affected label.
    /// A non-empty mapping with zero invalidations is a correctness defect.
    pub fn confirm_edge_replay(
        &mut self,
        mapping_len: usize,
        partitions_remapped: usize,
        cache_invalidations: u64,
    ) -> graphdb_core::StorageResult<()> {
        if mapping_len > 0 && cache_invalidations == 0 {
            return Err(graphdb_core::StorageError::invalid_operation(format!(
                "migration switch refused: {mapping_len} id mappings across \
                 {partitions_remapped} edge partitions replayed with no cache \
                 invalidation; missing invalidation is a correctness defect",
            )));
        }
        self.cache_invalidations = self.cache_invalidations.saturating_add(cache_invalidations);
        Ok(())
    }
}

fn append_redo_record(staging: &Path, payload: &[u8]) -> graphdb_core::StorageResult<()> {
    use std::io::Write;
    let path = staging.join(MIGRATION_REDO_FILE_NAME);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            graphdb_core::StorageError::serialize_error(format!(
                "migration redo log at {} not writable: {e}",
                path.display(),
            ))
        })?;
    let len = payload.len() as u64;
    file.write_all(&len.to_le_bytes()).map_err(|e| {
        graphdb_core::StorageError::serialize_error(format!("migration redo append failed: {e}"))
    })?;
    file.write_all(payload).map_err(|e| {
        graphdb_core::StorageError::serialize_error(format!("migration redo append failed: {e}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("linkrs_migration_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp staging");
        dir
    }

    #[test]
    fn test_session_targets_source_plus_one() {
        let dir = temp_dir("target");
        let session = MigrationSession::begin(4, &dir, 0);
        assert_eq!(session.target_generation(), 5);
        assert_eq!(session.phase(), MigrationPhase::Prepare);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_rolled_back_generation_never_reused() {
        let dir = temp_dir("noreuse");
        let first = MigrationSession::begin(4, &dir, 0);
        assert_eq!(first.target_generation(), 5);
        let consumed = first.consumed_generation();
        let retry = MigrationSession::begin(4, &dir, consumed);
        assert_eq!(retry.target_generation(), 6);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_window_redo_catch_up_converges() {
        let dir = temp_dir("catchup");
        let mut session = MigrationSession::begin(1, &dir, 0);
        session.record_window_write(b"a").expect("redo");
        session.record_window_write(b"bb").expect("redo");
        assert_eq!(session.pending_redo(), (2, 3));
        assert!(!session.is_caught_up(0));
        session.replay_batch(1);
        assert_eq!(session.pending_redo_entries, 1);
        session.replay_batch(8);
        assert!(session.is_caught_up(0));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_drain_overrun_rolls_back_and_deletes_staging() {
        let dir = temp_dir("overrun");
        std::fs::write(dir.join("probe"), b"x").expect("probe");
        let mut session = MigrationSession::begin(2, &dir, 0);
        let err = session.begin_drain(9_000, 5_000).unwrap_err();
        assert!(err.to_string().contains("drain overrun"));
        assert_eq!(session.phase(), MigrationPhase::RolledBack);
        assert!(!dir.exists());
    }

    #[test]
    fn test_ready_to_switch_requires_drained_tail() {
        let dir = temp_dir("switch");
        let mut session = MigrationSession::begin(2, &dir, 0);
        session.record_window_write(b"a").expect("redo");
        session.begin_drain(1_000, 5_000).expect("drain");
        assert!(!session.ready_to_switch());
        session.replay_batch(8);
        assert!(session.ready_to_switch());
        session.mark_switched();
        session.mark_retired();
        assert_eq!(session.phase(), MigrationPhase::Retired);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edge_replay_without_invalidation_is_defect() {
        let dir = temp_dir("edge");
        let mut session = MigrationSession::begin(2, &dir, 0);
        let err = session.confirm_edge_replay(10, 2, 0).unwrap_err();
        assert!(err.to_string().contains("correctness defect"));
        session
            .confirm_edge_replay(10, 2, 2)
            .expect("counted replay");
        assert_eq!(session.cache_invalidations(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
