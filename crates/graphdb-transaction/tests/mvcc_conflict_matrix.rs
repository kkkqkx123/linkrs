//! Integration tests: MVCC write-set conflict matrix.
//!
//! Locks the certification semantics of `WriteSet::has_conflict_with` case by
//! case and documents where the resource-level granularity deliberately
//! differs from row-level checking:
//!
//! - same vertex written twice always conflicts, even for different
//!   properties (conservative: the write set tracks vertices, not columns);
//! - edges sharing an endpoint but identifying different edges do not
//!   conflict;
//! - deleting a vertex conflicts with any concurrent edge write touching it;
//! - schema changes conflict with concurrent data writes in either direction;
//! - index conflicts are keyed by resource name. Uniqueness enforcement for a
//!   specific key additionally requires the index layer to record the key as
//!   part of the resource; name-only recording (the current caller contract)
//!   over-aborts on same-index writes but never misses a same-name conflict.

use graphdb_core::types::{EdgeIdentifier, VertexId};
use graphdb_transaction::WriteSet;

fn vid(n: i64) -> VertexId {
    VertexId::try_from_int64(n).expect("test vertex id")
}

fn edge(src: i64, dst: i64, rank: i64) -> EdgeIdentifier {
    EdgeIdentifier::new(1, vid(src), 1, vid(dst), 1, rank)
}

#[test]
fn same_vertex_writes_conflict() {
    let mut a = WriteSet::new();
    a.record_vertex(vid(1));
    let mut b = WriteSet::new();
    b.record_vertex(vid(1));
    assert!(a.has_conflict_with(&b));
    assert!(b.has_conflict_with(&a));
}

#[test]
fn different_vertices_do_not_conflict() {
    let mut a = WriteSet::new();
    a.record_vertex(vid(1));
    let mut b = WriteSet::new();
    b.record_vertex(vid(2));
    assert!(!a.has_conflict_with(&b));
}

#[test]
fn shared_endpoint_different_edges_do_not_conflict() {
    let mut a = WriteSet::new();
    a.record_edge(edge(1, 2, 0));
    let mut b = WriteSet::new();
    b.record_edge(edge(1, 3, 0));
    assert!(!a.has_conflict_with(&b));
}

#[test]
fn same_edge_writes_conflict() {
    let mut a = WriteSet::new();
    a.record_edge(edge(1, 2, 0));
    let mut b = WriteSet::new();
    b.record_edge(edge(1, 2, 0));
    assert!(a.has_conflict_with(&b));
}

#[test]
fn vertex_delete_conflicts_with_edge_write_on_either_side() {
    let mut deleter = WriteSet::new();
    deleter.record_vertex_delete(vid(1));
    let mut edge_writer = WriteSet::new();
    edge_writer.record_edge(edge(1, 2, 0));
    assert!(deleter.has_conflict_with(&edge_writer));
    assert!(edge_writer.has_conflict_with(&deleter));
}

#[test]
fn vertex_delete_does_not_conflict_with_unrelated_edge() {
    let mut deleter = WriteSet::new();
    deleter.record_vertex_delete(vid(9));
    let mut edge_writer = WriteSet::new();
    edge_writer.record_edge(edge(1, 2, 0));
    assert!(!deleter.has_conflict_with(&edge_writer));
}

#[test]
fn schema_change_conflicts_with_concurrent_data_write() {
    let mut ddl = WriteSet::new();
    ddl.record_schema_resource("person");
    let mut dml = WriteSet::new();
    dml.record_vertex(vid(1));
    assert!(ddl.has_conflict_with(&dml));
    assert!(dml.has_conflict_with(&ddl));
}

#[test]
fn same_index_resource_conflicts() {
    let mut a = WriteSet::new();
    a.record_index_resource("person.name");
    let mut b = WriteSet::new();
    b.record_index_resource("person.name");
    assert!(a.has_conflict_with(&b));
}

#[test]
fn different_index_resources_do_not_conflict() {
    let mut a = WriteSet::new();
    a.record_index_resource("person.name");
    let mut b = WriteSet::new();
    b.record_index_resource("person.age");
    assert!(!a.has_conflict_with(&b));
}

#[test]
fn empty_write_sets_never_conflict() {
    let a = WriteSet::new();
    let b = WriteSet::new();
    assert!(!a.has_conflict_with(&b));
}
