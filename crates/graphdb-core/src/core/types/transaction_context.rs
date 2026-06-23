use crate::core::types::TransactionId;

#[derive(Debug, Clone)]
pub struct TransactionContextInfo {
    pub id: TransactionId,
    pub timestamp: u32,
    pub is_read_only: bool,
    pub sync_sequence: u64,
}

impl TransactionContextInfo {
    pub fn new(id: TransactionId, timestamp: u32, is_read_only: bool, sync_sequence: u64) -> Self {
        Self {
            id,
            timestamp,
            is_read_only,
            sync_sequence,
        }
    }
}
