use crc32fast::Hasher;

use crate::core::types::{CommitLsn, Timestamp, TransactionId};
use crate::core::wal::{
    OutboxIntent, TransactionAbort, TransactionCommit, WalError, WalOpType, WalResult,
};

use super::parser::ParsedWalEntry;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionWalEntry {
    pub op_type: WalOpType,
    pub timestamp: Timestamp,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CommittedWalTransaction {
    pub transaction_id: TransactionId,
    pub commit_lsn: CommitLsn,
    pub redo_entries: Vec<ParsedWalEntry>,
    pub intents: Vec<OutboxIntent>,
}

pub fn batch_checksum(entries: &[TransactionWalEntry]) -> u32 {
    let mut hasher = Hasher::new();
    for entry in entries {
        checksum_entry(&mut hasher, entry.op_type, entry.timestamp, &entry.payload);
    }
    hasher.finalize()
}

fn parsed_batch_checksum(entries: &[ParsedWalEntry]) -> WalResult<u32> {
    let mut hasher = Hasher::new();
    for entry in entries {
        let op_type = WalOpType::try_from(entry.header.op_type)?;
        checksum_entry(&mut hasher, op_type, entry.header.timestamp, &entry.payload);
    }
    Ok(hasher.finalize())
}

fn checksum_entry(hasher: &mut Hasher, op_type: WalOpType, timestamp: Timestamp, payload: &[u8]) {
    hasher.update(&[op_type as u8]);
    hasher.update(&timestamp.to_le_bytes());
    hasher.update(&(payload.len() as u64).to_le_bytes());
    hasher.update(payload);
}

pub fn collect_committed_transactions(
    entries: &[ParsedWalEntry],
) -> WalResult<Vec<CommittedWalTransaction>> {
    let mut committed = Vec::new();
    let mut pending = Vec::new();

    for entry in entries {
        match WalOpType::try_from(entry.header.op_type)? {
            WalOpType::TransactionAbort => {
                let abort: TransactionAbort = postcard::from_bytes(&entry.payload)?;
                abort.validate().map_err(WalError::InvalidOperation)?;
                pending.clear();
            }
            WalOpType::TransactionCommit => {
                let commit: TransactionCommit = postcard::from_bytes(&entry.payload)?;
                commit.validate().map_err(WalError::InvalidOperation)?;
                // During the migration window, legacy records can precede a
                // transactional batch in the same WAL stream. The writer's
                // checksum covers only the transactional suffix, so locate
                // that exact suffix instead of accidentally treating legacy
                // records as part of the batch. A missing match remains a
                // hard corruption error.
                let batch_start = (0..=pending.len())
                    .rev()
                    .find(|start| {
                        parsed_batch_checksum(&pending[*start..])
                            .map(|checksum| checksum == commit.batch_checksum)
                            .unwrap_or(false)
                    })
                    .ok_or_else(|| WalError::ChecksumMismatch {
                        expected: commit.batch_checksum,
                        actual: parsed_batch_checksum(&pending).unwrap_or_default(),
                    })?;
                let batch = pending.split_off(batch_start);
                pending.clear();
                let mut redo_entries = Vec::new();
                let mut intents = Vec::new();
                for pending_entry in batch {
                    if WalOpType::try_from(pending_entry.header.op_type)? == WalOpType::OutboxIntent
                    {
                        let intent: OutboxIntent = postcard::from_bytes(&pending_entry.payload)?;
                        intent.validate().map_err(WalError::InvalidOperation)?;
                        if intent.transaction_id != commit.transaction_id {
                            return Err(WalError::InvalidOperation(format!(
                                "Intent transaction {} does not match commit transaction {}",
                                intent.transaction_id, commit.transaction_id
                            )));
                        }
                        intents.push(intent);
                    } else {
                        redo_entries.push(pending_entry);
                    }
                }
                intents.sort_by_key(|intent| intent.intent_sequence);
                if intents.len() != commit.intent_count as usize {
                    return Err(WalError::InvalidOperation(format!(
                        "Commit expected {} intents, recovered {}",
                        commit.intent_count,
                        intents.len()
                    )));
                }
                for (expected, intent) in intents.iter().enumerate() {
                    if intent.intent_sequence as usize != expected {
                        return Err(WalError::InvalidOperation(format!(
                            "Intent sequence is not contiguous: expected {}, got {}",
                            expected, intent.intent_sequence
                        )));
                    }
                }
                committed.push(CommittedWalTransaction {
                    transaction_id: commit.transaction_id,
                    commit_lsn: CommitLsn::new(entry.lsn.as_u64()),
                    redo_entries,
                    intents,
                });
            }
            _ => pending.push(entry.clone()),
        }
    }

    Ok(committed)
}

#[cfg(test)]
mod tests {
    use super::{batch_checksum, TransactionWalEntry};
    use crate::core::wal::WalOpType;

    #[test]
    fn checksum_covers_operation_timestamp_and_payload() {
        let entry = TransactionWalEntry {
            op_type: WalOpType::InsertVertex,
            timestamp: 10,
            payload: vec![1, 2, 3],
        };
        let checksum = batch_checksum(std::slice::from_ref(&entry));
        let mut changed = entry;
        changed.timestamp = 11;
        assert_ne!(checksum, batch_checksum(&[changed]));
    }
}
