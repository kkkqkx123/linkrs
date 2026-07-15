use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::core::types::TransactionId;
use crate::core::{Edge, Value};
use crate::sync::types::ChangeType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) enum OutboxPayload {
    Vertex {
        space_id: u64,
        tag_name: String,
        vertex_id: Value,
        properties: Vec<(String, Value)>,
        change_type: ChangeType,
    },
    EdgeInsert {
        space_id: u64,
        edge: Edge,
    },
    EdgeDelete {
        space_id: u64,
        src: Value,
        dst: Value,
        edge_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct OutboxEvent {
    pub id: String,
    pub transaction_id: Option<TransactionId>,
    pub sequence: u64,
    pub committed: bool,
    pub retries: u64,
    pub created_at_ms: u64,
    pub payload: OutboxPayload,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutboxStats {
    pub pending: usize,
    pub retries: u64,
    pub oldest_event_age_ms: u64,
}

#[derive(Debug)]
pub(crate) struct PersistentOutbox {
    path: PathBuf,
    events: Mutex<Vec<OutboxEvent>>,
}

impl PersistentOutbox {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let events = if path.exists() {
            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            events: Mutex::new(events),
        })
    }

    pub fn enqueue(
        &self,
        transaction_id: Option<TransactionId>,
        sequence: u64,
        payload: OutboxPayload,
    ) -> Result<String, String> {
        let id = match transaction_id {
            Some(transaction_id) => format!("{}:{}", transaction_id.0, sequence),
            None => uuid::Uuid::new_v4().to_string(),
        };
        let mut events = self.events.lock();
        if events.iter().any(|event| event.id == id) {
            return Ok(id);
        }
        events.push(OutboxEvent {
            id: id.clone(),
            transaction_id,
            sequence,
            committed: transaction_id.is_none(),
            retries: 0,
            created_at_ms: now_ms(),
            payload,
        });
        self.persist(&events)?;
        Ok(id)
    }

    pub fn commit(&self, transaction_id: TransactionId) -> Result<(), String> {
        let mut events = self.events.lock();
        for event in &mut *events {
            if event.transaction_id == Some(transaction_id) {
                event.committed = true;
            }
        }
        self.persist(&events)
    }

    pub fn rollback(&self, transaction_id: TransactionId) -> Result<(), String> {
        let mut events = self.events.lock();
        events.retain(|event| event.transaction_id != Some(transaction_id));
        self.persist(&events)
    }

    pub fn acknowledge(&self, id: &str) -> Result<(), String> {
        let mut events = self.events.lock();
        events.retain(|event| event.id != id);
        self.persist(&events)
    }

    pub fn acknowledge_transaction(&self, transaction_id: TransactionId) -> Result<(), String> {
        self.rollback(transaction_id)
    }

    pub fn record_retry(&self, id: &str) -> Result<(), String> {
        let mut events = self.events.lock();
        if let Some(event) = events.iter_mut().find(|event| event.id == id) {
            event.retries = event.retries.saturating_add(1);
        }
        self.persist(&events)
    }

    pub fn committed_events(&self) -> Vec<OutboxEvent> {
        let mut events: Vec<_> = self
            .events
            .lock()
            .iter()
            .filter(|event| event.committed)
            .cloned()
            .collect();
        events.sort_by_key(|event| (event.transaction_id, event.sequence));
        events
    }

    pub fn stats(&self) -> OutboxStats {
        let events = self.events.lock();
        let oldest = events
            .iter()
            .map(|event| event.created_at_ms)
            .min()
            .unwrap_or_else(now_ms);
        OutboxStats {
            pending: events.len(),
            retries: events.iter().map(|event| event.retries).sum(),
            oldest_event_age_ms: now_ms().saturating_sub(oldest),
        }
    }

    fn persist(&self, events: &[OutboxEvent]) -> Result<(), String> {
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec(events).map_err(|error| error.to_string())?;
        std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
        std::fs::File::open(&temporary)
            .and_then(|file| file.sync_all())
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &self.path).map_err(|error| error.to_string())?;
        Ok(())
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
