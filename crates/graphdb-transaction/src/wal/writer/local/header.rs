//! Local WAL writer - header module

use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::Ordering;

use graphdb_core::wal::types::{Lsn, WalError, WalFileHeader, WalResult, WAL_FILE_HEADER_SIZE};

use super::LocalWalWriter;

impl LocalWalWriter {
    /// Write WAL file header
    pub(crate) fn write_file_header(&mut self) -> WalResult<()> {
        let current_lsn = Lsn::new(self.current_lsn.load(Ordering::SeqCst));
        let header = WalFileHeader::new(self.thread_id, self.checkpoint_seq, current_lsn)
            .with_checksum_enabled(self.config.checksum_enabled);
        self.persist_file_header(header, true)
    }

    /// Persist a WAL file header to disk.
    pub(crate) fn persist_file_header(
        &mut self,
        header: WalFileHeader,
        reset_file_used: bool,
    ) -> WalResult<()> {
        let header_bytes = header.as_bytes();

        let file = self.file.as_mut().ok_or(WalError::Closed)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header_bytes)?;
        file.sync_all()?;

        self.file_header = Some(header);
        self.file_start_lsn = header.start_lsn();

        if reset_file_used {
            self.file_used = WAL_FILE_HEADER_SIZE;
        }

        Ok(())
    }

    /// Rewrite the current file header with the latest checkpoint sequence.
    pub(crate) fn refresh_file_header(&mut self) -> WalResult<()> {
        if self.file.is_none() {
            return Ok(());
        }

        let Some(header) = self.file_header else {
            return Ok(());
        };

        let updated_header = WalFileHeader {
            checkpoint_seq: self.checkpoint_seq,
            ..header
        };
        self.persist_file_header(updated_header, false)
    }

    pub fn file_start_lsn(&self) -> Lsn {
        self.file_start_lsn
    }

    /// Establish a recovered logical WAL baseline when the durable prefix has
    /// already been moved into a checkpoint and the remaining WAL segment is
    /// empty. The empty segment must start at the recovered LSN; otherwise the
    /// first record appended after restart would have an invalid prev_lsn chain.
    pub fn set_recovery_baseline_lsn(&mut self, lsn: Lsn) -> WalResult<()> {
        let current_lsn = self.current_lsn();
        if lsn <= current_lsn {
            return Ok(());
        }

        if self.file_used > WAL_FILE_HEADER_SIZE {
            return Err(WalError::InvalidOperation(format!(
                "Cannot advance WAL baseline to {} while the active segment contains records",
                lsn
            )));
        }

        self.current_lsn.store(lsn.as_u64(), Ordering::SeqCst);
        self.last_synced_lsn.store(lsn.as_u64(), Ordering::SeqCst);

        if let Some(header) = self.file_header {
            let updated_header = WalFileHeader {
                start_lsn: lsn.as_u64(),
                ..header
            };
            self.persist_file_header(updated_header, false)?;
        } else {
            self.file_start_lsn = lsn;
        }

        Ok(())
    }

    pub fn checkpoint_seq(&self) -> u64 {
        self.checkpoint_seq
    }

    pub fn set_checkpoint_seq(&mut self, seq: u64) -> WalResult<()> {
        self.checkpoint_seq = seq;
        if self.file.is_some() {
            self.refresh_file_header()?;
        }
        Ok(())
    }

    pub fn file_header(&self) -> Option<&WalFileHeader> {
        self.file_header.as_ref()
    }
}
