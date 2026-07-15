//! Sync Manager
//!
//! Unified synchronization manager using SyncCoordinator.

use crate::core::types::{TransactionContextInfo, TransactionId};
use crate::core::Value;
#[cfg(feature = "fulltext-search")]
use crate::search::SyncConfig;
#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::{ChangeType, CoordinatorError, SyncCoordinator};
use crate::sync::outbox::{OutboxPayload, PersistentOutbox};
#[cfg(not(feature = "fulltext-search"))]
use crate::sync::types::ChangeType;
#[cfg(feature = "qdrant")]
use crate::sync::vector_sync::VectorSyncCoordinator;
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;

#[cfg(feature = "fulltext-search")]
use crate::sync::coordinator::ChangeContext;

#[cfg(feature = "qdrant")]
pub use vector_client::{CollectionConfig, SearchResult};

pub struct SyncManager {
    #[cfg(feature = "fulltext-search")]
    sync_coordinator: Option<Arc<SyncCoordinator>>,
    #[cfg(feature = "qdrant")]
    vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    txn_sequences: DashMap<TransactionId, AtomicU64>,
    running: Arc<std::sync::atomic::AtomicBool>,
    dead_letter_queue: Option<Arc<crate::sync::DeadLetterQueue>>,
    outbox: Option<Arc<PersistentOutbox>>,
    #[allow(clippy::type_complexity)]
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Clone for SyncManager {
    fn clone(&self) -> Self {
        let txn_sequences = DashMap::new();
        for entry in self.txn_sequences.iter() {
            txn_sequences.insert(
                *entry.key(),
                AtomicU64::new(entry.value().load(Ordering::Relaxed)),
            );
        }

        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: self.sync_coordinator.clone(),
            #[cfg(feature = "qdrant")]
            vector_coordinator: self.vector_coordinator.clone(),
            txn_sequences,
            running: self.running.clone(),
            dead_letter_queue: self.dead_letter_queue.clone(),
            outbox: self.outbox.clone(),
            handle: Mutex::new(None),
        }
    }
}

impl std::fmt::Debug for SyncManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("SyncManager");
        #[cfg(feature = "fulltext-search")]
        d.field("sync_coordinator", &self.sync_coordinator);
        #[cfg(feature = "qdrant")]
        d.field("vector_coordinator", &self.vector_coordinator);
        d.field("running", &self.running);
        d.finish_non_exhaustive()
    }
}

#[cfg_attr(
    not(any(feature = "fulltext-search", feature = "qdrant")),
    allow(unused_variables)
)]
impl SyncManager {
    fn next_sync_sequence(&self, txn_id: TransactionId) -> u64 {
        self.txn_sequences
            .entry(txn_id)
            .or_insert_with(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::SeqCst)
            + 1
    }

    pub fn sync_sequence(&self, txn_id: TransactionId) -> u64 {
        self.txn_sequences
            .get(&txn_id)
            .map(|entry| entry.load(Ordering::SeqCst))
            .unwrap_or(0)
    }

    pub fn attach_transaction_context(&self, txn_id: TransactionId) -> TransactionContextInfo {
        TransactionContextInfo::new(txn_id, 0, false, self.sync_sequence(txn_id))
    }

    pub fn rollback_transaction_to_sequence_sync(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord
                .truncate_transaction(txn_id, sequence)
                .map_err(SyncError::from)?;
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .truncate_transaction(txn_id, sequence)
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }

        self.txn_sequences
            .entry(txn_id)
            .and_modify(|current| current.store(sequence, Ordering::SeqCst))
            .or_insert_with(|| AtomicU64::new(sequence));

        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub fn new(sync_coordinator: Arc<SyncCoordinator>) -> Self {
        Self {
            sync_coordinator: Some(sync_coordinator),
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            txn_sequences: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dead_letter_queue: None,
            outbox: None,
            handle: Mutex::new(None),
        }
    }

    pub fn new_without_fulltext() -> Self {
        Self {
            #[cfg(feature = "fulltext-search")]
            sync_coordinator: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
            txn_sequences: DashMap::new(),
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            dead_letter_queue: None,
            outbox: None,
            handle: Mutex::new(None),
        }
    }

    #[cfg(feature = "qdrant")]
    pub fn with_vector_coordinator(
        mut self,
        vector_coordinator: Arc<VectorSyncCoordinator>,
    ) -> Self {
        self.vector_coordinator = Some(vector_coordinator);
        self
    }

