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
///
/// Retention rule: callbacks must not retain `&E` or any borrow derived
/// from it beyond the dispatch call. The event (and heavy payloads it may
/// carry, e.g. a transaction write set) is only valid for the duration of
/// the callback; observers needing data afterwards must clone explicitly
/// and pay that cost themselves. Prefer filtered subscriptions over
/// cloning the full event stream.
/// Event-hook observer callbacks run inline on the emitter path.
pub type EventCallbacks<E> = [Arc<dyn Fn(&E) + Send + Sync>];

pub fn dispatch_event_callbacks<E>(owner: &str, callbacks: &EventCallbacks<E>, event: &E) -> usize {
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

/// Outcome of a timed dispatch (`dispatch_timed`).
///
/// `outcome` is the standard breakdown; `slow` counts callbacks whose
/// wall time reached `slow_threshold`, and `slowest_ms` is the maximum
/// observed callback latency in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TimedDispatchOutcome {
    pub outcome: DispatchOutcome,
    pub slow: usize,
    pub slowest_ms: u64,
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

struct SnapshotEntry<E> {
    callback: Arc<dyn Fn(&E) + Send + Sync>,
    filter: Option<EventFilter<E>>,
}

enum FilterEval {
    Matched,
    Skip,
    Panicked,
}

fn eval_filter<E>(
    owner: &str,
    index: usize,
    filter: Option<&EventFilter<E>>,
    event: &E,
) -> FilterEval {
    match filter {
        None => FilterEval::Matched,
        Some(filter) => {
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| filter(event))) {
                Ok(true) => FilterEval::Matched,
                Ok(false) => FilterEval::Skip,
                Err(payload) => {
                    log::error!(
                        "{} filter #{} panicked: {}; skipping observer",
                        owner,
                        index,
                        panic_payload_message(&payload)
                    );
                    FilterEval::Panicked
                }
            }
        }
    }
}

