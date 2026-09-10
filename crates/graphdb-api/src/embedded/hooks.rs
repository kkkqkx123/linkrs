//! Central HookBus: unified subscription facade over leaf event registries.
//!
//! Notification hooks only: observers run synchronously inline on the emitter
//! path with best-effort `catch_unwind` isolation (see
//! `graphdb_core::event_dispatch`). Decision hooks (C-API `commit_hook` veto
//! semantics) are a separate mechanism and are explicitly excluded here.
//!
//! The bus forwards subscriptions to the leaf registries and hands out its own
//! bus-level ids, keeping a `bus id -> (leaf registry, leaf id)` map so one
//! `unsubscribe` stops delivery everywhere. Leaf `dispatch` paths are
//! untouched: no event data moves, no async channel.
//!
//! Scope: registries visible from the embedded layer (schema, transaction).
//! The server-side `GraphSessionManager` registry is aggregated by the server
//! layer, not here; a session/query registry can be attached later via
//! `attach_session_registry` once the executor assembly owns both halves.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use graphdb_core::metadata::{SchemaChangeCallback, SchemaChangeEvent};
use graphdb_query::{SessionEvent, SessionEventCallback};
use graphdb_transaction::{RollbackCallback, TransactionEvent, TxnCallback};
use parking_lot::RwLock;

/// Where one bus-level subscription fans out to.
enum BusTarget {
    Schema(Arc<EventSubscriptions<SchemaChangeEvent>>, SubscriptionId),
    Txn(Arc<EventSubscriptions<TransactionEvent>>, SubscriptionId),
    Session(Arc<EventSubscriptions<SessionEvent>>, SubscriptionId),
}

/// Unified subscription entry point for embedded event hooks.
pub struct HookBus {
    next_id: AtomicU64,
    targets: RwLock<HashMap<SubscriptionId, BusTarget>>,
    txn_events: Arc<EventSubscriptions<TransactionEvent>>,
    schema_events: Option<Arc<EventSubscriptions<SchemaChangeEvent>>>,
    session_events: RwLock<Option<Arc<EventSubscriptions<SessionEvent>>>>,
}

impl HookBus {
    /// Build a bus over the unified transaction registry and an optional
    /// shared schema registry (`None` for storages without one, e.g. mocks).
    pub fn new(
        txn_events: Arc<EventSubscriptions<TransactionEvent>>,
        schema_events: Option<Arc<EventSubscriptions<SchemaChangeEvent>>>,
    ) -> Self {
        Self {
            next_id: AtomicU64::new(1),
            targets: RwLock::new(HashMap::new()),
            txn_events,
            schema_events,
            session_events: RwLock::new(None),
        }
    }

    /// Build a bus directly from a transaction manager.
    pub fn with_txn_manager(
        manager: &graphdb_transaction::TransactionManager,
        schema_events: Option<Arc<EventSubscriptions<SchemaChangeEvent>>>,
    ) -> Self {
        Self::new(manager.shared_txn_callbacks(), schema_events)
    }

    /// Attach the shared session/query registry once the executor assembly
    /// owns both emission halves.
    pub fn attach_session_registry(&self, shared: Arc<EventSubscriptions<SessionEvent>>) {
        *self.session_events.write() = Some(shared);
    }

