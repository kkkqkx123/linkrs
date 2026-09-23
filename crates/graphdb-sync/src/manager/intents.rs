//! Transactional intent staging: pre-filtering, backpressure, staging,
//! commit materialization and rollback.
use super::*;
use crate::outbox::OutboxPayload;
use crate::types::ChangeType;
use graphdb_core::types::{TransactionContextInfo, TransactionId};
#[cfg(feature = "fulltext")]
use graphdb_core::Value;
pub(crate) fn payload_to_intent(
    txn_id: graphdb_core::types::TransactionId,
    intent_sequence: u32,
    target_name: &str,
    payload: &OutboxPayload,
) -> Result<graphdb_core::wal::OutboxIntent, SyncError> {
    use graphdb_core::types::{IdempotencyKey, IndexGeneration, OrderingKey, TargetId, VertexId};
    use graphdb_core::wal::{EntityRef, IndexMutation, IndexOperation, WAL_SYNC_WIRE_VERSION};

    let (space_id, index_name, entity_ref, operation) = match payload {
        OutboxPayload::Vertex {
            space_id,
            tag_name,
            vertex_id,
            change_type,
            ..
        } => (
            *space_id,
            tag_name.as_str(),
            EntityRef::Vertex(VertexId::try_from(vertex_id).map_err(|error| {
                SyncError::PersistenceError(format!("Invalid vertex ID in outbox event: {}", error))
            })?),
            match change_type {
                ChangeType::Insert | ChangeType::Update => IndexOperation::Upsert,
                ChangeType::Delete => IndexOperation::Delete,
            },
        ),
        OutboxPayload::EdgeInsert { space_id, edge } => (
            *space_id,
            edge.edge_type.as_str(),
            EntityRef::Edge {
                src: edge.src,
                dst: edge.dst,
                edge_type: stable_hash(edge.edge_type.as_bytes()) as u32,
                ranking: edge.ranking,
            },
            IndexOperation::Upsert,
        ),
        OutboxPayload::EdgeDelete {
            space_id,
            src,
            dst,
            edge_type,
            ranking,
            ..
        } => (
            *space_id,
            edge_type.as_str(),
            EntityRef::Edge {
                src: VertexId::try_from(src).map_err(|error| {
                    SyncError::PersistenceError(format!(
                        "Invalid edge source ID in outbox event: {}",
                        error
                    ))
                })?,
                dst: VertexId::try_from(dst).map_err(|error| {
                    SyncError::PersistenceError(format!(
                        "Invalid edge destination ID in outbox event: {}",
                        error
                    ))
                })?,
                edge_type: stable_hash(edge_type.as_bytes()) as u32,
                ranking: *ranking,
            },
            IndexOperation::Delete,
        ),
        OutboxPayload::CreateIndex {
            space_id,
            index_name,
            ..
        } => (
            *space_id,
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_u64(0)),
            IndexOperation::Upsert,
        ),
        OutboxPayload::DropIndex {
            space_id,
            index_name,
            ..
        } => (
            *space_id,
            index_name.as_str(),
            EntityRef::Vertex(VertexId::from_u64(0)),
            IndexOperation::Delete,
        ),
        OutboxPayload::DropSpace { space_id } => (
            *space_id,
            "__space__",
            EntityRef::Vertex(VertexId::from_u64(0)),
            IndexOperation::Delete,
        ),
        OutboxPayload::DropTag { space_id, tag_name } => (
            *space_id,
            tag_name.as_str(),
            EntityRef::Vertex(VertexId::from_u64(0)),
            IndexOperation::Delete,
        ),
    };
    let sequence = u64::from(intent_sequence).saturating_add(1);
    let id = format!("{}:{}:{}", txn_id.0, target_name, sequence);
    let target = TargetId::new(target_name.to_string()).map_err(SyncError::PersistenceError)?;
    // Entity-scoped ordering key so concurrent updates to the same graph
    // entity are serialized by the SQLite `NOT EXISTS (ordering_key)` fence
    // in `claim_next`. The previous per-event `default:{txn}:{seq}` made every
    // ordering_key unique, so the fence was vacuously true. Now all events
    // for the same logical entity share one key: {target}:{space}:{index}:{entity}.
    let ordering_key = {
        let entity_str = match payload {
            OutboxPayload::Vertex { vertex_id, .. } => format!("{}", vertex_id),
            OutboxPayload::EdgeInsert { edge, .. } => {
                format!("{}->{}#{}", edge.src, edge.dst, edge.ranking)
            }
            OutboxPayload::EdgeDelete {
                src, dst, ranking, ..
            } => format!("{}->{}#{}", src, dst, ranking),
            OutboxPayload::CreateIndex { index_name, .. }
            | OutboxPayload::DropIndex { index_name, .. } => {
                // DDL is per-index; serialize all DDL for the same index.
                format!("ddl:{}", index_name)
            }
            OutboxPayload::DropSpace { .. } => "ddl:__space__".to_string(),
            OutboxPayload::DropTag { tag_name, .. } => {
                format!("ddl:tag:{}", tag_name)
            }
        };
        let key = format!("{}:{}:{}:{}", target_name, space_id, index_name, entity_str);
        OrderingKey::new(key).map_err(SyncError::PersistenceError)?
    };
    let idempotency_key = IdempotencyKey::new(id).map_err(SyncError::PersistenceError)?;
    Ok(graphdb_core::wal::OutboxIntent {
        wire_version: WAL_SYNC_WIRE_VERSION,
        transaction_id: txn_id,
        intent_sequence,
        mutation: IndexMutation {
            wire_version: WAL_SYNC_WIRE_VERSION,
            target,
            index_id: stable_hash(
                format!("{}:{}:{}", target_name, space_id, index_name).as_bytes(),
            ),
            index_generation: IndexGeneration::new(1),
            entity_ref,
            operation,
            document_or_vector: postcard::to_allocvec(payload).map_err(|error| {
                SyncError::PersistenceError(format!(
                    "Failed to serialize target mutation: {}",
                    error
                ))
            })?,
            idempotency_key,
            ordering_key,
        },
    })
}
pub(crate) fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Index IDs are persisted in SQLite INTEGER columns, so keep the
    // deterministic hash within the signed 64-bit range.
    hash & (i64::MAX as u64)
}
#[cfg(feature = "vector")]
pub(crate) fn format_vector_point_id(
    vertex_id: &graphdb_core::Value,
    tag: &str,
    field: &str,
) -> String {
    let raw = format!("{}", vertex_id);
    // Escape the delimiter '#' and the escape char '%' inside the vertex id
    // so that decoding remains unambiguous across Local and Qdrant backends.
    // Percent-encoding is minimal and deterministic; '%' is escaped first to
    // avoid double-encoding.
    let encoded = raw.replace('%', "%25").replace('#', "%23");
    format!("{}#{}#{}", encoded, tag, field)
}
impl super::SyncManager {
    #[cfg(any(feature = "fulltext", feature = "vector"))]
    fn delivery_target_names(&self) -> Vec<&'static str> {
        let mut targets = Vec::new();
        #[cfg(feature = "fulltext")]
        if self.sync_coordinator.is_some() {
            targets.push("fulltext");
        }
        #[cfg(feature = "vector")]
        if self.vector_coordinator.is_some() {
            targets.push("vector");
        }
        targets
    }

    /// No sync engines are compiled in, so there are no delivery targets.
    #[cfg(not(any(feature = "fulltext", feature = "vector")))]
    fn delivery_target_names(&self) -> Vec<&'static str> {
        Vec::new()
    }

    fn payload_needs_vector(&self, payload: &OutboxPayload) -> bool {
        #[cfg(feature = "vector")]
        {
            let Some(coord) = self.vector_coordinator.as_ref() else {
                return false;
            };
            match payload {
                OutboxPayload::Vertex {
                    space_id,
                    tag_name,
                    properties,
                    change_type,
                    ..
                } => {
                    if matches!(change_type, ChangeType::Delete) {
                        // For deletes with empty properties, still need to fan out
                        // to all vector fields of this tag if any index exists.
                        if properties.is_empty() {
                            return coord
                                .list_indexes()
                                .iter()
                                .any(|m| m.space_id == *space_id && m.tag_name == *tag_name);
                        }
                    }
                    for (field, value) in properties {
                        if value.as_vector().is_some()
                            && coord.index_exists(*space_id, tag_name, field)
                        {
                            return true;
                        }
                    }
                    // Delete case with no matching field still handled above; for
                    // Insert/Update with vector-like values but missing index, the
                    // mutation will be filtered later in apply_vector_mutation, but
                    // we pre-filter here to avoid write amplification. If no
                    // indexed field matched, no vector intent is needed.
                    false
                }
                OutboxPayload::CreateIndex { fields, .. } => {
                    fields.iter().any(|(_, v)| v.as_vector().is_some())
                }
                OutboxPayload::DropIndex {
                    space_id,
                    schema_name,
                    fields,
                    ..
                } => {
                    // Drop needs vector only if any dropped field is a vector index
                    fields
                        .iter()
                        .any(|field| coord.index_exists(*space_id, schema_name, field))
                }
                OutboxPayload::DropSpace { space_id } => {
                    coord.list_indexes().iter().any(|m| m.space_id == *space_id)
                }
                OutboxPayload::DropTag { space_id, tag_name } => coord
                    .list_indexes()
                    .iter()
                    .any(|m| m.space_id == *space_id && m.tag_name == *tag_name),
                OutboxPayload::EdgeInsert { .. } | OutboxPayload::EdgeDelete { .. } => false,
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            let _ = payload;
            false
        }
    }

    fn payload_needs_fulltext(&self, payload: &OutboxPayload) -> bool {
        #[cfg(feature = "fulltext")]
        {
            let Some(coord) = self.sync_coordinator.as_ref() else {
                return false;
            };
            let manager = coord.fulltext_manager();
            match payload {
                OutboxPayload::Vertex {
                    space_id,
                    tag_name,
                    properties,
                    change_type,
                    ..
                } => {
                    if matches!(change_type, ChangeType::Delete) && properties.is_empty() {
                        return manager
                            .get_space_indexes(*space_id)
                            .into_iter()
                            .any(|meta| meta.tag_name == *tag_name);
                    }
                    for (field, value) in properties {
                        let is_text = matches!(value, Value::String(_) | Value::FixedString(_));
                        if is_text && manager.has_index(*space_id, tag_name, field) {
                            return true;
                        }
                    }
                    false
                }
                OutboxPayload::CreateIndex { fields, .. } => fields
                    .iter()
                    .any(|(_, v)| matches!(v, Value::String(_) | Value::FixedString(_))),
                OutboxPayload::DropIndex {
                    space_id,
                    schema_name,
                    fields,
                    ..
                } => fields.iter().any(|field| {
                    manager
                        .get_space_indexes(*space_id)
                        .iter()
                        .any(|meta| meta.tag_name == *schema_name && meta.field_name == *field)
                }),
                OutboxPayload::DropSpace { space_id } => {
                    !manager.get_space_indexes(*space_id).is_empty()
                }
                OutboxPayload::DropTag { space_id, tag_name } => manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .any(|meta| meta.tag_name == *tag_name),
                OutboxPayload::EdgeInsert { space_id, edge } => {
                    edge.props.iter().any(|(field, value)| {
                        let is_text = matches!(value, Value::String(_) | Value::FixedString(_));
                        is_text && manager.has_index(*space_id, &edge.edge_type, field)
                    })
                }
                OutboxPayload::EdgeDelete {
                    space_id,
                    edge_type,
                    ..
                } => manager
                    .get_space_indexes(*space_id)
                    .into_iter()
                    .any(|meta| meta.tag_name == *edge_type),
            }
        }
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = payload;
            false
        }
    }

    pub(super) fn stage_intent(
        &self,
        txn_id: TransactionId,
        payload: OutboxPayload,
    ) -> Result<(), SyncError> {
        // Pre-filter by durable outbox requirement: if at least one target
        // actually needs this payload and the outbox is not configured, fail fast.
        let needs_vector = self.payload_needs_vector(&payload);
        let needs_fulltext = self.payload_needs_fulltext(&payload);
        let has_needed_target = needs_vector || needs_fulltext;
        // If neither target needs the payload, skip entirely to avoid write
        // amplification for pure-graph mutations.
        if !has_needed_target {
            // Still validate outbox presence if any target *could* have been
            // needed but was filtered due to missing index: no intent needed.
            return Ok(());
        }
        if has_needed_target && self.sqlite_outbox.is_none() {
            return Err(SyncError::PersistenceError(
                "Synchronized writes require a configured durable outbox".to_string(),
            ));
        }
        // Backpressure: bound the in-memory staging map so a hot writer cannot
        // grow `pending_intents` without bound and hide `p99` delivery latency.
        let needed_targets = self
            .delivery_target_names()
            .into_iter()
            .filter(|name| match *name {
                "vector" => needs_vector,
                "fulltext" => needs_fulltext,
                _ => true,
            })
            .count();
        if needed_targets > 0 {
            // Per-transaction limit
            let current_len = self
                .pending_intents
                .get(&txn_id)
                .map(|v| v.len())
                .unwrap_or(0);
            if current_len + needed_targets > self.backpressure.max_pending_per_txn {
                return Err(SyncError::OutboxBackpressure(format!(
                    "txn {} pending {} + {} exceeds max_pending_per_txn {}",
                    txn_id.0, current_len, needed_targets, self.backpressure.max_pending_per_txn
                )));
            }
            // Global in-memory limit (approximate; durable backlog is counted
            // separately via `outbox_pending` metric but not used for fencing
            // to keep the hot path lock-free).
            let total_pending: usize = self.pending_intents.iter().map(|e| e.value().len()).sum();
            if total_pending + needed_targets > self.backpressure.max_pending_total {
                return Err(SyncError::OutboxBackpressure(format!(
                    "global pending {} + {} exceeds max_outbox_pending {}",
                    total_pending, needed_targets, self.backpressure.max_pending_total
                )));
            }
            // Optional: also fence on durable `pending` if outbox is configured
            // and already large. This is best-effort via the cached stats; a
            // stale read may still allow one extra batch through.
            if let Some(outbox) = self.sqlite_outbox.as_ref() {
                if let Some(stats) = self
                    .stats_manager
                    .as_ref()
                    .and_then(|s| s.get_value(graphdb_metrics::MetricType::OutboxPending))
                {
                    let durable_pending = stats as usize;
                    if durable_pending + total_pending + needed_targets
                        > self.backpressure.max_pending_total
                    {
                        return Err(SyncError::OutboxBackpressure(format!(
                            "durable pending {} + staged {} + {} exceeds limit {}",
                            durable_pending,
                            total_pending,
                            needed_targets,
                            self.backpressure.max_pending_total
                        )));
                    }
                    let _ = outbox;
                }
            }
        }
        let mut intents = self.pending_intents.entry(txn_id).or_default();
        for target_name in self.delivery_target_names() {
            let should_deliver = match target_name {
                "vector" => needs_vector,
                "fulltext" => needs_fulltext,
                _ => true,
            };
            if !should_deliver {
                continue;
            }
            let sequence = u32::try_from(intents.len()).map_err(|_| {
                SyncError::PersistenceError(
                    "Transaction intent count exceeds u32 range".to_string(),
                )
            })?;
            intents.push(payload_to_intent(txn_id, sequence, target_name, &payload)?);
        }
        // Update the staged backlog gauge for observability.
        if let Some(stats) = self.stats_manager.as_ref() {
            let total: usize = self.pending_intents.iter().map(|e| e.value().len()).sum();
            stats.set_value(graphdb_metrics::MetricType::OutboxPending, total as u64);
        }
        Ok(())
    }

    pub fn clear_transaction_intents(&self, txn_id: TransactionId) {
        self.pending_intents.remove(&txn_id);
    }

    pub fn attach_transaction_context(&self, txn_id: TransactionId) -> TransactionContextInfo {
        TransactionContextInfo::new(txn_id, 0, false, 0)
    }

    pub fn rollback_transaction_to_sequence_sync(
        &self,
        txn_id: TransactionId,
        sequence: u64,
    ) -> Result<(), SyncError> {
        if let Some(mut intents) = self.pending_intents.get_mut(&txn_id) {
            intents.retain(|intent| u64::from(intent.intent_sequence) <= sequence);
        }

        Ok(())
    }

    pub async fn rollback_transaction(
        &self,
        txn_id: graphdb_core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.rollback_transaction_to_sequence_sync(txn_id, 0)?;
        self.pending_intents.remove(&txn_id);
        Ok(())
    }

    pub fn rollback_transaction_sync(
        &self,
        txn_id: graphdb_core::types::TransactionId,
    ) -> Result<(), SyncError> {
        self.execute_sync(|| self.rollback_transaction(txn_id))
    }

    pub fn pending_transaction_intents(
        &self,
        txn_id: graphdb_core::types::TransactionId,
    ) -> Result<Vec<graphdb_core::wal::OutboxIntent>, SyncError> {
        Ok(self
            .pending_intents
            .get(&txn_id)
            .map(|intents| intents.clone())
            .unwrap_or_default())
    }

    /// Return the last staged intent sequence for a transaction.
    ///
    /// Savepoints retain intents whose sequence is less than or equal to the
    /// saved boundary, so an empty transaction maps to boundary zero.
    pub fn pending_transaction_intent_sequence(
        &self,
        txn_id: graphdb_core::types::TransactionId,
    ) -> u64 {
        self.pending_intents
            .get(&txn_id)
            .and_then(|intents| {
                intents
                    .last()
                    .map(|intent| u64::from(intent.intent_sequence))
            })
            .unwrap_or(0)
    }

    pub fn materialize_committed_transaction(
        &self,
        txn_id: graphdb_core::types::TransactionId,
        commit_lsn: graphdb_core::types::CommitLsn,
        intents: &[graphdb_core::wal::OutboxIntent],
    ) -> Result<(), SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(());
        };
        let mut targets = intents
            .iter()
            .map(|intent| intent.mutation.target.clone())
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        let materializer_started = std::time::Instant::now();
        self.execute_sync(|| async {
            outbox
                .materialize_commit(commit_lsn, intents, &targets)
                .await
                .map_err(SyncError::PersistenceError)
        })?;
        if let Some(stats) = &self.stats_manager {
            stats.record_materializer_latency(materializer_started.elapsed().as_millis() as u64);
        }
        log::debug!(
            "Materialized transaction {} at commit LSN {}",
            txn_id,
            commit_lsn
        );
        Ok(())
    }

    /// Return the durable projection frontier used as the lower bound for
    /// committed-WAL replay after an outbox restore.
    pub fn outbox_materialized_lsn(
        &self,
    ) -> Result<Option<graphdb_core::types::CommitLsn>, SyncError> {
        let Some(outbox) = &self.sqlite_outbox else {
            return Ok(None);
        };
        self.execute_sync(|| async {
            outbox
                .materialized_lsn()
                .await
                .map(Some)
                .map_err(SyncError::PersistenceError)
        })
    }
}
