//! Central HookBus: unified subscription facade over leaf event registries.
//!
//! Notification hooks only: observers run synchronously inline on the emitter
//! path with best-effort `catch_unwind` isolation (see
//! `graphdb_core::event_dispatch`). The one decision hook (commit veto) lives
//! on the transaction manager (`register_commit_veto`); the C-API
//! `graphdb_commit_hook` is bridged into that same registry, so C and Rust
//! vetoes share one evaluation point with first-veto-wins semantics.
//!
//! The bus forwards subscriptions to the leaf registries and hands out its own
//! bus-level ids, keeping a `bus id -> (leaf registry, leaf id)` map so one
//! `unsubscribe` stops delivery everywhere. Leaf `dispatch` paths are
//! untouched: no event data moves, no async channel.
//!
//! Scope: registries visible from the embedded layer (schema, transaction,
//! storage, index). The server-side `GraphSessionManager` registry is
//! aggregated by the server layer, not here; a session/query registry can be
//! attached later via `attach_session_registry` once the executor assembly
//! owns both halves.
//!
//! A `None` return from `subscribe_*` means that domain's registry is not
//! attached in this backend (e.g. mocks, disabled fulltext): it signals
//! "unavailable here", not "no events are happening". Use the
//! `has_*_registry` guards to branch explicitly instead of ignoring it.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use graphdb_core::event_dispatch::{EventFilter, EventSubscriptions, SubscriptionId};
use graphdb_core::metadata::{SchemaChangeCallback, SchemaChangeEvent};
use graphdb_query::{
    DmlOp, DmlStatementCallback, DmlStatementEvent, SessionEvent, SessionEventCallback,
};
use graphdb_transaction::{RollbackCallback, TransactionEvent, TxnCallback};
use parking_lot::RwLock;

/// Where one bus-level subscription fans out to.
enum BusTarget {
    Schema(Arc<EventSubscriptions<SchemaChangeEvent>>, SubscriptionId),
    Txn(Arc<EventSubscriptions<TransactionEvent>>, SubscriptionId),
    Session(Arc<EventSubscriptions<SessionEvent>>, SubscriptionId),
    Storage(
        Arc<EventSubscriptions<graphdb_storage::StorageEvent>>,
        SubscriptionId,
    ),
    Index(
        Arc<EventSubscriptions<graphdb_fulltext::IndexEvent>>,
        SubscriptionId,
    ),
    Dml(Arc<EventSubscriptions<DmlStatementEvent>>, SubscriptionId),
}

/// Unified subscription entry point for embedded event hooks.
pub struct HookBus {
    next_id: AtomicU64,
    targets: RwLock<HashMap<SubscriptionId, BusTarget>>,
    txn_events: Arc<EventSubscriptions<TransactionEvent>>,
    schema_events: Option<Arc<EventSubscriptions<SchemaChangeEvent>>>,
    session_events: RwLock<Option<Arc<EventSubscriptions<SessionEvent>>>>,
    storage_events: RwLock<Option<Arc<EventSubscriptions<graphdb_storage::StorageEvent>>>>,
    index_events: RwLock<Option<Arc<EventSubscriptions<graphdb_fulltext::IndexEvent>>>>,
    /// Statement-level DML registry, owned by the bus itself: no other
    /// manager emits these, so no attach step is needed. Always present,
    /// zero overhead when empty.
    dml_events: Arc<EventSubscriptions<DmlStatementEvent>>,
}

impl HookBus {
    /// Build a bus over the unified transaction registry and an optional
    /// shared schema registry (`None` for storages without one, e.g. mocks).
    /// Storage/index/session registries attach later via `attach_*`.
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
            storage_events: RwLock::new(None),
            index_events: RwLock::new(None),
            dml_events: Arc::new(EventSubscriptions::new()),
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

    /// Attach the shared storage-lifecycle registry (persistence coordinator).
    pub fn attach_storage_registry(
        &self,
        shared: Arc<EventSubscriptions<graphdb_storage::StorageEvent>>,
    ) {
        *self.storage_events.write() = Some(shared);
    }

    /// Attach the shared fulltext/vector index-lifecycle registry.
    pub fn attach_index_registry(
        &self,
        shared: Arc<EventSubscriptions<graphdb_fulltext::IndexEvent>>,
    ) {
        *self.index_events.write() = Some(shared);
    }

