//! Acceptance tests for transaction-level vertex staging: online writes
//! accumulate in the per-transaction buffer, reads inside the transaction
//! compose their own writes, foreign snapshots see nothing until the
//! commit publishes the timestamp, and an abort leaves no trace.

use super::*;
use graphdb_core::types::TransactionId;

fn next_ts(storage: &GraphStorage) -> Timestamp {
    storage
        .version_manager()
        .acquire_insert_timestamp()
        .expect("acquire write ts")
}

fn publish(storage: &GraphStorage, ts: Timestamp) {
    storage
        .version_manager()
        .commit_ordered(ts)
        .expect("ordered commit");
}

fn front(storage: &GraphStorage) -> Timestamp {
    storage.version_manager().read_timestamp()
}

fn writer_handle(storage: &GraphStorage, txn: u64, ts: Timestamp) -> GraphStorage {
    storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
        TransactionId::from(txn),
        ts,
        Some(ts),
        false,
        false,
    ))
}

fn reader_handle(storage: &GraphStorage, txn: u64) -> GraphStorage {
    let snapshot = front(storage);
    storage.bind_operation_context(StorageOperationContext::transaction_with_timestamps(
        TransactionId::from(txn),
        snapshot,
        None,
        true,
        false,
    ))
}

fn person(id: i64, name: &str, age: i64) -> Vertex {
    Vertex::new(
        VertexId::try_from_int64(id).expect("test vertex id"),
        Tag::new(
            "Person".to_string(),
            [
                ("id".to_string(), Value::BigInt(id)),
                ("name".to_string(), Value::string(name)),
                ("age".to_string(), Value::BigInt(age)),
            ]
            .into_iter()
            .collect(),
        ),
    )
}

fn vid(id: i64) -> VertexId {
    VertexId::try_from_int64(id).expect("test vertex id")
}

fn age_of(vertex: &Vertex) -> i64 {
    match vertex.property_value("age").expect("age property") {
        Value::BigInt(age) => age,
        other => panic!("age must be BigInt, got {other:?}"),
    }
}

#[test]
fn staged_vertex_writes_invisible_until_commit_publishes() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let ts = next_ts(&storage);
    let mut writer = writer_handle(&storage, 1, ts);
    writer
        .insert_vertex("test_space", person(1, "alice", 30))
        .expect("staged insert");

    // A foreign snapshot sees no bytes of the staged row.
    let reader = reader_handle(&storage, 2);
    assert!(reader
        .get_vertex("test_space", "Person", &vid(1))
        .expect("point read")
        .is_none());
    assert!(reader.scan_vertices("test_space").expect("scan").is_empty());

    // Applying the buffer without publishing the timestamp keeps the row
    // private: the commit-apply window is still gated.
    storage
        .commit_staged_writes(TransactionId::from(1), &[])
        .expect("commit apply");
    let reader = reader_handle(&storage, 3);
    assert!(reader
        .get_vertex("test_space", "Person", &vid(1))
        .expect("point read during apply window")
        .is_none());

    publish(&storage, ts);
    let reader = reader_handle(&storage, 4);
    let alice = reader
        .get_vertex("test_space", "Person", &vid(1))
        .expect("read after publish")
        .expect("row visible after publish");
    assert_eq!(age_of(&alice), 30);
    assert_eq!(
        reader
            .scan_vertices("test_space")
            .expect("scan after publish")
            .len(),
        1
    );
}

#[test]
fn rolled_back_transaction_leaves_no_trace() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    // A committed baseline row for the update-and-abort case.
    let base = next_ts(&storage);
    let mut opener = writer_handle(&storage, 10, base);
    opener
        .insert_vertex("test_space", person(1, "alice", 30))
        .expect("baseline insert");
    drop(opener);
    storage
        .commit_staged_writes(TransactionId::from(10), &[])
        .expect("baseline commit");
    publish(&storage, base);

    let ts = next_ts(&storage);
    let mut writer = writer_handle(&storage, 20, ts);
    writer
        .insert_vertex("test_space", person(2, "bob", 25))
        .expect("staged insert");
    writer
        .update_vertex("test_space", person(1, "alice", 99))
        .expect("staged update of committed row");
    writer
        .delete_vertex("test_space", "Person", &vid(1))
        .expect("staged delete of committed row");

    storage
        .abort_staged_writes(TransactionId::from(20))
        .expect("abort");
    storage.version_manager().abort_write_timestamp(ts);

    // Nothing global survived: no staging buffer, no staged WAL redo, and
    // the main table answers exactly as before the transaction.
    assert!(storage
        .ctx
        .peek_txn_staging_mark(TransactionId::from(20))
        .is_none());
    assert_eq!(storage.ctx.staged_wal_len(), 0);
    let reader = reader_handle(&storage, 30);
    let alice = reader
        .get_vertex("test_space", "Person", &vid(1))
        .expect("read after abort")
        .expect("baseline row intact after abort");
    assert_eq!(age_of(&alice), 30);
    assert!(reader
        .get_vertex("test_space", "Person", &vid(2))
        .expect("read after abort")
        .is_none());
    assert_eq!(
        reader
            .scan_vertices("test_space")
            .expect("scan after abort")
            .len(),
        1
    );

    // The rolled-back slot is reusable: a fresh transaction writes the
    // same external id cleanly.
    let ts = next_ts(&storage);
    let mut writer = writer_handle(&storage, 40, ts);
    writer
        .insert_vertex("test_space", person(2, "bob", 40))
        .expect("re-insert after abort");
    storage
        .commit_staged_writes(TransactionId::from(40), &[])
        .expect("commit re-insert");
    publish(&storage, ts);
    let reader = reader_handle(&storage, 50);
    assert_eq!(
        reader
            .scan_vertices("test_space")
            .expect("scan after re-insert")
            .len(),
        2
    );
}