    #[cfg(feature = "fulltext-search")]
    pub fn with_sync_config(
        sync_coordinator: Arc<SyncCoordinator>,
        _sync_config: SyncConfig,
    ) -> Self {
        Self::new(sync_coordinator)
    }

    pub fn with_dead_letter_queue(
        mut self,
        dead_letter_queue: Arc<crate::sync::DeadLetterQueue>,
    ) -> Self {
        self.dead_letter_queue = Some(dead_letter_queue);
        self
    }

    pub fn with_outbox(mut self, path: impl AsRef<std::path::Path>) -> Result<Self, SyncError> {
        self.configure_outbox(path)?;
        Ok(self)
    }

    pub fn configure_outbox(&mut self, path: impl AsRef<std::path::Path>) -> Result<(), SyncError> {
        self.outbox = Some(Arc::new(
            PersistentOutbox::open(path).map_err(SyncError::PersistenceError)?,
        ));
        Ok(())
    }

    pub fn outbox_stats(&self) -> crate::sync::OutboxStats {
        self.outbox
            .as_ref()
            .map(|outbox| outbox.stats())
            .unwrap_or_default()
    }

    fn finish_direct_outbox_attempt(
        &self,
        event_id: Option<&str>,
        delivered: bool,
    ) -> Result<(), SyncError> {
        let (Some(outbox), Some(event_id)) = (&self.outbox, event_id) else {
            return Ok(());
        };
        if delivered {
            outbox
                .acknowledge(event_id)
                .map_err(SyncError::PersistenceError)
        } else {
            outbox
                .record_retry(event_id)
                .map_err(SyncError::PersistenceError)
        }
    }

    pub fn retry_outbox_sync(&self) -> Result<usize, SyncError> {
        let Some(outbox) = &self.outbox else {
            return Ok(0);
        };
        let mut delivered = 0;
        for event in outbox.committed_events() {
            let result = match &event.payload {
                OutboxPayload::Vertex {
                    space_id,
                    tag_name,
                    vertex_id,
                    properties,
                    change_type,
                } => self.execute_sync(|| {
                    self.on_vertex_change_direct(
                        *space_id,
                        tag_name,
                        vertex_id,
                        properties,
                        *change_type,
                    )
                }),
                OutboxPayload::EdgeInsert { space_id, edge } => {
                    self.execute_sync(|| self.on_edge_insert_direct(*space_id, edge))
                }
                OutboxPayload::EdgeDelete {
                    space_id,
                    src,
                    dst,
                    edge_type,
                } => {
                    self.execute_sync(|| self.on_edge_delete_direct(*space_id, src, dst, edge_type))
                }
            };
            if result.is_ok() {
                outbox
                    .acknowledge(&event.id)
                    .map_err(SyncError::PersistenceError)?;
                delivered += 1;
            } else {
                outbox
                    .record_retry(&event.id)
                    .map_err(SyncError::PersistenceError)?;
            }
        }
        Ok(delivered)
    }

    pub async fn start(&self) -> Result<(), SyncError> {
        if self.running.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }

