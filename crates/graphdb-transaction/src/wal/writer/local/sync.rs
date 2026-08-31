//! Local WAL writer - sync module

use std::sync::atomic::Ordering;

use graphdb_core::wal::types::{WalError, WalResult};

use super::LocalWalWriter;
use crate::wal::writer::group_commit::GroupCommitCoordinator;

impl LocalWalWriter {
    /// Enable group commit coordination for this writer.
    ///
    /// Must be called after [`open`](Self::open) so that the file handle exists.
    /// When enabled, calls to [`sync`](WalWriter::sync) and the final sync in
    /// [`append_batch`](Self::append_batch) are routed through the coordinator,
    /// which batches fsync operations across threads.
    pub fn enable_group_commit(&mut self) -> WalResult<()> {
        self.enable_group_commit_with_timeout(self.config.group_commit_timeout())
    }

    pub fn enable_group_commit_with_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> WalResult<()> {
        let file = self.file.as_ref().ok_or(WalError::Closed)?;
        let start_lsn = self.current_lsn.load(Ordering::SeqCst);
        self.group_commit = Some(GroupCommitCoordinator::with_timeout(
            file.try_clone()
                .map_err(|e| WalError::IoError(e.to_string()))?,
            start_lsn,
            timeout,
        ));
        Ok(())
    }

    pub fn enable_group_commit_with_config(
        &mut self,
        config: &graphdb_core::wal::types::WalConfig,
    ) -> WalResult<()> {
        self.enable_group_commit_with_timeout(config.group_commit_timeout())
    }

    /// Get the group commit coordinator, if enabled.
    pub fn group_commit_coordinator(&self) -> Option<&GroupCommitCoordinator> {
        self.group_commit.as_ref()
    }
}