#[test]
fn read_your_own_staged_writes_covers_merged_forms() {
    let mut storage = create_test_storage();
    setup_space(&mut storage);
    setup_person_tag(&mut storage);

    let base = next_ts(&storage);
    let mut opener = writer_handle(&storage, 10, base);
    opener
        .insert_vertex("test_space", person(1, "alice", 30))
        .expect("baseline insert");
    drop(opener);
    storage
        .commit_staged_writes(TransactionId::from(10), &[])
        .expect("baseline commit");
    publish(&storage, base);

    let ts = next_ts(&storage);
    let mut writer = writer_handle(&storage, 20, ts);

    // Plain insert: visible to the owning transaction at once.
    writer
        .insert_vertex("test_space", person(2, "bob", 25))
        .expect("staged insert");
    let bob = writer
        .get_vertex("test_space", "Person", &vid(2))
        .expect("own insert must resolve")
        .expect("own insert visible");
    assert_eq!(age_of(&bob), 25);

    // Update folded into the staged insert: the latest column wins.
    writer
        .update_vertex("test_space", person(2, "bob", 26))
        .expect("staged update of own insert");
    let bob = writer
        .get_vertex("test_space", "Person", &vid(2))
        .expect("re-read after update")
        .expect("updated own insert still visible");
    assert_eq!(age_of(&bob), 26);

    // Insert then delete cancels the row.
    writer
        .insert_vertex("test_space", person(3, "carol", 28))
        .expect("staged insert before delete");
    writer
        .delete_vertex("test_space", "Person", &vid(3))
        .expect("staged delete of own insert");
    assert!(writer
        .get_vertex("test_space", "Person", &vid(3))
        .expect("cancelled row must resolve")
        .is_none());

    // Delete then insert folds to a whole-row update of the committed row.
    writer
        .delete_vertex("test_space", "Person", &vid(1))
        .expect("staged delete of committed row");
    assert!(writer
        .get_vertex("test_space", "Person", &vid(1))
        .expect("tombstone must resolve")
        .is_none());
    writer
        .insert_vertex("test_space", person(1, "alice", 31))
        .expect("staged re-insert over own delete");
    let alice = writer
        .get_vertex("test_space", "Person", &vid(1))
        .expect("re-inserted row must resolve")
        .expect("re-insert visible");
    assert_eq!(age_of(&alice), 31);

    // Predicated scans inside the transaction compose the same view.
    let mut scanned: Vec<i64> = writer
        .scan_vertices("test_space")
        .expect("own scan")
        .iter()
        .map(|v| v.vid.as_int64().expect("bigint vid"))
        .collect();
    scanned.sort_unstable();
    assert_eq!(scanned, vec![1, 2]);

    storage
        .commit_staged_writes(TransactionId::from(20), &[])
        .expect("commit");
    publish(&storage, ts);

    // After the commit the merged view is the table's view.
    let reader = reader_handle(&storage, 60);
    let alice = reader
        .get_vertex("test_space", "Person", &vid(1))
        .expect("read after commit")
        .expect("delete-then-insert landed as an update");
    assert_eq!(age_of(&alice), 31);
    let bob = reader
        .get_vertex("test_space", "Person", &vid(2))
        .expect("read after commit")
        .expect("insert-with-update landed");
    assert_eq!(age_of(&bob), 26);
    assert!(reader
        .get_vertex("test_space", "Person", &vid(3))
        .expect("read after commit")
        .is_none());
}
