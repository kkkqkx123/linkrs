use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Best-effort synchronous dispatch for event-hook observers.
///
/// Callbacks run inline on the emitter path, each isolated with
/// `catch_unwind` so a panicking observer cannot break the main flow
/// or starve later observers. Observers must return quickly, must not
/// hold locks across the call, and must not call back into the
/// emitting subsystem.
///
/// This is a notification hook: observers cannot veto or mutate the
/// event. Decision hooks (commit veto, query interrupt) and pipeline
/// hooks (parser/binder/planner extensions) are separate mechanisms.
pub fn dispatch_event_callbacks<E>(
    owner: &str,
    callbacks: &[Arc<dyn Fn(&E) + Send + Sync>],
    event: &E,
) -> usize {
    let mut panics = 0;
    for (index, callback) in callbacks.iter().enumerate() {
        if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            callback(event);
        })) {
            panics += 1;
            log::error!(
                "{} callback #{} panicked: {}; continuing dispatch",
                owner,
                index,
                panic_payload_message(&payload)
            );
        }
    }
    panics
}

/// Outcome of a filtered dispatch.
///
/// `panics` counts both panicking filters and panicking callbacks.
/// A panicking filter skips its observer without invoking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DispatchOutcome {
    pub delivered: usize,
    pub skipped_by_filter: usize,
    pub panics: usize,
}

/// Handle returned when registering an event-hook observer.
///
/// The id can be passed back to `EventSubscriptions::remove` to unsubscribe.
/// Registration only appends; removal is explicit so long-running processes
/// do not accumulate dead observers.
pub type SubscriptionId = u64;

/// Filter predicate for event-hook observers.
///
/// Returning `false` skips the observer for that event without invoking it.
pub type EventFilter<E> = Arc<dyn Fn(&E) -> bool + Send + Sync>;

struct SubscriptionEntry<E> {
    id: SubscriptionId,
    callback: Arc<dyn Fn(&E) + Send + Sync>,
    filter: Option<EventFilter<E>>,
}

/// Cancellable, optionally filtered registry for event-hook observers.
///
/// Synchronous best-effort notification semantics match
/// `dispatch_event_callbacks`: observers run inline on the emitter path,
/// each isolated with `catch_unwind`. Observers must return quickly, must
/// not hold locks across the call, and must not call back into the
/// emitting subsystem.
///
/// Reentrancy: `dispatch` snapshots the subscription list, releases the
/// internal lock, then evaluates filters and invokes callbacks. A filter
/// or callback may therefore register or remove observers without
/// deadlocking; registrations made during dispatch do not affect the
/// in-flight dispatch. A panicking filter skips its observer and is
/// counted the same as a panicking callback.
pub struct EventSubscriptions<E> {
    next_id: AtomicU64,
    entries: parking_lot::RwLock<Vec<SubscriptionEntry<E>>>,
}

impl<E> Default for EventSubscriptions<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E> EventSubscriptions<E> {
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(1),
            entries: parking_lot::RwLock::new(Vec::new()),
        }
    }

    /// Register an observer, returning its subscription id.
    pub fn add(&self, callback: Arc<dyn Fn(&E) + Send + Sync>) -> SubscriptionId {
        self.add_filtered(callback, None)
    }

    /// Register an observer with an optional filter predicate.
    pub fn add_filtered(
        &self,
        callback: Arc<dyn Fn(&E) + Send + Sync>,
        filter: Option<EventFilter<E>>,
    ) -> SubscriptionId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.entries.write().push(SubscriptionEntry {
            id,
            callback,
            filter,
        });
        id
    }

    /// Remove a previously registered observer. Returns true if present.
    pub fn remove(&self, id: SubscriptionId) -> bool {
        let mut entries = self.entries.write();
        let before = entries.len();
        entries.retain(|entry| entry.id != id);
        entries.len() != before
    }

    /// Remove all registered observers.
    pub fn clear(&self) {
        self.entries.write().clear();
    }

    /// Register an observer and receive an RAII guard that unsubscribes
    /// on drop. Useful for scoped subscriptions in tests and request
    /// handlers where manual `remove` is easy to forget.
    pub fn subscribe(&self, callback: Arc<dyn Fn(&E) + Send + Sync>) -> SubscriptionGuard<'_, E> {
        let id = self.add(callback);
        SubscriptionGuard::new(self, id)
    }

    /// Number of registered observers.
    pub fn len(&self) -> usize {
        self.entries.read().len()
    }

    /// Whether any observer is registered.
    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }

    /// Dispatch an event to matching observers with panic isolation.
    ///
    /// Returns the number of observers that panicked.
    pub fn dispatch(&self, owner: &str, event: &E) -> usize {
        self.dispatch_detailed(owner, event).panics
    }

    /// Dispatch with a full outcome breakdown.
    pub fn dispatch_detailed(&self, owner: &str, event: &E) -> DispatchOutcome {
        struct Snapshot<E> {
            callback: Arc<dyn Fn(&E) + Send + Sync>,
            filter: Option<EventFilter<E>>,
        }
        let snapshot: Vec<Snapshot<E>> = {
            let entries = self.entries.read();
            if entries.is_empty() {
                return DispatchOutcome::default();
            }
            entries
                .iter()
                .map(|entry| Snapshot {
                    callback: Arc::clone(&entry.callback),
                    filter: entry.filter.clone(),
                })
                .collect()
        };
        let mut outcome = DispatchOutcome::default();
        for (index, item) in snapshot.iter().enumerate() {
            let matched = match &item.filter {
                None => true,
                Some(filter) => {
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter(event))) {
                        Ok(matched) => matched,
                        Err(payload) => {
                            outcome.panics += 1;
                            outcome.skipped_by_filter += 1;
                            log::error!(
                                "{} filter #{} panicked: {}; skipping observer",
                                owner,
                                index,
                                panic_payload_message(&payload)
                            );
                            continue;
                        }
                    }
                }
            };
            if !matched {
                outcome.skipped_by_filter += 1;
                continue;
            }
            if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                (item.callback)(event);
            })) {
                outcome.panics += 1;
                log::error!(
                    "{} callback #{} panicked: {}; continuing dispatch",
                    owner,
                    index,
                    panic_payload_message(&payload)
                );
                continue;
            }
            outcome.delivered += 1;
        }
        outcome
    }
}