    /// Whether a schema registry is attached in this backend.
    pub fn has_schema_registry(&self) -> bool {
        self.schema_events.is_some()
    }

    /// Whether a session/query registry has been attached.
    pub fn has_session_registry(&self) -> bool {
        self.session_events.read().is_some()
    }

    /// Whether a storage-lifecycle registry has been attached.
    pub fn has_storage_registry(&self) -> bool {
        self.storage_events.read().is_some()
    }

    /// Whether an index-lifecycle registry has been attached.
    pub fn has_index_registry(&self) -> bool {
        self.index_events.read().is_some()
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
            BusTarget::Storage(registry, leaf) => registry.remove(leaf),
            BusTarget::Index(registry, leaf) => registry.remove(leaf),
            BusTarget::Dml(registry, leaf) => registry.remove(leaf),
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
        let combined: EventFilter<TransactionEvent> = Arc::new(move |event| {
            matches!(event, TransactionEvent::BudgetWarning { .. }) && filter(event)
        });
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

    /// Subscribe to storage lifecycle events (checkpoint/WAL/GC/compaction).
    /// Returns `None` until a coordinator registry is attached via
    /// `attach_storage_registry`.
    pub fn subscribe_storage(
        &self,
        callback: graphdb_storage::StorageEventCallback,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.storage_events.read().as_ref()?);
        let leaf = registry.add(callback);
        Some(self.insert(BusTarget::Storage(registry, leaf)))
    }

