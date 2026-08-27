use serde::{Deserialize, Serialize};

use crate::types::{
    IdempotencyKey, IndexGeneration, LabelId, OrderingKey, TargetId, TransactionId, VertexId,
};

pub const WAL_SYNC_WIRE_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityRef {
    Vertex(VertexId),
    Edge {
        src: VertexId,
        dst: VertexId,
        edge_type: LabelId,
        ranking: i64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexOperation {
    Upsert,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleMutation {
    Create,
    BeginBackfill,
    BeginCatchUp,
    Activate,
    BeginDrain,
    Drop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexMutation {
    pub wire_version: u16,
    pub target: TargetId,
    pub index_id: u64,
    pub index_generation: IndexGeneration,
    pub entity_ref: EntityRef,
    pub operation: IndexOperation,
    pub document_or_vector: Vec<u8>,
    pub idempotency_key: IdempotencyKey,
    pub ordering_key: OrderingKey,
}

impl IndexMutation {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_version(self.wire_version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxIntent {
    pub wire_version: u16,
    pub transaction_id: TransactionId,
    pub intent_sequence: u32,
    pub mutation: IndexMutation,
}

impl OutboxIntent {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_version(self.wire_version)?;
        self.mutation.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionCommit {
    pub wire_version: u16,
    pub transaction_id: TransactionId,
    pub intent_count: u32,
    pub batch_checksum: u32,
}

impl TransactionCommit {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_version(self.wire_version)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransactionAbort {
    pub wire_version: u16,
    pub transaction_id: TransactionId,
}

impl TransactionAbort {
    pub fn validate(&self) -> Result<(), String> {
        validate_wire_version(self.wire_version)
    }
}

fn validate_wire_version(version: u16) -> Result<(), String> {
    if version == WAL_SYNC_WIRE_VERSION {
        Ok(())
    } else {
        Err(format!(
            "Unsupported WAL sync wire version: expected {}, got {}",
            WAL_SYNC_WIRE_VERSION, version
        ))
    }
}

#[cfg(test)]
mod tests {
    use postcard::{from_bytes, to_allocvec};

    use super::{EntityRef, IndexMutation, IndexOperation, OutboxIntent, WAL_SYNC_WIRE_VERSION};
    use crate::types::{
        IdempotencyKey, IndexGeneration, OrderingKey, TargetId, TransactionId, VertexId,
    };

    fn mutation() -> IndexMutation {
        IndexMutation {
            wire_version: WAL_SYNC_WIRE_VERSION,
            target: TargetId::new("fulltext").expect("target should be valid"),
            index_id: 7,
            index_generation: IndexGeneration::new(3),
            entity_ref: EntityRef::Vertex(VertexId::from_int64(42)),
            operation: IndexOperation::Upsert,
            document_or_vector: b"document".to_vec(),
            idempotency_key: IdempotencyKey::new("txn-1:0")
                .expect("idempotency key should be valid"),
            ordering_key: OrderingKey::new("index-7:vertex-42")
                .expect("ordering key should be valid"),
        }
    }

    #[test]
    fn intent_roundtrips_without_transport_types() {
        let intent = OutboxIntent {
            wire_version: WAL_SYNC_WIRE_VERSION,
            transaction_id: TransactionId::new(1),
            intent_sequence: 0,
            mutation: mutation(),
        };
        let bytes = to_allocvec(&intent).expect("intent should serialize");
        let decoded: OutboxIntent = from_bytes(&bytes).expect("intent should deserialize");
        assert_eq!(decoded, intent);
        assert!(decoded.validate().is_ok());
    }

    #[test]
    fn unknown_wire_version_is_rejected() {
        let mut mutation = mutation();
        mutation.wire_version = WAL_SYNC_WIRE_VERSION + 1;
        assert!(mutation.validate().is_err());
    }
}