fn invoke_callback<E>(
    callback: &dyn Fn(&E),
    event: &E,
) -> Result<(), Box<dyn std::any::Any + Send>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)))
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

    fn snapshot_locked(&self) -> Vec<SnapshotEntry<E>> {
        let entries = self.entries.read();
        if entries.is_empty() {
            return Vec::new();
        }
        entries
            .iter()
            .map(|entry| SnapshotEntry {
                callback: Arc::clone(&entry.callback),
                filter: entry.filter.clone(),
            })
            .collect()
    }

    /// Dispatch with a full outcome breakdown.
    pub fn dispatch_detailed(&self, owner: &str, event: &E) -> DispatchOutcome {
        let snapshot = self.snapshot_locked();
        if snapshot.is_empty() {
            return DispatchOutcome::default();
        }
        let mut outcome = DispatchOutcome::default();
        for (index, item) in snapshot.iter().enumerate() {
            match eval_filter(owner, index, item.filter.as_ref(), event) {
                FilterEval::Skip => {
                    outcome.skipped_by_filter += 1;
                    continue;
                }
                FilterEval::Panicked => {
                    outcome.panics += 1;
                    outcome.skipped_by_filter += 1;
                    continue;
                }
                FilterEval::Matched => {}
            }
            match invoke_callback(item.callback.as_ref(), event) {
                Ok(()) => {
                    outcome.delivered += 1;
                }
                Err(payload) => {
                    outcome.panics += 1;
                    log::error!(
                        "{} callback #{} panicked: {}; continuing dispatch",
                        owner,
                        index,
                        panic_payload_message(&payload)
                    );
                    continue;
                }
            }
        }
        outcome
    }

    /// Timed dispatch: like `dispatch_detailed`, additionally measuring each
    /// callback's wall time. Callbacks reaching `slow_threshold` are counted
    /// in `slow`, contribute to `slowest_ms`, and emit a `log::warn` so slow
    /// observers are visible without affecting delivery.
    ///
    /// Timing costs one `Instant::now()` pair per delivered callback, so
    /// hot paths should keep using `dispatch`/`dispatch_detailed` and opt
    /// into this only for diagnostics or slow-path emission points.
    pub fn dispatch_timed(
        &self,
        owner: &str,
        event: &E,
        slow_threshold: std::time::Duration,
    ) -> TimedDispatchOutcome {
        let snapshot = self.snapshot_locked();
        if snapshot.is_empty() {
            return TimedDispatchOutcome::default();
        }
        let mut result = TimedDispatchOutcome::default();
        for (index, item) in snapshot.iter().enumerate() {
            match eval_filter(owner, index, item.filter.as_ref(), event) {
                FilterEval::Skip => {
                    result.outcome.skipped_by_filter += 1;
                    continue;
                }
                FilterEval::Panicked => {
                    result.outcome.panics += 1;
                    result.outcome.skipped_by_filter += 1;
                    continue;
                }
                FilterEval::Matched => {}
            }
            let start = std::time::Instant::now();
            match invoke_callback(item.callback.as_ref(), event) {
                Ok(()) => {
                    result.outcome.delivered += 1;
                }
                Err(payload) => {
                    result.outcome.panics += 1;
                    log::error!(
                        "{} callback #{} panicked: {}; continuing dispatch",
                        owner,
                        index,
                        panic_payload_message(&payload)
                    );
                }
            }
            let elapsed = start.elapsed();
            if elapsed >= slow_threshold {
                result.slow += 1;
                let elapsed_ms = elapsed.as_millis() as u64;
                result.slowest_ms = result.slowest_ms.max(elapsed_ms);
                log::warn!(
                    "{} callback #{} slow: {}ms >= {}ms threshold",
                    owner,
                    index,
                    elapsed_ms,
                    slow_threshold.as_millis() as u64,
                );
            }
        }
        result
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

/// Opt-in observer-side escape hatch for slow consumers.
///
/// The leaf registries always dispatch synchronously inline, so a slow
/// observer would block the emitter path. Wrap such an observer in an
/// `AsyncForwarder`: the dispatch-path callback only does a bounded
/// `try_send` (never blocks), while a dedicated worker thread runs the
/// real callback. When the queue is full the event is dropped and
/// `dropped_count` is incremented, so backpressure is explicit instead of
/// silently stalling commits, DDL, or checkpoints.
///
/// Dropping the forwarder closes the channel and ends the worker thread.
/// Use `adapter` to get an `Arc` callback suitable for `add`/`subscribe`.
pub struct AsyncForwarder<E> {
    sender: std::sync::mpsc::SyncSender<E>,
    dropped: std::sync::atomic::AtomicUsize,
}

impl<E> AsyncForwarder<E>
where
    E: Clone + Send + 'static,
{
    /// Spawn a worker running `callback` for each forwarded event.
    ///
    /// Returns the forwarder plus the worker's join handle. Drop the
    /// forwarder (closing the channel) before joining. A panicking
    /// `callback` is isolated per event: the error is logged and the
    /// worker keeps consuming.
    pub fn spawn(
        capacity: usize,
        callback: impl Fn(&E) + Send + 'static,
    ) -> (Arc<Self>, std::thread::JoinHandle<()>) {
        let (sender, receiver) = std::sync::mpsc::sync_channel(capacity);
        let forwarder = Arc::new(Self {
            sender,
            dropped: std::sync::atomic::AtomicUsize::new(0),
        });
        let handle = std::thread::spawn(move || {
            for event in receiver {
                if invoke_callback(&callback, &event).is_err() {
                    log::error!("async forwarder callback panicked; continuing");
                }
            }
        });
        (forwarder, handle)
    }

    /// Hand an event to the worker without blocking. Drops (and counts)
    /// the event when the queue is full or the worker is gone.
    pub fn forward(&self, event: &E) {
        if self.sender.try_send(event.clone()).is_err() {
            self.dropped
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Number of events dropped due to a full queue or a gone worker.
    pub fn dropped_count(&self) -> usize {
        self.dropped.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Dispatch-path adapter: `registry.add(forwarder.adapter())`.
    pub fn adapter(self: &Arc<Self>) -> Arc<dyn Fn(&E) + Send + Sync> {
        let forwarder = Arc::clone(self);
        Arc::new(move |event: &E| forwarder.forward(event))
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

    #[test]
    fn timed_dispatch_flags_slow_observer() {
        use std::time::Duration;
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        registry.add(Arc::new(|_| {}));
        registry.add(Arc::new(|_: &u32| {
            std::thread::sleep(Duration::from_millis(15));
        }));
        let outcome = registry.dispatch_timed("test", &1, Duration::from_millis(5));
        assert_eq!(outcome.outcome.delivered, 2);
        assert_eq!(outcome.outcome.panics, 0);
        assert_eq!(outcome.slow, 1);
        assert!(outcome.slowest_ms >= 5);

        // Empty registry: zero outcome, no timing cost.
        let empty: EventSubscriptions<u32> = EventSubscriptions::new();
        assert_eq!(
            empty.dispatch_timed("test", &1, Duration::from_millis(1)),
            TimedDispatchOutcome::default()
        );
    }

    #[test]
    fn async_forwarder_delivers_without_blocking_emitter() {
        use std::time::Duration;
        let registry: EventSubscriptions<u32> = EventSubscriptions::new();
        let received = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&received);
        let (forwarder, handle) = AsyncForwarder::spawn(16, move |_: &u32| {
            probe.fetch_add(1, Ordering::SeqCst);
        });
        registry.add(forwarder.adapter());
        for event in 0..5u32 {
            registry.dispatch("test", &event);
        }
        assert_eq!(forwarder.dropped_count(), 0);
        drop(registry);
        drop(forwarder);
        handle.join().expect("worker joins");
        assert_eq!(received.load(Ordering::SeqCst), 5);
    }

    #[test]
    fn async_forwarder_counts_drops_on_full_queue() {
        // Rendezvous channel with a slow worker: some forwards land while
        // the worker waits, the rest are dropped and counted. Dropped plus
        // received must account for every sent event.
        let received = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&received);
        let (forwarder, handle) = AsyncForwarder::spawn(0, move |_: &u32| {
            probe.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(std::time::Duration::from_millis(5));
        });
        for event in 0..10u32 {
            forwarder.forward(&event);
        }
        let dropped = forwarder.dropped_count();
        drop(forwarder);
        handle.join().expect("worker joins");
        assert_eq!(received.load(Ordering::SeqCst) + dropped, 10);
    }
}