/// RAII unsubscription guard returned by `EventSubscriptions::subscribe`.
///
/// Dropping the guard removes the observer. Use `forget` to keep a
/// long-lived subscription, or `id` to inspect the subscription id.
pub struct SubscriptionGuard<'a, E> {
    registry: Option<&'a EventSubscriptions<E>>,
    id: SubscriptionId,
}

impl<'a, E> SubscriptionGuard<'a, E> {
    fn new(registry: &'a EventSubscriptions<E>, id: SubscriptionId) -> Self {
        Self {
            registry: Some(registry),
            id,
        }
    }

    /// Subscription id of the guarded observer.
    pub fn id(&self) -> SubscriptionId {
        self.id
    }

    /// Keep the subscription alive beyond the guard without removing it.
    pub fn forget(mut self) -> SubscriptionId {
        let id = self.id;
        self.registry = None;
        std::mem::forget(self);
        id
    }
}

impl<E> Drop for SubscriptionGuard<'_, E> {
    fn drop(&mut self) {
        if let Some(registry) = self.registry {
            registry.remove(self.id);
        }
    }
}

pub fn panic_payload_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn dispatch_reaches_all_and_isolates_panic() {
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let first = Arc::clone(&hits);
        registry.add(Arc::new(move |_| {
            first.fetch_add(1, Ordering::SeqCst);
        }));
        registry.add(Arc::new(|_: &u32| panic!("boom")));
        let last = Arc::clone(&hits);
        registry.add(Arc::new(move |_| {
            last.fetch_add(1, Ordering::SeqCst);
        }));
        let panics = registry.dispatch("test", &1);
        assert_eq!(panics, 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn remove_and_filter_are_honored() {
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        let id = registry.add(Arc::new(move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        assert_eq!(registry.len(), 1);
        assert!(registry.remove(id));
        assert!(!registry.remove(id));
        assert_eq!(registry.dispatch("test", &1), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 0);

        let even_probe = Arc::clone(&hits);
        registry.add_filtered(
            Arc::new(move |_| {
                even_probe.fetch_add(1, Ordering::SeqCst);
            }),
            Some(Arc::new(|event: &u32| *event % 2 == 0)),
        );
        assert_eq!(registry.dispatch("test", &1), 0);
        assert_eq!(registry.dispatch("test", &2), 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn panicking_filter_skips_observer_without_aborting_dispatch() {
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        let hits = Arc::new(AtomicUsize::new(0));
        let bad_probe = Arc::clone(&hits);
        registry.add_filtered(
            Arc::new(move |_| {
                bad_probe.fetch_add(1, Ordering::SeqCst);
            }),
            Some(Arc::new(|_: &u32| panic!("filter boom"))),
        );
        let good_probe = Arc::clone(&hits);
        registry.add(Arc::new(move |_| {
            good_probe.fetch_add(1, Ordering::SeqCst);
        }));
        let outcome = registry.dispatch_detailed("test", &1);
        assert_eq!(outcome.panics, 1);
        assert_eq!(outcome.delivered, 1);
        assert_eq!(outcome.skipped_by_filter, 1);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispatch_does_not_hold_lock_during_callbacks() {
        let registry: Arc<EventSubscriptions<u32>> = Arc::new(EventSubscriptions::new());
        let nested: Arc<EventSubscriptions<u32>> = Arc::clone(&registry);
        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        registry.add(Arc::new(move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
            nested.add(Arc::new(|_| {}));
        }));
        let nested_filter: Arc<EventSubscriptions<u32>> = Arc::clone(&registry);
        registry.add_filtered(
            Arc::new(|_| {}),
            Some(Arc::new(move |_| {
                nested_filter.len();
                true
            })),
        );
        let outcome = registry.dispatch_detailed("test", &1);
        assert_eq!(outcome.delivered, 2);
        assert_eq!(outcome.panics, 0);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn subscription_guard_removes_on_drop_and_clear_empties() {
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        {
            let _guard = registry.subscribe(Arc::new(|_: &u32| {}));
            assert_eq!(registry.len(), 1);
        }
        assert_eq!(registry.len(), 0);
        registry.add(Arc::new(|_: &u32| {}));
        registry.add(Arc::new(|_: &u32| {}));
        assert_eq!(registry.len(), 2);
        registry.clear();
        assert!(registry.is_empty());
    }
}