        self.running
            .store(true, std::sync::atomic::Ordering::SeqCst);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.start_background_tasks().await;
        }

        Ok(())
    }

    pub async fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::SeqCst);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.stop_background_tasks().await;
        }

        if let Some(handle) = self.handle.lock().await.take() {
            let _ = handle.await;
        }
    }

    pub fn on_vertex_change_with_txn(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        tag_name: &str,
        vertex_id: &Value,
        properties: &[(String, Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        let sequence = self.next_sync_sequence(txn_id);
        if let Some(outbox) = &self.outbox {
            outbox
                .enqueue(
                    Some(txn_id),
                    sequence,
                    OutboxPayload::Vertex {
                        space_id,
                        tag_name: tag_name.to_string(),
                        vertex_id: vertex_id.clone(),
                        properties: properties.to_vec(),
                        change_type,
                    },
                )
                .map_err(SyncError::PersistenceError)?;
        }
        for (field_name, value) in properties {
            #[cfg(feature = "fulltext-search")]
            if let Value::String(text) = value {
                let ctx = ChangeContext::new_fulltext(
                    space_id,
                    tag_name,
                    field_name,
                    change_type,
                    format!("{}", vertex_id),
                    text.clone(),
                );
                if let Some(ref coord) = self.sync_coordinator {
                    coord
                        .buffer_operation_with_sequence(txn_id, sequence, ctx)
                        .map_err(SyncError::from)?;
                }
            }

            #[cfg(feature = "qdrant")]
            if let Some(vector) = value.as_vector() {
                if let Some(ref vector_coord) = self.vector_coordinator {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        tag_name,
                        field_name,
                        crate::sync::vector_sync::VectorChangeType::from(change_type),
                        crate::sync::vector_sync::VectorPointData {
                            id: format!("{}", vertex_id),
                            vector: vector.clone(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .buffer_vector_change_with_sequence(txn_id, sequence, ctx)
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
                }
            }
        }

        Ok(())
    }

    pub async fn on_vertex_change_direct(
        &self,
        space_id: u64,
        tag_name: &str,
        vertex_id: &crate::core::Value,
        properties: &[(String, crate::core::Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            for (field_name, value) in properties {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        tag_name,
                        field_name,
                        change_type,
                        format!("{}", vertex_id),
                        text.clone(),
                    );
                    coord.on_change(ctx).await.map_err(SyncError::from)?;
                }
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            for (field_name, value) in properties {
                if let Some(vector) = value.as_vector() {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        tag_name,
                        field_name,
                        crate::sync::vector_sync::VectorChangeType::from(change_type),
                        crate::sync::vector_sync::VectorPointData {
                            id: format!("{}", vertex_id),
                            vector: vector.clone(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .on_vector_change(ctx)
                        .await
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
                }
            }
        }

        Ok(())
    }

    pub fn on_edge_insert(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        let sequence = self.next_sync_sequence(txn_id);
        if let Some(outbox) = &self.outbox {
            outbox
                .enqueue(
                    Some(txn_id),
                    sequence,
                    OutboxPayload::EdgeInsert {
                        space_id,
                        edge: edge.clone(),
                    },
                )
                .map_err(SyncError::PersistenceError)?;
        }
        let props: Vec<(String, Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        for (field_name, value) in &props {
            #[cfg(feature = "fulltext-search")]
            if let Value::String(text) = value {
                let ctx = ChangeContext::new_fulltext(
                    space_id,
                    &edge.edge_type,
                    field_name,
                    ChangeType::Insert,
                    format!("{}->{}", edge.src, edge.dst),
                    text.clone(),
                );
                if let Some(ref coord) = self.sync_coordinator {
                    coord
                        .buffer_operation_with_sequence(txn_id, sequence, ctx)
                        .map_err(SyncError::from)?;
                }
            }

            #[cfg(feature = "qdrant")]
            if let Some(vector) = value.as_vector() {
                if let Some(ref vector_coord) = self.vector_coordinator {
                    if vector_coord.index_exists(space_id, &edge.edge_type, field_name) {
                        let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                            space_id,
                            &edge.edge_type,
                            field_name,
                            crate::sync::vector_sync::VectorChangeType::from(ChangeType::Insert),
                            crate::sync::vector_sync::VectorPointData {
                                id: format!("{}->{}", edge.src, edge.dst),
                                vector: vector.clone(),
                                payload: std::collections::HashMap::new(),
                            },
                        );
                        vector_coord
                            .buffer_vector_change_with_sequence(txn_id, sequence, ctx)
                            .map_err(|e| SyncError::VectorError(e.to_string()))?;
                    }
                }
            }
        }

        Ok(())
    }

    pub fn on_edge_delete(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        src: &Value,
        dst: &Value,
        edge_type: &str,
    ) -> Result<(), SyncError> {
        let sequence = self.next_sync_sequence(txn_id);
        if let Some(outbox) = &self.outbox {
            outbox
                .enqueue(
                    Some(txn_id),
                    sequence,
                    OutboxPayload::EdgeDelete {
                        space_id,
                        src: src.clone(),
                        dst: dst.clone(),
                        edge_type: edge_type.to_string(),
                    },
                )
                .map_err(SyncError::PersistenceError)?;
        }
        let edge_id = format!("{}->{}", src, dst);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            let indexes = coord
                .fulltext_manager()
                .get_space_indexes(space_id)
                .into_iter()
                .filter(|m| m.tag_name == edge_type);

            let mut found = false;
            for metadata in indexes {
                found = true;
                let ctx = ChangeContext::new_fulltext(
                    space_id,
                    edge_type,
                    &metadata.field_name,
                    ChangeType::Delete,
                    edge_id.clone(),
                    String::new(),
                );
                coord
                    .buffer_operation_with_sequence(txn_id, sequence, ctx)
                    .map_err(SyncError::from)?;
            }

            if !found {
                tracing::debug!(
                    "Edge delete for {}.{}: no fulltext indexes found, skipping",
                    space_id,
                    edge_type
                );
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            let vector_indexes = vector_coord.list_indexes();
            for idx in vector_indexes {
                if idx.space_id == space_id && idx.tag_name == edge_type {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        edge_type,
                        &idx.field_name,
                        crate::sync::vector_sync::VectorChangeType::Delete,
                        crate::sync::vector_sync::VectorPointData {
                            id: edge_id.clone(),
                            vector: Vec::new(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .buffer_vector_change_with_sequence(txn_id, sequence, ctx)
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
                }
            }
        }

        Ok(())
    }

    pub async fn on_edge_insert_direct(
        &self,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        let props: Vec<(String, crate::core::Value)> = edge
            .props
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            for (field_name, value) in &props {
                if let crate::core::Value::String(text) = value {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        &edge.edge_type,
                        field_name,
                        ChangeType::Insert,
                        format!("{}->{}", edge.src, edge.dst),
                        text.clone(),
                    );
                    coord.on_change(ctx).await.map_err(SyncError::from)?;
                }
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            for (field_name, value) in &props {
                if let Some(vector) = value.as_vector() {
                    if vector_coord.index_exists(space_id, &edge.edge_type, field_name) {
                        let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                            space_id,
                            &edge.edge_type,
                            field_name,
                            crate::sync::vector_sync::VectorChangeType::from(ChangeType::Insert),
                            crate::sync::vector_sync::VectorPointData {
                                id: format!("{}->{}", edge.src, edge.dst),
                                vector: vector.clone(),
                                payload: std::collections::HashMap::new(),
                            },
                        );
                        vector_coord
                            .on_vector_change(ctx)
                            .await
                            .map_err(|e| SyncError::VectorError(e.to_string()))?;
                    }
                }
            }
        }

        Ok(())
    }

    pub async fn on_edge_delete_direct(
        &self,
        space_id: u64,
        src: &crate::core::Value,
        dst: &crate::core::Value,
        edge_type: &str,
    ) -> Result<(), SyncError> {
        let edge_id = format!("{}->{}", src, dst);

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            let indexes = coord
                .fulltext_manager()
                .get_space_indexes(space_id)
                .into_iter()
                .filter(|m| m.tag_name == edge_type);

            for metadata in indexes {
                let ctx = ChangeContext::new_fulltext(
                    space_id,
                    edge_type,
                    &metadata.field_name,
                    ChangeType::Delete,
                    edge_id.clone(),
                    String::new(),
                );
                coord.on_change(ctx).await.map_err(SyncError::from)?;
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            let vector_indexes = vector_coord.list_indexes();
            for idx in vector_indexes {
                if idx.space_id == space_id && idx.tag_name == edge_type {
                    let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                        space_id,
                        edge_type,
                        &idx.field_name,
                        crate::sync::vector_sync::VectorChangeType::Delete,
                        crate::sync::vector_sync::VectorPointData {
                            id: edge_id.clone(),
                            vector: Vec::new(),
                            payload: std::collections::HashMap::new(),
                        },
                    );
                    vector_coord
                        .on_vector_change(ctx)
                        .await
                        .map_err(|e| SyncError::VectorError(e.to_string()))?;
                }
            }
        }

        Ok(())
    }

    pub fn on_vertex_change_direct_sync(
        &self,
        space_id: u64,
        tag_name: &str,
        vertex_id: &crate::core::Value,
        properties: &[(String, crate::core::Value)],
        change_type: ChangeType,
    ) -> Result<(), SyncError> {
        let event_id = self
            .outbox
            .as_ref()
            .map(|outbox| {
                outbox.enqueue(
                    None,
                    0,
                    OutboxPayload::Vertex {
                        space_id,
                        tag_name: tag_name.to_string(),
                        vertex_id: vertex_id.clone(),
                        properties: properties.to_vec(),
                        change_type,
                    },
                )
            })
            .transpose()
            .map_err(SyncError::PersistenceError)?;
        let result = self.execute_sync(|| {
            self.on_vertex_change_direct(space_id, tag_name, vertex_id, properties, change_type)
        });
        self.finish_direct_outbox_attempt(event_id.as_deref(), result.is_ok())?;
        result
    }

    pub fn on_edge_insert_direct_sync(
        &self,
        space_id: u64,
        edge: &crate::core::Edge,
    ) -> Result<(), SyncError> {
        let event_id = self
            .outbox
            .as_ref()
            .map(|outbox| {
                outbox.enqueue(
                    None,
                    0,
                    OutboxPayload::EdgeInsert {
                        space_id,
                        edge: edge.clone(),
                    },
                )
            })
            .transpose()
            .map_err(SyncError::PersistenceError)?;
        let result = self.execute_sync(|| self.on_edge_insert_direct(space_id, edge));
        self.finish_direct_outbox_attempt(event_id.as_deref(), result.is_ok())?;
        result
    }

    pub fn on_edge_delete_direct_sync(
        &self,
        space_id: u64,
        src: &crate::core::Value,
        dst: &crate::core::Value,
        edge_type: &str,
    ) -> Result<(), SyncError> {
        let event_id = self
            .outbox
            .as_ref()
            .map(|outbox| {
                outbox.enqueue(
                    None,
                    0,
                    OutboxPayload::EdgeDelete {
                        space_id,
                        src: src.clone(),
                        dst: dst.clone(),
                        edge_type: edge_type.to_string(),
                    },
                )
            })
            .transpose()
            .map_err(SyncError::PersistenceError)?;
        let result =
            self.execute_sync(|| self.on_edge_delete_direct(space_id, src, dst, edge_type));
        self.finish_direct_outbox_attempt(event_id.as_deref(), result.is_ok())?;
        result
    }

    pub fn on_edge_update(
        &self,
        txn_id: TransactionId,
        space_id: u64,
        edge: EdgeRef<'_>,
        props: EdgeProps<'_>,
    ) -> Result<(), SyncError> {
        let sequence = self.next_sync_sequence(txn_id);
        let edge_id = edge.id();

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            let indexes = coord
                .fulltext_manager()
                .get_space_indexes(space_id)
                .into_iter()
                .filter(|m| m.tag_name == edge.edge_type)
                .collect::<Vec<_>>();

            for metadata in &indexes {
                let field_name = &metadata.field_name;

                if let Some((_, Value::String(text))) =
                    props.old.iter().find(|(k, _)| k == field_name)
                {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        edge.edge_type,
                        field_name,
                        ChangeType::Delete,
                        edge_id.clone(),
                        text.clone(),
                    );
                    coord
                        .buffer_operation_with_sequence(txn_id, sequence, ctx)
                        .map_err(SyncError::from)?;
                }

                if let Some((_, Value::String(text))) =
                    props.new.iter().find(|(k, _)| k == field_name)
                {
                    let ctx = ChangeContext::new_fulltext(
                        space_id,
                        edge.edge_type,
                        field_name,
                        ChangeType::Insert,
                        edge_id.clone(),
                        text.clone(),
                    );
                    coord
                        .buffer_operation_with_sequence(txn_id, sequence, ctx)
                        .map_err(SyncError::from)?;
                }
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            for idx in vector_coord.list_indexes() {
                if idx.space_id == space_id && idx.tag_name == edge.edge_type {
                    if let Some((_, old_value)) =
                        props.old.iter().find(|(k, _)| k == &idx.field_name)
                    {
                        if old_value.as_vector().is_some() {
                            let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                                space_id,
                                edge.edge_type,
                                &idx.field_name,
                                crate::sync::vector_sync::VectorChangeType::Delete,
                                crate::sync::vector_sync::VectorPointData {
                                    id: edge_id.clone(),
                                    vector: Vec::new(),
                                    payload: std::collections::HashMap::new(),
                                },
                            );
                            vector_coord
                                .buffer_vector_change_with_sequence(txn_id, sequence, ctx)
                                .map_err(|e| SyncError::VectorError(e.to_string()))?;
                        }
                    }
                    if let Some((_, new_value)) =
                        props.new.iter().find(|(k, _)| k == &idx.field_name)
                    {
                        if let Some(vector) = new_value.as_vector() {
                            let ctx = crate::sync::vector_sync::VectorChangeContext::new(
                                space_id,
                                edge.edge_type,
                                &idx.field_name,
                                crate::sync::vector_sync::VectorChangeType::Insert,
                                crate::sync::vector_sync::VectorPointData {
                                    id: edge_id.clone(),
                                    vector: vector.clone(),
                                    payload: std::collections::HashMap::new(),
                                },
                            );
                            vector_coord
                                .buffer_vector_change_with_sequence(txn_id, sequence, ctx)
                                .map_err(|e| SyncError::VectorError(e.to_string()))?;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg(feature = "qdrant")]
    pub fn on_vector_change_with_context_buffered(
        &self,
        txn_id: crate::core::types::TransactionId,
        ctx: crate::sync::vector_sync::VectorChangeContext,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .buffer_vector_change(txn_id, ctx)
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }
        Ok(())
    }

    #[cfg(feature = "qdrant")]
    pub async fn on_vector_change_with_context(
        &self,
        ctx: crate::sync::vector_sync::VectorChangeContext,
    ) -> Result<(), SyncError> {
        if self.vector_coordinator.is_none() {
            return Ok(());
        }

        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .on_vector_change(ctx)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }

        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub async fn commit_all(&self) -> Result<(), SyncError> {
        if let Some(ref coord) = self.sync_coordinator {
            coord.commit_all().await?;
        }
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub async fn prepare_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        if let Some(ref coord) = self.sync_coordinator {
            coord.prepare_transaction(txn_id).await?;
        }
        Ok(())
    }

    /// Commit transaction: flush buffered operations to external indexes.
    ///
    /// Uses a commit order that minimizes inconsistency on partial failure:
    /// 1. Validate both coordinators (prepare phase)
    /// 2. Commit vector first (external system, harder to recover from)
    /// 3. Commit fulltext second (local system, easier to rebuild)
    /// 4. If vector fails, fulltext buffer is still intact for rollback.
    /// 5. If fulltext fails after vector succeeds, vector is committed but
    ///    fulltext can be rebuilt from storage.
    pub async fn commit_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        if let Some(outbox) = &self.outbox {
            outbox.commit(txn_id).map_err(SyncError::PersistenceError)?;
        }
        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            coord.prepare_transaction(txn_id).await?;
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .commit_transaction(txn_id)
                .await
                .map_err(|e| {
                    // Vector commit failed — fulltext buffer is still intact.
                    // Rollback fulltext buffer so it can be re-applied on retry.
                    #[cfg(feature = "fulltext-search")]
                    if let Some(ref coord) = self.sync_coordinator {
                        if let Err(rollback_err) = coord.rollback_transaction(txn_id) {
                            tracing::error!(
                                "Fulltext rollback also failed after vector commit failure for txn {:?}: {}",
                                txn_id, rollback_err
                            );
                        }
                    }
                    SyncError::VectorError(e.to_string())
                })?;
        }

        #[cfg(feature = "fulltext-search")]
        if let Some(ref coord) = self.sync_coordinator {
            if let Err(e) = coord.commit_transaction(txn_id).await {
                // Fulltext commit failed after vector succeeded.
                // Vector data is already committed. Fulltext can be rebuilt from storage.
                tracing::error!(
                    "Fulltext commit failed after vector commit succeeded for txn {:?}: {}. \
                     Vector data is committed; fulltext index needs rebuild.",
                    txn_id,
                    e
                );
                return Err(SyncError::from(e));
            }
        }

        self.txn_sequences.remove(&txn_id);
        if let Some(outbox) = &self.outbox {
            outbox
                .acknowledge_transaction(txn_id)
                .map_err(SyncError::PersistenceError)?;
        }

        Ok(())
    }

    pub async fn rollback_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.rollback_transaction_to_sequence_sync(txn_id, 0)?;
        if let Some(outbox) = &self.outbox {
            outbox
                .rollback(txn_id)
                .map_err(SyncError::PersistenceError)?;
        }
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub fn prepare_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.prepare_transaction(txn_id))
    }

    pub fn commit_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.commit_transaction(txn_id))
    }

    pub fn rollback_transaction_sync(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.rollback_transaction(txn_id))
    }

    fn execute_sync<F, Fut, T>(&self, f: F) -> Result<T, SyncError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<T, SyncError>>,
    {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            return tokio::task::block_in_place(|| handle.block_on(f()));
        }

        futures::executor::block_on(f())
    }

    #[cfg(feature = "qdrant")]
    pub async fn commit_vector_transaction(
        &self,
        txn_id: crate::core::types::TransactionId,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .commit_transaction(txn_id)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))?;
        }
        Ok(())
    }

    #[cfg(feature = "fulltext-search")]
    pub fn sync_coordinator(&self) -> &Arc<SyncCoordinator> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext-search feature")
    }

    #[cfg(feature = "qdrant")]
    pub fn vector_coordinator(&self) -> Option<&Arc<VectorSyncCoordinator>> {
        self.vector_coordinator.as_ref()
    }

    #[cfg(feature = "fulltext-search")]
    pub fn fulltext_manager(&self) -> Arc<crate::search::manager::FulltextIndexManager> {
        self.sync_coordinator
            .as_ref()
            .expect("SyncCoordinator not available without fulltext-search feature")
            .fulltext_manager()
            .clone()
    }

    pub fn is_running(&self) -> bool {
        self.running.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn get_dead_letter_entries(&self) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all()
        } else {
            vec![]
        }
    }

    pub fn get_unrecovered_entries(&self) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered()
        } else {
            vec![]
        }
    }

    pub fn get_old_dead_letter_entries(
        &self,
        age: std::time::Duration,
    ) -> Vec<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_old_entries(age)
        } else {
            vec![]
        }
    }

    pub fn remove_dead_letter_entry(&self, index: usize) -> Option<crate::sync::DeadLetterEntry> {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.remove(index)
        } else {
            None
        }
    }

    pub fn get_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_all().len()
        } else {
            0
        }
    }

    pub fn get_unrecovered_dlq_size(&self) -> usize {
        if let Some(ref dlq) = self.dead_letter_queue {
            dlq.get_unrecovered().len()
        } else {
            0
        }
    }

    #[cfg(feature = "qdrant")]
    pub fn vector_index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord.index_exists(space_id, tag_name, field_name)
        } else {
            false
        }
    }

    #[cfg(feature = "qdrant")]
    pub async fn create_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: usize,
        distance: vector_client::DistanceMetric,
    ) -> Result<String, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .create_vector_index(space_id, tag_name, field_name, vector_size, distance)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "qdrant")]
    pub async fn drop_vector_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<(), SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            vector_coord
                .drop_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }

    #[cfg(feature = "qdrant")]
    pub async fn search_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector: &[f32],
        top_k: usize,
    ) -> Result<Vec<SearchResult>, SyncError> {
        if let Some(ref vector_coord) = self.vector_coordinator {
            let options = crate::sync::vector_sync::SearchOptions::new(
                space_id,
                tag_name,
                field_name,
                vector.to_vec(),
                top_k,
            );
            vector_coord
                .search_with_options(options)
                .await
                .map_err(|e| SyncError::VectorError(e.to_string()))
        } else {
            Err(SyncError::Internal(
                "Vector coordinator not available".to_string(),
            ))
        }
    }
}

