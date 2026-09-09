use crate::plan::{MigrationPlan, MigrationReport};

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
}
