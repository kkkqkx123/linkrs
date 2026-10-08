//! Long-read lease issuance, heartbeat renewal, expiry reads, and
//! backpressure admission. Storage never issues; only declared long reads
//! hold leases. Termination (commit, abort, timeout) always releases.

use std::time::{Duration, Instant};

use super::TransactionManager;
use crate::error::TransactionError;
use crate::snapshot_lease::{
    check_long_read_admission, clamp_lease_ttl, snapshot_expired, LeaseAdmissionThresholds,
    LeaseBackpressureView, LongReadLease,
};
use crate::types::{TransactionId, TransactionOptions};

impl TransactionManager {
    fn lease_thresholds(&self) -> LeaseAdmissionThresholds {
        LeaseAdmissionThresholds {
            max_live_leases: self.config.long_read_max_live_leases,
            max_pin_secs: self.config.long_read_max_pin_secs,
        }
    }

    /// Begin a declared long read: poll backpressure first (refuse new long
    /// reads when over threshold; short reads bypass this entirely), then
    /// issue a lease floored at the snapshot timestamp with the deployment
    /// TTL clamped to the storage maximum. `now` is injected for tests.
    pub fn begin_long_read_transaction(
        &self,
        options: TransactionOptions,
        backpressure: LeaseBackpressureView,
        storage_max_ttl: Duration,
        now: Instant,
    ) -> Result<TransactionId, TransactionError> {
        check_long_read_admission(backpressure, self.lease_thresholds())?;
        let txn_id = self.begin_read_transaction(options)?;
        let context = self.get_context(txn_id)?;
        let ttl = clamp_lease_ttl(self.config.default_lease_ttl, storage_max_ttl);
        self.long_read_leases.insert(
            txn_id,
            LongReadLease {
                holder: txn_id,
                floor_ts: context.start_timestamp,
                deadline: now + ttl,
            },
        );
        Ok(txn_id)
    }

    /// Read-path expiry check: an expired lease returns the retryable
    /// snapshot error carrying holder plus floor; live or short reads pass.
    pub fn check_long_read(
        &self,
        txn_id: TransactionId,
        now: Instant,
    ) -> Result<(), TransactionError> {
        let Some(lease) = self.long_read_leases.get(&txn_id).map(|e| *e.value()) else {
            return Ok(());
        };
        if lease.is_live(now) {
            Ok(())
        } else {
            Err(snapshot_expired(lease.holder, lease.floor_ts))
        }
    }

    /// Heartbeat renewal on the same term. Returns false when no lease
    /// exists (the holder must issue rather than renew); an expired lease
    /// renews the deadline but the read path still reports expiry until
    /// the caller reissues after retry.
    pub fn renew_long_read(
        &self,
        txn_id: TransactionId,
        storage_max_ttl: Duration,
        now: Instant,
    ) -> bool {
        let ttl = clamp_lease_ttl(self.config.default_lease_ttl, storage_max_ttl);
        match self.long_read_leases.get_mut(&txn_id) {
            Some(mut lease) => {
                lease.deadline = now + ttl;
                true
            }
            None => false,
        }
    }

    /// Release the lease held by `txn_id`. Every termination path calls
    /// this; missing entries are not errors (short transactions).
    pub fn release_long_read(&self, txn_id: TransactionId) -> bool {
        self.long_read_leases.remove(&txn_id).is_some()
    }

    /// Reap leases expired at `now`, counting each one. Expired leases stop
    /// gating admission; the count keeps overrunning holders observable.
    pub fn reap_expired_long_reads(&self, now: Instant) -> usize {
        let expired: Vec<TransactionId> = self
            .long_read_leases
            .iter()
            .filter(|e| !e.value().is_live(now))
            .map(|e| *e.key())
            .collect();
        let count = expired.len();
        for id in expired {
            self.long_read_leases.remove(&id);
        }
        count
    }

    /// Live long-read count at `now` for admission polling.
    pub fn live_long_reads(&self, now: Instant) -> usize {
        self.long_read_leases
            .iter()
            .filter(|e| e.value().is_live(now))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TransactionManagerConfig;

    fn manager() -> TransactionManager {
        TransactionManager::new(TransactionManagerConfig::default())
    }

    fn view(live: usize, pinned: u64) -> LeaseBackpressureView {
        LeaseBackpressureView {
            live_leases: live,
            pinned_secs: pinned,
        }
    }

    #[test]
    fn test_long_read_issue_expiry_renew_release() {
        let manager = manager();
        let now = Instant::now();
        let max = Duration::from_secs(600);
        let txn = manager
            .begin_long_read_transaction(
                TransactionOptions::default().read_only().long_read(),
                view(0, 0),
                max,
                now,
            )
            .expect("issue");
        assert!(manager.check_long_read(txn, now).is_ok());
        let late = now + manager.config.default_lease_ttl + Duration::from_secs(1);
        let err = manager.check_long_read(txn, late).unwrap_err();
        assert!(err.is_retryable());
        assert!(err.message().contains(&txn.0.to_string()));
        assert!(manager.renew_long_read(txn, max, late));
        assert!(manager.check_long_read(txn, late).is_ok());
        assert!(manager.release_long_read(txn));
        assert!(!manager.release_long_read(txn));
        assert!(manager.check_long_read(txn, late).is_ok());
    }

    #[test]
    fn test_admission_refuses_new_long_reads() {
        let manager = manager();
        let now = Instant::now();
        let max = Duration::from_secs(600);
        let err = manager
            .begin_long_read_transaction(
                TransactionOptions::default().read_only().long_read(),
                view(usize::MAX, 0),
                max,
                now,
            )
            .unwrap_err();
        assert_eq!(
            err.kind(),
            crate::error::TransactionErrorKind::TooManyTransactions
        );
    }

    #[test]
    fn test_reap_expired_counts_and_unpins() {
        let manager = manager();
        let now = Instant::now();
        let max = Duration::from_secs(600);
        let first = manager
            .begin_long_read_transaction(
                TransactionOptions::default().read_only().long_read(),
                view(0, 0),
                max,
                now,
            )
            .expect("first");
        let _second = manager
            .begin_long_read_transaction(
                TransactionOptions::default().read_only().long_read(),
                view(0, 0),
                max,
                now,
            )
            .expect("second");
        assert_eq!(manager.live_long_reads(now), 2);
        let late = now + manager.config.default_lease_ttl + Duration::from_secs(1);
        assert_eq!(manager.reap_expired_long_reads(late), 2);
        assert_eq!(manager.live_long_reads(late), 0);
        assert!(manager.check_long_read(first, late).is_ok());
    }

    #[test]
    fn test_short_reads_hold_no_lease() {
        let manager = manager();
        let now = Instant::now();
        let txn = manager
            .begin_read_transaction(TransactionOptions::default())
            .expect("short read");
        assert!(manager.check_long_read(txn, now).is_ok());
        assert!(!manager.renew_long_read(txn, Duration::from_secs(60), now));
        manager.abort_transaction(txn).expect("abort");
    }
}