#[derive(Debug, Clone)]
pub struct EdgeRef<'a> {
    pub src: &'a Value,
    pub dst: &'a Value,
    pub edge_type: &'a str,
}

impl<'a> EdgeRef<'a> {
    pub fn new(src: &'a Value, dst: &'a Value, edge_type: &'a str) -> Self {
        Self {
            src,
            dst,
            edge_type,
        }
    }

    pub fn id(&self) -> String {
        format!("{}->{}", self.src, self.dst)
    }
}

#[derive(Debug, Clone)]
pub struct EdgeProps<'a> {
    pub old: &'a [(String, Value)],
    pub new: &'a [(String, Value)],
}

impl<'a> EdgeProps<'a> {
    pub fn new(old: &'a [(String, Value)], new: &'a [(String, Value)]) -> Self {
        Self { old, new }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[cfg(feature = "fulltext-search")]
    #[error("Coordinator error: {0}")]
    CoordinatorError(#[from] CoordinatorError),

    #[cfg(feature = "fulltext-search")]
    #[error("Sync coordinator error: {0}")]
    SyncCoordinatorError(#[from] crate::sync::coordinator::SyncCoordinatorError),

    #[error("Buffer error: {0}")]
    BufferError(String),

    #[error("Vector error: {0}")]
    VectorError(String),

    #[error("Persistence error: {0}")]
    PersistenceError(String),

    #[error("Internal error: {0}")]
    Internal(String),
}
