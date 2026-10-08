//! Transaction manager behavior: event subscriptions, commit vetoes and notification fan-out

use std::sync::atomic::Ordering;
use std::sync::Arc;

use super::TransactionManager;
use crate::context::TransactionContext;
use crate::types::*;
use linkrs_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
impl TransactionManager {
    fn is_commit_event(event: &TransactionEvent) -> bool {
        matches!(
            event,
            TransactionEvent::Committed { .. }
                | TransactionEvent::CommitDurableButUnfinalized { .. }
        )
    }

    fn is_rollback_event(event: &TransactionEvent) -> bool {
        matches!(event, TransactionEvent::Aborted { .. })
    }

    fn is_budget_warning_event(event: &TransactionEvent) -> bool {
        matches!(event, TransactionEvent::BudgetWarning { .. })
    }

    /// Register an observer for every transaction lifecycle event.
    pub fn register_txn_callback(&self, callback: TxnCallback) -> SubscriptionId {
        self.txn_callbacks.add(callback)
    }

    /// Register a filtered observer over every transaction lifecycle event.
    pub fn register_txn_callback_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        self.txn_callbacks.add_filtered(callback, Some(filter))
    }

    /// Remove a previously registered transaction observer. Returns true if present.
    pub fn unregister_txn_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total number of observers on the unified transaction registry (all kinds).
    pub fn txn_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Shared transaction-event registry behind this manager.
    pub fn shared_txn_callbacks(&self) -> Arc<EventSubscriptions<TransactionEvent>> {
        Arc::clone(&self.txn_callbacks)
    }

    /// Register a pre-commit decision hook.
    ///
    /// Evaluated synchronously in registration order after conflict
    /// certification and before any WAL I/O. The first veto blocks the
    /// commit; a panicking hook is logged and treated as allow. A vetoed
    /// commit fails with a `CommitVetoed` error and leaves the transaction
    /// active: the caller must roll it back.
    pub fn register_commit_veto(&self, callback: CommitVetoCallback) -> SubscriptionId {
        let id = self.next_veto_id.fetch_add(1, Ordering::SeqCst);
        self.commit_vetoes.write().push((id, callback));
        id
    }

    /// Remove a previously registered commit veto. Returns true if present.
    pub fn unregister_commit_veto(&self, id: SubscriptionId) -> bool {
        let mut vetoes = self.commit_vetoes.write();
        let before = vetoes.len();
        vetoes.retain(|(entry_id, _)| *entry_id != id);
        vetoes.len() != before
    }

    /// Number of registered commit vetoes.
    pub fn commit_veto_count(&self) -> usize {
        self.commit_vetoes.read().len()
    }

    /// Evaluate vetoes against `context`, returning the first veto reason.
    ///
    /// Snapshots the hook list under the lock, then invokes callbacks
    /// without holding it. Never vetoes when no hook is registered
    /// (zero overhead beyond one lock acquisition).
    pub(crate) fn eval_commit_vetoes(&self, context: &TransactionContext) -> Option<String> {
        let vetoes: Vec<CommitVetoCallback> = {
            let guard = self.commit_vetoes.read();
            if guard.is_empty() {
                return None;
            }
            guard.iter().map(|(_, cb)| Arc::clone(cb)).collect()
        };
        let view = CommitVetoContext {
            txn_id: context.id,
            write_timestamp: context.timestamp(),
        };
        for (index, callback) in vetoes.iter().enumerate() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(&view)));
            match outcome {
                Ok(decision) if decision.veto => {
                    let reason = decision
                        .reason
                        .unwrap_or_else(|| "commit vetoed".to_string());
                    log::warn!(
                        "commit veto #{} blocked transaction {}: {}",
                        index,
                        view.txn_id,
                        reason,
                    );
                    return Some(reason);
                }
                Ok(_) => {}
                Err(_) => {
                    log::error!("commit veto #{} panicked; treating as allow", index,);
                }
            }
        }
        None
    }

    /// Register a commit observer (compatibility vest: only
    /// `Committed | CommitDurableButUnfinalized` pass the built-in filter).
    pub fn register_commit_callback(&self, callback: CommitCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_commit_event)))
    }

    /// Register a filtered commit observer invoked only when both the
    /// built-in commit filter and `filter` return true.
    pub fn register_commit_callback_filtered(
        &self,
        callback: CommitCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_commit_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered commit observer. Returns true if present.
    pub fn unregister_commit_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total observers on the unified registry (all kinds, not only commits).
    pub fn commit_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Register a rollback observer (compatibility vest: only `Aborted`
    /// passes the built-in filter).
    pub fn register_rollback_callback(&self, callback: RollbackCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_rollback_event)))
    }

    /// Register a filtered rollback observer invoked only when both the
    /// built-in rollback filter and `filter` return true.
    pub fn register_rollback_callback_filtered(
        &self,
        callback: RollbackCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_rollback_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered rollback observer. Returns true if present.
    pub fn unregister_rollback_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    /// Total observers on the unified registry (all kinds, not only rollbacks).
    pub fn rollback_callback_count(&self) -> usize {
        self.txn_callbacks.len()
    }

    /// Register a budget-warning observer (only `BudgetWarning` passes).
    pub fn register_budget_warning_callback(&self, callback: TxnCallback) -> SubscriptionId {
        self.txn_callbacks
            .add_filtered(callback, Some(Arc::new(Self::is_budget_warning_event)))
    }

    /// Register a filtered budget-warning observer invoked only when both
    /// the built-in budget-warning filter and `filter` return true.
    pub fn register_budget_warning_callback_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| Self::is_budget_warning_event(event) && filter(event));
        self.txn_callbacks.add_filtered(callback, Some(combined))
    }

    /// Remove a previously registered budget-warning observer.
    pub fn unregister_budget_warning_callback(&self, id: SubscriptionId) -> bool {
        self.txn_callbacks.remove(id)
    }

    pub(crate) fn emit_commit_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("commit", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    pub(crate) fn emit_rollback_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("rollback", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    pub(crate) fn emit_budget_warning_event(&self, event: TransactionEvent) {
        if self.txn_callbacks.is_empty() {
            return;
        }
        let outcome = self.txn_callbacks.dispatch_detailed("txn-budget", &event);
        self.stats
            .record_hook_dispatch(outcome.delivered, outcome.panics);
        for _ in 0..outcome.panics {
            self.stats.increment_cleanup_failure();
        }
    }

    /// Drain pending budget warnings from a context and fan them out as
    /// `TransactionEvent::BudgetWarning` through the unified registry
    /// (budget-warning observers only).
    pub fn drain_context_budget_warnings(&self, context: &TransactionContext) {
        for event in context.drain_budget_warnings() {
            self.emit_budget_warning_event(event);
        }
    }
}
