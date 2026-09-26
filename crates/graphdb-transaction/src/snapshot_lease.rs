//! Long-read snapshot leases issued by the transaction layer.
//!
//! Storage never issues leases: only a transaction that declares a long
//! read holds one, with the snapshot timestamp as the floor and the
//! deployment lease TTL as the term. Expiry returns a retryable snapshot
//! error for the caller to retry; admission (refusing new long reads under
//! backpressure) is decoupled from expiry and never kills holders.
//! All clocks are injected (`Instant` params) so issue, expiry, renew, and
//! reap are unit-testable without time mocking.

use std::time::{Duration, Instant};

use super::error::{TransactionError, TransactionErrorKind};
use super::types::TransactionId;
use graphdb_core::types::Timestamp;

/// One held long-read lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LongReadLease {
    /// Owning transaction.
    pub holder: TransactionId,
    /// Snapshot timestamp the holder may still read.
    pub floor_ts: Timestamp,
    /// Lease expiry.
    pub deadline: Instant,
}

impl LongReadLease {
    /// Whether the lease still protects reads at `now`.
    pub fn is_live(&self, now: Instant) -> bool {
        now < self.deadline
    }
}

/// Backpressure view polled from storage before admitting a new long read.
/// Read-only signal: exceeding it refuses the new long read, never kills
/// holders (expiry stays on the read path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseBackpressureView {
    /// Live leases pinning the watermark at the sampled instant.
    pub live_leases: usize,
    /// How long the deepest floor has pinned the watermark, in seconds.
    pub pinned_secs: u64,
}

/// Admission thresholds from deployment configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseAdmissionThresholds {
    /// Refuse new long reads at or above this many live leases.
    pub max_live_leases: usize,
    /// Refuse new long reads when the pin age reaches this.
    pub max_pin_secs: u64,
}

impl Default for LeaseAdmissionThresholds {
    fn default() -> Self {
        Self {
            max_live_leases: 64,
            max_pin_secs: 300,
        }
    }
}

/// Check admission before issuing: refuse the new long read (with the
/// current backpressure values in the message) when either threshold is
/// met. Short transactions skip this entirely.
pub fn check_long_read_admission(
    backpressure: LeaseBackpressureView,
    thresholds: LeaseAdmissionThresholds,
) -> Result<(), TransactionError> {
    if backpressure.live_leases >= thresholds.max_live_leases {
        return Err(TransactionError::too_many_transactions());
    }
    if backpressure.pinned_secs >= thresholds.max_pin_secs {
        return Err(TransactionError::new(
            TransactionErrorKind::TooManyTransactions,
            format!(
                "long read refused: snapshot pinned for {}s (limit {}s) with {} live leases",
                backpressure.live_leases, thresholds.max_pin_secs, backpressure.pinned_secs,
            ),
        ));
    }
    Ok(())
}

/// Clamp the requested TTL to the storage-side maximum: the deployment
/// default applies, but it must never exceed what storage will honor.
pub fn clamp_lease_ttl(requested: Duration, storage_max: Duration) -> Duration {
    let ttl = requested.min(storage_max);
    if ttl.is_zero() {
        Duration::from_millis(1)
    } else {
        ttl
    }
}

/// Snapshot-expiry read error: carries holder plus floor timestamp so the
/// caller can retry on the existing retry path. Retryable by construction.
pub fn snapshot_expired(holder: TransactionId, floor_ts: Timestamp) -> TransactionError {
    TransactionError::new(
        TransactionErrorKind::SnapshotExpired,
        format!(
            "snapshot expired for long read txn={} floor_ts={floor_ts}; retry the read",
            holder.0,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(id: u64) -> TransactionId {
        TransactionId(id)
    }

    #[test]
    fn test_issue_and_expiry_paths() {
        let now = Instant::now();
        let lease = LongReadLease {
            holder: holder(7),
            floor_ts: 500,
            deadline: now + Duration::from_secs(10),
        };
        assert!(lease.is_live(now));
        assert!(!lease.is_live(now + Duration::from_secs(11)));
        let err = snapshot_expired(lease.holder, lease.floor_ts);
        assert_eq!(err.kind(), TransactionErrorKind::SnapshotExpired);
        assert!(err.is_retryable());
        assert!(err.message().contains('7'));
        assert!(err.message().contains("500"));
    }

    #[test]
    fn test_admission_refuses_new_long_reads_only() {
        let thresholds = LeaseAdmissionThresholds {
            max_live_leases: 2,
            max_pin_secs: 60,
        };
        assert!(check_long_read_admission(
            LeaseBackpressureView {
                live_leases: 1,
                pinned_secs: 10
            },
            thresholds,
        )
        .is_ok());
        assert!(check_long_read_admission(
            LeaseBackpressureView {
                live_leases: 2,
                pinned_secs: 10
            },
            thresholds,
        )
        .is_err());
        assert!(check_long_read_admission(
            LeaseBackpressureView {
                live_leases: 1,
                pinned_secs: 60
            },
            thresholds,
        )
        .is_err());
    }

    #[test]
    fn test_renewal_extends_deadline_and_reap_counts_expiry() {
        let start = Instant::now();
        let mut lease = LongReadLease {
            holder: holder(1),
            floor_ts: 100,
            deadline: start + Duration::from_secs(5),
        };
        lease.deadline = start + Duration::from_secs(30);
        assert!(lease.is_live(start + Duration::from_secs(10)));
        assert!(!lease.is_live(start + Duration::from_secs(31)));
    }

    #[test]
    fn test_ttl_clamped_to_storage_maximum() {
        let max = Duration::from_secs(300);
        assert_eq!(
            clamp_lease_ttl(Duration::from_secs(600), max),
            Duration::from_secs(300)
        );
        assert_eq!(
            clamp_lease_ttl(Duration::from_secs(10), max),
            Duration::from_secs(10)
        );
        assert!(!clamp_lease_ttl(Duration::ZERO, max).is_zero());
    }
}
