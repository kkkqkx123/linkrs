use std::sync::Arc;

use graphdb_core::event_dispatch::EventSubscriptions;

use crate::plan::{MigrationPlan, MigrationReport};

/// Migration lifecycle notification.
///
/// The event value is cloned per listener hop; listeners must return
/// quickly and must not block the migration path.
#[derive(Debug, Clone)]
pub enum MigrationEvent {
    Started { plan: MigrationPlan },
    StepStarted { step_idx: usize },
    StepCompleted { step_idx: usize, rows: u64 },
    Completed { report: MigrationReport },
    Failed { error: String },
    RolledBack { report: MigrationReport },
}

pub trait MigrationEventListener: Send + Sync {
    fn on_event(&self, event: MigrationEvent);
}

#[derive(Debug, Clone, Copy)]
pub struct NoopEventListener;

impl MigrationEventListener for NoopEventListener {
    fn on_event(&self, _event: MigrationEvent) {}
}

/// Deliver a migration event to a listener with panic isolation.
///
/// Listeners run synchronously on the migration path and must return
/// quickly. A panicking listener is logged and skipped so the migration
/// itself is never broken by an observer. Returns true if the listener
/// completed without panicking.
pub fn notify_migration_listener(
    listener: &dyn MigrationEventListener,
    event: MigrationEvent,
) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener.on_event(event))) {
        Ok(()) => true,
        Err(payload) => {
            log::error!(
                "migration listener panicked: {}; continuing migration",
                migration_panic_message(&payload)
            );
            false
        }
    }
}

fn migration_panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        message.to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Unified emission point for migration lifecycle events.
///
/// Fans out to an optional shared registry (borrowed `&event`, matching the
/// `EventSubscriptions` style used by every other domain) and to an optional
/// legacy trait listener (owned event). Either side may be absent; notifying
/// with neither attached is a no-op. Registry dispatch reuses the common
/// panic isolation; the trait hop keeps `notify_migration_listener`.
pub struct MigrationDispatcher<'a> {
    listener: Option<&'a dyn MigrationEventListener>,
    registry: Option<&'a Arc<EventSubscriptions<MigrationEvent>>>,
}

impl<'a> MigrationDispatcher<'a> {
    /// Dispatch to a legacy trait listener only.
    pub fn new(listener: Option<&'a dyn MigrationEventListener>) -> Self {
        Self {
            listener,
            registry: None,
        }
    }

    /// Dispatch to both a trait listener and a shared registry.
    pub fn with_registry(
        listener: Option<&'a dyn MigrationEventListener>,
        registry: Option<&'a Arc<EventSubscriptions<MigrationEvent>>>,
    ) -> Self {
        Self { listener, registry }
    }

    /// Emit one event to all attached sides.
    pub fn notify(&self, event: MigrationEvent) {
        if let Some(registry) = self.registry {
            registry.dispatch("migration", &event);
        }
        if let Some(listener) = self.listener {
            notify_migration_listener(listener, event);
        }
    }
}

/// Adapt a trait listener into a registry callback (`registry.add(..)`).
///
/// The event is cloned per delivery; a panicking listener is isolated by the
/// registry dispatch and does not affect other observers.
pub fn bridge_listener(
    listener: Arc<dyn MigrationEventListener>,
) -> Arc<dyn Fn(&MigrationEvent) + Send + Sync> {
    Arc::new(move |event: &MigrationEvent| {
        notify_migration_listener(listener.as_ref(), event.clone());
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    struct PanickingListener;

    impl MigrationEventListener for PanickingListener {
        fn on_event(&self, _event: MigrationEvent) {
            panic!("listener boom");
        }
    }

    struct CountingListener {
        hits: Arc<AtomicUsize>,
    }

    impl MigrationEventListener for CountingListener {
        fn on_event(&self, _event: MigrationEvent) {
            self.hits.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn panicking_listener_does_not_propagate() {
        let listener = PanickingListener;
        let survived = notify_migration_listener(
            &listener,
            MigrationEvent::Failed {
                error: "x".to_string(),
            },
        );
        assert!(!survived);
    }

    #[test]
    fn healthy_listener_reports_survival() {
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = CountingListener {
            hits: Arc::clone(&hits),
        };
        let survived = notify_migration_listener(
            &listener,
            MigrationEvent::StepCompleted {
                step_idx: 0,
                rows: 1,
            },
        );
        assert!(survived);
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn dispatcher_fans_out_to_registry_and_trait_listener() {
        use graphdb_core::event_dispatch::EventSubscriptions;

        let registry = Arc::new(EventSubscriptions::<MigrationEvent>::new());
        let registry_hits = Arc::new(AtomicUsize::new(0));
        let registry_probe = Arc::clone(&registry_hits);
        registry.add(Arc::new(move |_| {
            registry_probe.fetch_add(1, Ordering::SeqCst);
        }));
        let trait_hits = Arc::new(AtomicUsize::new(0));
        let listener = CountingListener {
            hits: Arc::clone(&trait_hits),
        };
        let dispatcher = MigrationDispatcher::with_registry(Some(&listener), Some(&registry));
        dispatcher.notify(MigrationEvent::StepStarted { step_idx: 3 });
        assert_eq!(registry_hits.load(Ordering::SeqCst), 1);
        assert_eq!(trait_hits.load(Ordering::SeqCst), 1);

        // Neither side attached: no-op, no panic.
        MigrationDispatcher::new(None).notify(MigrationEvent::StepStarted { step_idx: 0 });
    }

    #[test]
    fn bridge_listener_adapts_trait_into_registry_callback() {
        use graphdb_core::event_dispatch::EventSubscriptions;

        let registry = Arc::new(EventSubscriptions::<MigrationEvent>::new());
        let hits = Arc::new(AtomicUsize::new(0));
        let listener = Arc::new(CountingListener {
            hits: Arc::clone(&hits),
        });
        registry.add(bridge_listener(listener));
        registry.dispatch(
            "test",
            &MigrationEvent::Failed {
                error: "x".to_string(),
            },
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
