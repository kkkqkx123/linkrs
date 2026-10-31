//! Migration progress fan-out shared by the SSE endpoint and gRPC stream.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use linkrs_migration::MigrationEvent;

type Sender = tokio::sync::broadcast::Sender<MigrationEvent>;
type Receiver = tokio::sync::broadcast::Receiver<MigrationEvent>;

static HUB: std::sync::OnceLock<Arc<RwLock<HashMap<String, Sender>>>> = std::sync::OnceLock::new();

fn hub() -> Arc<RwLock<HashMap<String, Sender>>> {
    HUB.get_or_init(|| Arc::new(RwLock::new(HashMap::new())))
        .clone()
}

fn hub_key(space: &str, label: &str, is_edge: bool) -> String {
    format!("{}/{}/{}", space, label, is_edge)
}

pub(crate) fn get_or_create_sender(space: &str, label: &str, is_edge: bool) -> Sender {
    let key = hub_key(space, label, is_edge);
    let hub_arc = hub();
    {
        let read = hub_arc
            .read()
            .expect("migration progress hub lock poisoned");
        if let Some(s) = read.get(&key) {
            return s.clone();
        }
    }
    let mut write = hub_arc
        .write()
        .expect("migration progress hub lock poisoned");
    if let Some(s) = write.get(&key) {
        return s.clone();
    }
    let (tx, _rx) = tokio::sync::broadcast::channel(100);
    write.insert(key, tx.clone());
    tx
}

pub(crate) fn subscribe(space: &str, label: &str, is_edge: bool) -> Receiver {
    get_or_create_sender(space, label, is_edge).subscribe()
}

/// Adapt a broadcast sender into an `EventSubscriptions` callback so a
/// running migration fans its lifecycle events out to subscribers.
pub(crate) fn event_bridge(sender: Sender) -> Arc<dyn Fn(&MigrationEvent) + Send + Sync> {
    Arc::new(move |event: &MigrationEvent| {
        let _ = sender.send(event.clone());
    })
}
