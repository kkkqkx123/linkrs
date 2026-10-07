use crate::plan::{MigrationPlan, MigrationReport};

/// Migration lifecycle notification.
///
/// Events are delivered through `EventSubscriptions<MigrationEvent>`.
/// Callbacks run inline on the migration path with the event borrowed;
/// they must return quickly, must not block, and a panicking callback is
/// isolated by the registry so the migration itself is never broken by an
/// observer.
#[derive(Debug, Clone)]
pub enum MigrationEvent {
    Started { plan: MigrationPlan },
    StepStarted { step_idx: usize },
    StepCompleted { step_idx: usize, rows: u64 },
    Completed { report: MigrationReport },
    Failed { error: String },
    RolledBack { report: MigrationReport },
}

#[cfg(test)]
mod tests {
    use super::MigrationEvent;
    use graphdb_core::event_dispatch::EventSubscriptions;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn registry_fans_out_to_all_subscribers() {
        let registry = EventSubscriptions::<MigrationEvent>::new();
        let hits = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let probe = Arc::clone(&hits);
            registry.add(Arc::new(move |_| {
                probe.fetch_add(1, Ordering::SeqCst);
            }));
        }
        registry.dispatch("migration", &MigrationEvent::StepStarted { step_idx: 3 });
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn panicking_subscriber_does_not_block_others() {
        let registry = EventSubscriptions::<MigrationEvent>::new();
        registry.add(Arc::new(|_| panic!("subscriber boom")));
        let hits = Arc::new(AtomicUsize::new(0));
        let probe = Arc::clone(&hits);
        registry.add(Arc::new(move |_| {
            probe.fetch_add(1, Ordering::SeqCst);
        }));
        registry.dispatch(
            "migration",
            &MigrationEvent::Failed {
                error: "x".to_string(),
            },
        );
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
