use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use linkrs_core::error::QueryError;

// ── DiskQuota (separate from memory budget) ──────────────────────────────────

/// Tracks disk space used by spill operations for a single query.
///
/// Disk quota is independent of the memory budget.  Exceeding disk quota
/// produces a structured error rather than a silent spill-to-nowhere.
#[derive(Debug, Clone)]
pub struct DiskQuota {
    max_bytes: u64,
    used: Arc<AtomicU64>,
}

impl DiskQuota {
    /// Create a quota with the given byte limit.  `0` = unlimited.
    pub fn new(max_bytes: u64) -> Self {
        Self {
            max_bytes,
            used: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Default quota: 2 GiB per query.
    pub fn default_quota() -> Self {
        Self::new(2 * 1024 * 1024 * 1024)
    }

    /// Try to reserve `bytes` of disk space.  Returns an error when the
    /// quota would be exceeded.
    pub fn try_reserve(&self, bytes: u64) -> Result<(), QueryError> {
        if self.max_bytes == 0 {
            // unlimited
            let _ = self.used.fetch_add(bytes, Ordering::Relaxed);
            return Ok(());
        }
        let mut prev = self.used.load(Ordering::Relaxed);
        loop {
            let total = prev.checked_add(bytes).ok_or_else(|| {
                QueryError::execution(format!(
                    "Disk quota overflow: request {} bytes overflows u64",
                    bytes,
                ))
            })?;
            if total > self.max_bytes {
                return Err(QueryError::execution(format!(
                    "Disk quota exceeded: request {} bytes, total {} > quota {} bytes",
                    bytes, total, self.max_bytes,
                )));
            }
            match self
                .used
                .compare_exchange_weak(prev, total, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return Ok(()),
                Err(current) => prev = current,
            }
        }
    }

    /// Release `bytes` of disk space.
    pub fn release(&self, bytes: u64) {
        self.used.fetch_sub(bytes, Ordering::Relaxed);
    }

    /// Current used disk space.
    pub fn current(&self) -> u64 {
        self.used.load(Ordering::Relaxed)
    }

    /// Maximum allowed disk space.
    pub fn max(&self) -> u64 {
        self.max_bytes
    }
}
