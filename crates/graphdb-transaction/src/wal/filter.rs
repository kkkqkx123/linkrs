//! WAL Intent Filtering for Index Rebuild
//!
//! Provides filtering of WAL intents for specific index rebuild operations.

use graphdb_core::types::CommitLsn;
use graphdb_core::wal::OutboxIntent;

use super::commit::CommittedWalTransaction;

/// Filter WAL intents for a specific index within an LSN range.
///
/// This is the core of generation rebuild catch-up phase. It extracts
/// only the intents that affect a specific index, allowing incremental
/// replay from a snapshot LSN to a barrier LSN.
pub fn filter_intents_for_index(
    transactions: &[CommittedWalTransaction],
    index_id: u64,
    from_lsn: CommitLsn,
    to_lsn: CommitLsn,
) -> Vec<OutboxIntent> {
    filter_intents_for_indexes(transactions, &[index_id], from_lsn, to_lsn)
}

/// Filter WAL intents for any of the stable identifiers associated with one
/// logical index. Results retain commit-LSN and intent-sequence ordering.
pub fn filter_intents_for_indexes(
    transactions: &[CommittedWalTransaction],
    index_ids: &[u64],
    from_lsn: CommitLsn,
    to_lsn: CommitLsn,
) -> Vec<OutboxIntent> {
    let mut filtered = Vec::new();

    for txn in transactions {
        // The snapshot already contains the commit at from_lsn. Catch-up is
        // therefore the half-open interval (from_lsn, to_lsn].
        if txn.commit_lsn <= from_lsn || txn.commit_lsn > to_lsn {
            continue;
        }

        for intent in &txn.intents {
            if index_ids.contains(&intent.mutation.index_id) {
                filtered.push((txn.commit_lsn, intent.clone()));
            }
        }
    }

    filtered.sort_by(|(a_lsn, a), (b_lsn, b)| {
        a_lsn
            .cmp(b_lsn)
            .then_with(|| a.transaction_id.cmp(&b.transaction_id))
            .then_with(|| a.intent_sequence.cmp(&b.intent_sequence))
    });

    filtered.into_iter().map(|(_, intent)| intent).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::types::{IndexGeneration, TransactionId};
    use graphdb_core::wal::IndexMutation;

    #[test]
    fn test_filter_intents_by_lsn_range() {
        let transactions = vec![
            CommittedWalTransaction {
                transaction_id: TransactionId::new(1),
                commit_lsn: CommitLsn::new(100),
                redo_entries: vec![],
                intents: vec![create_test_intent(TransactionId::new(1), 100, 0)],
            },
            CommittedWalTransaction {
                transaction_id: TransactionId::new(2),
                commit_lsn: CommitLsn::new(200),
                redo_entries: vec![],
                intents: vec![create_test_intent(TransactionId::new(2), 200, 0)],
            },
            CommittedWalTransaction {
                transaction_id: TransactionId::new(3),
                commit_lsn: CommitLsn::new(300),
                redo_entries: vec![],
                intents: vec![create_test_intent(TransactionId::new(3), 300, 0)],
            },
        ];

        let filtered =
            filter_intents_for_index(&transactions, 1, CommitLsn::new(150), CommitLsn::new(250));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].transaction_id, TransactionId::new(2));

        let filtered_at_snapshot =
            filter_intents_for_index(&transactions, 1, CommitLsn::new(200), CommitLsn::new(300));
        assert_eq!(filtered_at_snapshot.len(), 1);
        assert_eq!(
            filtered_at_snapshot[0].transaction_id,
            TransactionId::new(3)
        );
    }

    #[test]
    fn test_filter_intents_by_index_id() {
        let mut intent1 = create_test_intent(TransactionId::new(1), 100, 0);
        intent1.mutation.index_id = 1;
        let mut intent2 = create_test_intent(TransactionId::new(1), 100, 1);
        intent2.mutation.index_id = 2;
        let mut intent3 = create_test_intent(TransactionId::new(1), 100, 2);
        intent3.mutation.index_id = 3;

        let transactions = vec![CommittedWalTransaction {
            transaction_id: TransactionId::new(1),
            commit_lsn: CommitLsn::new(100),
            redo_entries: vec![],
            intents: vec![intent1, intent2, intent3],
        }];

        let filtered =
            filter_intents_for_index(&transactions, 2, CommitLsn::ZERO, CommitLsn::new(1000));

        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].mutation.index_id, 2);
    }

    #[test]
    fn test_filter_orders_by_commit_lsn() {
        let transactions = vec![
            CommittedWalTransaction {
                transaction_id: TransactionId::new(20),
                commit_lsn: CommitLsn::new(200),
                redo_entries: vec![],
                intents: vec![create_test_intent(TransactionId::new(20), 200, 0)],
            },
            CommittedWalTransaction {
                transaction_id: TransactionId::new(10),
                commit_lsn: CommitLsn::new(100),
                redo_entries: vec![],
                intents: vec![create_test_intent(TransactionId::new(10), 100, 0)],
            },
        ];

        let filtered =
            filter_intents_for_index(&transactions, 1, CommitLsn::ZERO, CommitLsn::new(300));

        assert_eq!(
            filtered
                .iter()
                .map(|intent| intent.transaction_id)
                .collect::<Vec<_>>(),
            vec![TransactionId::new(10), TransactionId::new(20)]
        );
    }

    fn create_test_intent(txn_id: TransactionId, lsn: u64, sequence: u32) -> OutboxIntent {
        use graphdb_core::types::{IdempotencyKey, OrderingKey, TargetId, VertexId};
        use graphdb_core::wal::EntityRef;

        OutboxIntent {
            wire_version: 1,
            transaction_id: txn_id,
            intent_sequence: sequence,
            mutation: IndexMutation {
                wire_version: 1,
                target: TargetId::new("fulltext").unwrap(),
                index_id: 1,
                index_generation: IndexGeneration::new(1),
                entity_ref: EntityRef::Vertex(VertexId::from_int64(lsn as i64)),
                operation: graphdb_core::wal::IndexOperation::Upsert,
                document_or_vector: vec![1, 2, 3],
                idempotency_key: IdempotencyKey::new(format!("{}-{}", lsn, sequence)).unwrap(),
                ordering_key: OrderingKey::new(format!("index-1-vertex-{}", lsn)).unwrap(),
            },
        }
    }
}