    fn insert(&self, target: BusTarget) -> SubscriptionId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.targets.write().insert(id, target);
        id
    }

    /// Remove a bus-level subscription, unsubscribing from the leaf registry.
    /// Idempotent: unknown ids return false.
    pub fn unsubscribe(&self, id: SubscriptionId) -> bool {
        let target = match self.targets.write().remove(&id) {
            Some(target) => target,
            None => return false,
        };
        match target {
            BusTarget::Schema(registry, leaf) => registry.remove(leaf),
            BusTarget::Txn(registry, leaf) => registry.remove(leaf),
            BusTarget::Session(registry, leaf) => registry.remove(leaf),
        }
    }

    /// Number of live bus-level subscriptions.
    pub fn subscription_count(&self) -> usize {
        self.targets.read().len()
    }

    /// Subscribe to schema changes (table/space + index DDL via the shared registry).
    /// Returns `None` when no schema registry is attached.
    pub fn subscribe_schema(&self, callback: SchemaChangeCallback) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.schema_events.as_ref()?);
        let leaf = registry.add(callback);
        Some(self.insert(BusTarget::Schema(registry, leaf)))
    }

    /// Filtered schema subscription. Returns `None` without a schema registry.
    pub fn subscribe_schema_filtered(
        &self,
        callback: SchemaChangeCallback,
        filter: EventFilter<SchemaChangeEvent>,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.schema_events.as_ref()?);
        let leaf = registry.add_filtered(callback, Some(filter));
        Some(self.insert(BusTarget::Schema(registry, leaf)))
    }

    /// Subscribe to every transaction lifecycle event.
    pub fn subscribe_txn(&self, callback: TxnCallback) -> SubscriptionId {
        let registry = Arc::clone(&self.txn_events);
        let leaf = registry.add(callback);
        self.insert(BusTarget::Txn(registry, leaf))
    }

    /// Filtered transaction subscription.
    pub fn subscribe_txn_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let registry = Arc::clone(&self.txn_events);
        let leaf = registry.add_filtered(callback, Some(filter));
        self.insert(BusTarget::Txn(registry, leaf))
    }

    /// Commit-only vest (only `Committed | CommitDurableButUnfinalized` pass).
    pub fn subscribe_commit(&self, callback: TxnCallback) -> SubscriptionId {
        self.subscribe_txn_filtered(
            callback,
            Arc::new(|event| {
                matches!(
                    event,
                    TransactionEvent::Committed { .. }
                        | TransactionEvent::CommitDurableButUnfinalized { .. }
                )
            }),
        )
    }

    /// Rollback-only vest (only `Aborted` passes).
    pub fn subscribe_rollback(&self, callback: RollbackCallback) -> SubscriptionId {
        self.subscribe_txn_filtered(
            callback,
            Arc::new(|event| matches!(event, TransactionEvent::Aborted { .. })),
        )
    }

    /// Budget-warning-only subscription (never mixed into the commit channel).
    pub fn subscribe_budget_warning(&self, callback: TxnCallback) -> SubscriptionId {
        self.subscribe_txn_filtered(
            callback,
            Arc::new(|event| matches!(event, TransactionEvent::BudgetWarning { .. })),
        )
    }

    /// Filtered budget-warning subscription (built-in `BudgetWarning` filter
    /// combined with `filter`).
    pub fn subscribe_budget_warning_filtered(
        &self,
        callback: TxnCallback,
        filter: EventFilter<TransactionEvent>,
    ) -> SubscriptionId {
        let combined: EventFilter<TransactionEvent> =
            Arc::new(move |event| matches!(event, TransactionEvent::BudgetWarning { .. }) && filter(event));
        self.subscribe_txn_filtered(callback, combined)
    }

    /// Subscribe to session/query events. Returns `None` until a shared
    /// session registry is attached via `attach_session_registry`.
    pub fn subscribe_session(&self, callback: SessionEventCallback) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.session_events.read().as_ref()?);
        let leaf = registry.add(callback);
        Some(self.insert(BusTarget::Session(registry, leaf)))
    }

    /// Filtered session subscription. Returns `None` without an attached registry.
    pub fn subscribe_session_filtered(
        &self,
        callback: SessionEventCallback,
        filter: EventFilter<SessionEvent>,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.session_events.read().as_ref()?);
        let leaf = registry.add_filtered(callback, Some(filter));
        Some(self.insert(BusTarget::Session(registry, leaf)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::metadata::{IndexManager, IndexMetadataManager, SchemaManager};
    use graphdb_core::types::{Index, IndexConfig, IndexType, SpaceInfo};
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn one_bus_subscription_receives_schema_and_txn() {
        use graphdb_transaction::TransactionId;

        let shared = Arc::new(EventSubscriptions::<SchemaChangeEvent>::new());
        let schema_manager = SchemaManager::with_shared_schema_callbacks(Arc::clone(&shared));
        let index_manager = IndexManager::with_shared_schema_callbacks(Arc::clone(&shared));
        let bus = HookBus::new(
            Arc::new(EventSubscriptions::<TransactionEvent>::new()),
            Some(shared),
        );

        let schema_hits = Arc::new(AtomicUsize::new(0));
        let txn_hits = Arc::new(AtomicUsize::new(0));
        let schema_probe = Arc::clone(&schema_hits);
        let txn_probe = Arc::clone(&txn_hits);

        let schema_id = bus
            .subscribe_schema(Arc::new(move |_| {
                schema_probe.fetch_add(1, Ordering::SeqCst);
            }))
            .expect("schema registry attached");
        let txn_id = bus.subscribe_txn(Arc::new(move |_| {
            txn_probe.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(bus.subscription_count(), 2);

        let mut space = SpaceInfo::new("bus_space".to_string());
        schema_manager.create_space(&mut space).unwrap();
        let index = Index::new(IndexConfig {
            id: 0,
            name: "bus_idx".to_string(),
            space_id: space.space_id,
            schema_name: "t".to_string(),
            fields: Vec::new(),
            properties: Vec::new(),
            index_type: IndexType::TagIndex,
            is_unique: false,
            covering: false,
            partial_condition: None,
        });
        index_manager
            .create_tag_index(space.space_id, &index)
            .unwrap();
        bus.txn_events.dispatch(
            "test",
            &TransactionEvent::Aborted {
                txn_id: TransactionId(1),
                write_timestamp: 1,
            },
        );

        // One schema subscription got both DDL halves; one txn subscription fired.
        assert_eq!(schema_hits.load(Ordering::SeqCst), 2);
        assert_eq!(txn_hits.load(Ordering::SeqCst), 1);

        assert!(bus.unsubscribe(schema_id));
        assert!(bus.unsubscribe(txn_id));
        assert!(!bus.unsubscribe(txn_id));
        assert_eq!(bus.subscription_count(), 0);
    }

    #[test]
    fn session_subscription_requires_attached_registry() {
        let bus = HookBus::new(
            Arc::new(EventSubscriptions::<TransactionEvent>::new()),
            None,
        );
        assert!(bus.subscribe_schema(Arc::new(|_| {})).is_none());
        assert!(bus.subscribe_session(Arc::new(|_| {})).is_none());

        bus.attach_session_registry(Arc::new(EventSubscriptions::<SessionEvent>::new()));
        let id = bus
            .subscribe_session(Arc::new(|_| {}))
            .expect("attached registry serves subscriptions");
        assert!(bus.unsubscribe(id));
    }
}
