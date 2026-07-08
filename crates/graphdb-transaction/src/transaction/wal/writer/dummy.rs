//! Dummy WAL writer (no-op, for read-only mode)

use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::wal::traits::WalWriter;
use crate::core::wal::types::WalResult;

/// Dummy WAL writer (no-op, for read-only mode)
pub struct DummyWalWriter {
    is_open: AtomicBool,
}

impl DummyWalWriter {
    pub fn new() -> Self {
        Self {
            is_open: AtomicBool::new(false),
        }
    }
}

impl Default for DummyWalWriter {
    fn default() -> Self {
        Self::new()
    }
}

impl WalWriter for DummyWalWriter {
    fn open(&mut self) -> WalResult<()> {
        self.is_open.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn close(&mut self) {
        self.is_open.store(false, Ordering::SeqCst);
    }

    fn append(&mut self, _data: &[u8]) -> WalResult<bool> {
        Ok(true)
    }

    fn sync(&self) -> WalResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dummy_wal_writer() {
        let mut writer = DummyWalWriter::new();
        writer.open().expect("Failed to open");
        writer.append(b"test").expect("Failed to append");
        writer.sync().expect("Failed to sync");
        writer.close();
    }
}