    /// Filtered storage subscription. Returns `None` without an attached registry.
    pub fn subscribe_storage_filtered(
        &self,
        callback: graphdb_storage::StorageEventCallback,
        filter: EventFilter<graphdb_storage::StorageEvent>,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.storage_events.read().as_ref()?);
        let leaf = registry.add_filtered(callback, Some(filter));
        Some(self.insert(BusTarget::Storage(registry, leaf)))
    }

    /// Subscribe to index lifecycle events (fulltext/vector build/drop/merge).
    /// Returns `None` until an index registry is attached via
    /// `attach_index_registry`.
    pub fn subscribe_index(
        &self,
        callback: graphdb_fulltext::IndexEventCallback,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.index_events.read().as_ref()?);
        let leaf = registry.add(callback);
        Some(self.insert(BusTarget::Index(registry, leaf)))
    }

    /// Filtered index subscription. Returns `None` without an attached registry.
    pub fn subscribe_index_filtered(
        &self,
        callback: graphdb_fulltext::IndexEventCallback,
        filter: EventFilter<graphdb_fulltext::IndexEvent>,
    ) -> Option<SubscriptionId> {
        let registry = Arc::clone(self.index_events.read().as_ref()?);
        let leaf = registry.add_filtered(callback, Some(filter));
        Some(self.insert(BusTarget::Index(registry, leaf)))
    }

    /// Subscribe to statement-level DML notifications (always available;
    /// the bus owns this registry, so no attach step exists).
    pub fn subscribe_dml(&self, callback: DmlStatementCallback) -> SubscriptionId {
        let registry = Arc::clone(&self.dml_events);
        let leaf = registry.add(callback);
        self.insert(BusTarget::Dml(registry, leaf))
    }

    /// Filtered DML subscription (e.g. only `DmlOp::Delete`).
    pub fn subscribe_dml_filtered(
        &self,
        callback: DmlStatementCallback,
        filter: EventFilter<DmlStatementEvent>,
    ) -> SubscriptionId {
        let registry = Arc::clone(&self.dml_events);
        let leaf = registry.add_filtered(callback, Some(filter));
        self.insert(BusTarget::Dml(registry, leaf))
    }
    /// Emit one statement-level DML notification. No-op when nobody listens.
    pub(crate) fn emit_dml(&self, op: DmlOp, space_name: &str, rows: u64) {
        if self.dml_events.is_empty() {
            return;
        }
        self.dml_events.dispatch(
            "dml",
            &DmlStatementEvent {
                op,
                space_name: space_name.to_string(),
                rows,
            },
        );
    }

    /// Whether any statement-level DML observer is registered.
    pub(crate) fn has_dml_observers(&self) -> bool {
        !self.dml_events.is_empty()
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

    #[test]
    fn storage_and_index_subscription_require_attached_registries() {
        use graphdb_fulltext::IndexEvent;
        use graphdb_storage::StorageEvent;

        let bus = HookBus::new(
            Arc::new(EventSubscriptions::<TransactionEvent>::new()),
            None,
        );
        assert!(!bus.has_schema_registry());
        assert!(!bus.has_session_registry());
        assert!(!bus.has_storage_registry());
        assert!(!bus.has_index_registry());
        assert!(bus.subscribe_storage(Arc::new(|_| {})).is_none());
        assert!(bus.subscribe_index(Arc::new(|_| {})).is_none());

        let storage_shared = Arc::new(EventSubscriptions::<StorageEvent>::new());
        let index_shared = Arc::new(EventSubscriptions::<IndexEvent>::new());
        bus.attach_storage_registry(Arc::clone(&storage_shared));
        bus.attach_index_registry(Arc::clone(&index_shared));
        assert!(bus.has_storage_registry());
        assert!(bus.has_index_registry());

        let storage_hits = Arc::new(AtomicUsize::new(0));
        let index_hits = Arc::new(AtomicUsize::new(0));
        let storage_probe = Arc::clone(&storage_hits);
        let index_probe = Arc::clone(&index_hits);
        let storage_id = bus
            .subscribe_storage(Arc::new(move |_| {
                storage_probe.fetch_add(1, Ordering::SeqCst);
            }))
            .expect("attached storage registry serves subscriptions");
        let index_id = bus
            .subscribe_index_filtered(
                Arc::new(move |_| {
                    index_probe.fetch_add(1, Ordering::SeqCst);
                }),
                Arc::new(|event| matches!(event, IndexEvent::FulltextRefresh { .. })),
            )
            .expect("attached index registry serves subscriptions");
        assert_eq!(bus.subscription_count(), 2);

        storage_shared.dispatch(
            "test",
            &StorageEvent::GcRun {
                reclaimed_entries: 7,
            },
        );
        index_shared.dispatch(
            "test",
            &IndexEvent::FulltextBuildStarted {
                index_name: "idx".to_string(),
            },
        );
        index_shared.dispatch(
            "test",
            &IndexEvent::FulltextRefresh {
                index_name: "idx".to_string(),
            },
        );
        assert_eq!(storage_hits.load(Ordering::SeqCst), 1);
        // Filtered index subscription only sees the refresh.
        assert_eq!(index_hits.load(Ordering::SeqCst), 1);

        assert!(bus.unsubscribe(storage_id));
        assert!(bus.unsubscribe(index_id));
        assert_eq!(bus.subscription_count(), 0);
        storage_shared.dispatch(
            "test",
            &StorageEvent::GcRun {
                reclaimed_entries: 1,
            },
        );
        assert_eq!(storage_hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dml_notifications_carry_op_space_and_rows() {
        use graphdb_query::DmlOp;

        let bus = HookBus::new(
            Arc::new(EventSubscriptions::<TransactionEvent>::new()),
            None,
        );
        let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let probe = Arc::clone(&seen);
        let all_id = bus.subscribe_dml(Arc::new(move |event| {
            probe
                .lock()
                .push((event.op, event.space_name.clone(), event.rows));
        }));
        let delete_hits = Arc::new(AtomicUsize::new(0));
        let delete_probe = Arc::clone(&delete_hits);
        let delete_id = bus.subscribe_dml_filtered(
            Arc::new(move |_| {
                delete_probe.fetch_add(1, Ordering::SeqCst);
            }),
            Arc::new(|event| event.op == DmlOp::Delete),
        );

        bus.emit_dml(DmlOp::Insert, "users", 3);
        bus.emit_dml(DmlOp::Delete, "users", 1);
        assert_eq!(
            *seen.lock(),
            vec![
                (DmlOp::Insert, "users".to_string(), 3),
                (DmlOp::Delete, "users".to_string(), 1),
            ]
        );
        assert_eq!(delete_hits.load(Ordering::SeqCst), 1);

        assert!(bus.unsubscribe(all_id));
        assert!(bus.unsubscribe(delete_id));
        assert_eq!(bus.subscription_count(), 0);
    }
}
