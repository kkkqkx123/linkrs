//! Safe monotonic timestamp allocation with sentinel guards.

use std::sync::atomic::Ordering;

use graphdb_core::types::Timestamp;

use super::types::{VersionManagerError, VersionManagerResult};
use super::VersionManager;

impl VersionManager {
    pub(super) fn reserve_timestamp(&self) -> VersionManagerResult<Timestamp> {
        let mut current = self.write_ts.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(1)
                .ok_or(VersionManagerError::TimestampExhausted)?;
            // The allocator shares the u64 domain with sentinel values, so it
            // must stop before either sentinel instead of handing one out as a
            // transaction timestamp.
            debug_assert!(
                graphdb_core::types::is_allocatable_timestamp(next),
                "timestamp allocator reached reserved sentinel"
            );
            if !graphdb_core::types::is_allocatable_timestamp(next) {
                return Err(VersionManagerError::TimestampExhausted);
            }
            match self
                .write_ts
                .compare_exchange(current, next, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(next),
                Err(observed) => current = observed,
            }
        }
    }
}
