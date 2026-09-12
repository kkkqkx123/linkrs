//! Regression tests for online vector index rebuild.
//!
//! Covers the purge primitive (field vs space granularity), the publish
//! fence identity, and one end-to-end rebuild through the durable outbox:
//! backfill restores purged points, strictly-newer events converge via
//! catch-up replay, and older pending events still drain through normal
//! delivery afterwards.

#![cfg(feature = "vector")]

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use graphdb_core::types::{CommitLsn, TransactionId, VertexId};
use graphdb_core::{Value, VectorValue};
use graphdb_fulltext::RebuildPhase;
use graphdb_sync::types::ChangeType;
use graphdb_sync::vector_sync::CollectionGranularity;
use graphdb_sync::{
    SyncManager, VectorBackend, VectorChangeContext, VectorChangeType, VectorDocSource,
    VectorPointData, VectorRebuildDoc, VectorRebuildOptions, VectorSyncCoordinator,
};
use vector_search::{DistanceMetric, LocalVectorEngine};

fn local_coordinator(directory: &tempfile::TempDir) -> Arc<VectorSyncCoordinator> {
    let engine = Arc::new(
        LocalVectorEngine::open(directory.path().join("vec"))
            .expect("local vector engine should open"),
    );
    Arc::new(VectorSyncCoordinator::new_without_embedding(
        VectorBackend::from_local_arc(engine),
        tokio::runtime::Handle::current(),
    ))
}

fn make_manager(coordinator: Arc<VectorSyncCoordinator>) -> SyncManager {
    SyncManager::new_without_fulltext().with_vector_coordinator(coordinator)
}

fn vector_property(value: Vec<f32>) -> Vec<(String, Value)> {
    vec![(
        "embedding".to_string(),
        Value::Vector(VectorValue::dense(value)),
    )]
}

fn vertex_id(value: &Value) -> VertexId {
    VertexId::try_from(value).expect("test vertex id should convert")
}

struct VecSource {
    docs: Vec<VectorRebuildDoc>,
    index: usize,
}

#[async_trait]
impl VectorDocSource for VecSource {
    async fn next_batch(&mut self) -> Result<Option<Vec<VectorRebuildDoc>>, String> {
        if self.index >= self.docs.len() {
            return Ok(None);
        }
        let batch = self.docs[self.index..].to_vec();
        self.index = self.docs.len();
        Ok(Some(batch))
    }
}

/// Source that serves one peek batch and then fails: simulates a primary
/// source (or remote dependency) breaking mid-backfill.
struct FailAfterPeekSource {
    docs: Vec<VectorRebuildDoc>,
    calls: usize,
}

#[async_trait]
impl VectorDocSource for FailAfterPeekSource {
    async fn next_batch(&mut self) -> Result<Option<Vec<VectorRebuildDoc>>, String> {
        self.calls += 1;
        if self.calls == 1 {
            return Ok(Some(self.docs.clone()));
        }
        Err("injected source failure mid-backfill".to_string())
    }
}

fn rebuild_doc(vertex: &Value, properties: Vec<(String, Value)>) -> VectorRebuildDoc {
    VectorRebuildDoc {
        vertex_id: vertex_id(vertex),
        properties,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_field_granularity_recreates_collection() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    coordinator
        .on_vector_change_batch(vec![VectorChangeContext::new(
            1,
            "user",
            "embedding",
            VectorChangeType::Insert,
            VectorPointData {
                id: "p1".to_string(),
                vector: vec![1.0, 0.0, 0.0, 0.0],
                payload: HashMap::new(),
            },
        )])
        .await
        .expect("point should upsert");
    assert_eq!(
        coordinator
            .backend()
            .count("space_1_user_embedding")
            .await
            .expect("count"),
        1
    );

    coordinator
        .index_manager()
        .purge_index_data(1, "user", "embedding")
        .await
        .expect("purge should succeed");

    assert!(
        coordinator.index_exists(1, "user", "embedding"),
        "purge must keep the logical registration"
    );
    assert_eq!(
        coordinator
            .backend()
            .count("space_1_user_embedding")
            .await
            .expect("count"),
        0,
        "purge must drop every point of the field collection"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn purge_space_granularity_keeps_siblings() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    // Share the physical collection: register the sibling logical index with
    // the stored config instead of creating the collection a second time.
    let config = coordinator
        .index_info(1, "user", "embedding")
        .expect("index info should exist")
        .config;
    coordinator.register_logical_index(1, "user", "bio", "space_1".to_string(), config, None);
    for field in ["embedding", "bio"] {
        coordinator
            .on_vector_change_batch(vec![VectorChangeContext::new(
                1,
                "user",
                field,
                VectorChangeType::Insert,
                VectorPointData {
                    id: format!("{field}-point"),
                    vector: vec![1.0, 0.0, 0.0, 0.0],
                    payload: HashMap::new(),
                },
            )])
            .await
            .expect("point should upsert");
    }
    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        2
    );

    coordinator
        .index_manager()
        .purge_index_data(1, "user", "embedding")
        .await
        .expect("purge should succeed");

    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        1,
        "purge must only remove the rebuilt group, siblings survive"
    );
    assert!(coordinator.index_exists(1, "user", "bio"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_fence_is_stable_per_index() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    let first = coordinator.publish_fence_for(1, "user", "embedding");
    let second = coordinator.publish_fence_for(1, "user", "embedding");
    assert!(
        Arc::ptr_eq(&first, &second),
        "delivery and rebuild must share one fence per index"
    );
    let other = coordinator.publish_fence_for(1, "user", "bio");
    assert!(
        !Arc::ptr_eq(&first, &other),
        "different indexes must not share a fence"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_converges_backfill_and_pending_delivery() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    let mut manager = make_manager(Arc::clone(&coordinator));
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");

    // v1 is delivered before the rebuild; the purge will remove its point.
    let v1 = Value::string("v1");
    let txn1 = TransactionId::from(1u64);
    manager
        .on_vertex_change_with_txn(
            txn1,
            1,
            "user",
            &v1,
            &vector_property(vec![1.0, 0.0, 0.0, 0.0]),
            ChangeType::Insert,
        )
        .expect("vertex change should stage");
    let intents1 = manager.pending_transaction_intents(txn1).expect("intents");
    manager
        .materialize_committed_transaction(txn1, CommitLsn::new(10), &intents1)
        .expect("materialize");
    manager.clear_transaction_intents(txn1);
    assert_eq!(manager.retry_outbox_sync().expect("delivery"), 1);

    // v2 is materialized but never delivered: it sits at the snapshot LSN,
    // so the rebuild must not lose it — it drains via normal delivery after.
    let v2 = Value::string("v2");
    let txn2 = TransactionId::from(2u64);
    manager
        .on_vertex_change_with_txn(
            txn2,
            1,
            "user",
            &v2,
            &vector_property(vec![0.0, 1.0, 0.0, 0.0]),
            ChangeType::Insert,
        )
        .expect("vertex change should stage");
    let intents2 = manager.pending_transaction_intents(txn2).expect("intents");
    manager
        .materialize_committed_transaction(txn2, CommitLsn::new(20), &intents2)
        .expect("materialize");
    manager.clear_transaction_intents(txn2);

    // Backfill enumerates primary storage (v1 only): the purge removed v1's
    // point, backfill restores it under the live point ID.
    let mut source = VecSource {
        docs: vec![rebuild_doc(&v1, vector_property(vec![1.0, 0.0, 0.0, 0.0]))],
        index: 0,
    };
    let applied = manager
        .rebuild_vector_index(
            1,
            "user",
            "embedding",
            &mut source,
            VectorRebuildOptions::default(),
        )
        .await
        .expect("rebuild should succeed");
    assert_eq!(
        applied, 1,
        "only the backfilled point is applied by the driver"
    );
    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        1,
        "purge plus backfill must converge to the scanned storage state"
    );

    let progress = coordinator
        .rebuild_progress(1, "user", "embedding")
        .expect("progress should be recorded");
    assert_eq!(progress.phase, RebuildPhase::Completed);

    // The pending v2 event drains through normal delivery afterwards.
    assert_eq!(manager.retry_outbox_sync().expect("delivery"), 1);
    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        2,
        "pending events at or below the snapshot must survive the rebuild"
    );
    let results = coordinator
        .search_by_location(1, "user", "embedding", vec![1.0, 0.0, 0.0, 0.0], 10)
        .await
        .expect("search should succeed");
    assert_eq!(results.len(), 2);

    // Rebuild is retryable: a second run over the full scan converges again.
    let mut source = VecSource {
        docs: vec![
            rebuild_doc(&v1, vector_property(vec![1.0, 0.0, 0.0, 0.0])),
            rebuild_doc(&v2, vector_property(vec![0.0, 1.0, 0.0, 0.0])),
        ],
        index: 0,
    };
    manager
        .rebuild_vector_index(
            1,
            "user",
            "embedding",
            &mut source,
            VectorRebuildOptions::default(),
        )
        .await
        .expect("second rebuild should succeed");
    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        2
    );
    assert_eq!(
        manager
            .recover_stale_vector_rebuilds()
            .await
            .expect("recovery should run"),
        0,
        "clean rebuilds leave no stranded generations"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temp_naming_isolates_granularities() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    let field_temp = coordinator
        .index_manager()
        .temp_name_for(1, "user", "embedding", 7);
    assert_eq!(field_temp, "space_1_user_embedding.rebuild-7");
    coordinator.set_granularity(CollectionGranularity::Space);
    let space_temp = coordinator
        .index_manager()
        .temp_name_for(1, "user", "embedding", 7);
    assert_eq!(space_temp, "space_1.rebuild-7__user_embedding");
    for temp in [&field_temp, &space_temp] {
        assert!(VectorBackend::is_temp_collection(temp));
        let (live, generation) =
            VectorBackend::parse_temp_collection(temp).expect("temp name should parse");
        assert_eq!(generation, 7);
        assert!(!VectorBackend::is_temp_collection(&live));
    }
    assert!(!VectorBackend::is_temp_collection("space_1"));
    assert!(VectorBackend::parse_temp_collection("space_1").is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn temp_collections_are_invisible_to_search() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    let temp = coordinator
        .index_manager()
        .temp_name_for(1, "user", "embedding", 7);
    coordinator
        .index_manager()
        .create_temp_collection(1, "user", "embedding", 7)
        .await
        .expect("temp should build");
    coordinator
        .on_vector_change_batch_to(
            &temp,
            vec![VectorChangeContext::new(
                1,
                "user",
                "embedding",
                VectorChangeType::Insert,
                VectorPointData {
                    id: "ghost".to_string(),
                    vector: vec![1.0, 0.0, 0.0, 0.0],
                    payload: HashMap::new(),
                },
            )],
        )
        .await
        .expect("temp write should succeed");
    assert_eq!(
        coordinator
            .backend()
            .count(&temp)
            .await
            .expect("temp count"),
        1
    );
    assert_eq!(
        coordinator
            .backend()
            .count("space_1_user_embedding")
            .await
            .expect("live count"),
        0,
        "temp points must not leak into live counts"
    );
    let results = coordinator
        .search_by_location(1, "user", "embedding", vec![1.0, 0.0, 0.0, 0.0], 10)
        .await
        .expect("search should succeed");
    assert!(
        results.is_empty(),
        "query path must never hit temp collections"
    );
    coordinator
        .index_manager()
        .drop_temp_collection(1, "user", "embedding", 7)
        .await
        .expect("temp discard should succeed");
    assert!(
        !coordinator.backend().index_exists(&temp),
        "discarded temp must be gone"
    );
    assert!(
        coordinator.index_exists(1, "user", "embedding"),
        "discarding a temp must keep the logical registration"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rebuild_failure_before_publish_leaves_live_untouched() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    let mut manager = make_manager(Arc::clone(&coordinator));
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    let live_vector = vec![1.0, 0.0, 0.0, 0.0];
    coordinator
        .on_vector_change_batch(vec![VectorChangeContext::new(
            1,
            "user",
            "embedding",
            VectorChangeType::Insert,
            VectorPointData {
                id: "live-point".to_string(),
                vector: live_vector.clone(),
                payload: HashMap::new(),
            },
        )])
        .await
        .expect("live point should upsert");

    // The peek batch passes, then the source breaks mid-backfill. Under the
    // old purge-in-place driver this destroyed the live slice; temp-swap
    // must fail with live data fully servable.
    let v = Value::string("v-rebuilt");
    let mut source = FailAfterPeekSource {
        docs: vec![rebuild_doc(&v, vector_property(vec![0.0, 1.0, 0.0, 0.0]))],
        calls: 0,
    };
    let result = manager
        .rebuild_vector_index(
            1,
            "user",
            "embedding",
            &mut source,
            VectorRebuildOptions::default(),
        )
        .await;
    assert!(
        result.is_err(),
        "injected source failure must fail the rebuild"
    );
    assert_eq!(
        coordinator
            .backend()
            .count("space_1_user_embedding")
            .await
            .expect("count"),
        1,
        "failed rebuild must not touch the live slice"
    );
    let kept = coordinator
        .backend()
        .get_vector("space_1_user_embedding", "live-point")
        .await
        .expect("get")
        .expect("live point must survive");
    assert_eq!(kept.vector, live_vector);
    let temps = coordinator
        .backend()
        .list_temp_collections()
        .await
        .expect("temp scan");
    assert!(temps.is_empty(), "failed rebuild must discard its temp");
    let progress = coordinator
        .rebuild_progress(1, "user", "embedding")
        .expect("progress should be recorded");
    assert_eq!(progress.phase, RebuildPhase::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn field_publish_swaps_collection_and_restore_reverses_it() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    let mut manager = make_manager(Arc::clone(&coordinator));
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    let old_vector = vec![1.0, 0.0, 0.0, 0.0];
    coordinator
        .on_vector_change_batch(vec![VectorChangeContext::new(
            1,
            "user",
            "embedding",
            VectorChangeType::Insert,
            VectorPointData {
                id: "old-point".to_string(),
                vector: old_vector.clone(),
                payload: HashMap::new(),
            },
        )])
        .await
        .expect("old point should upsert");

    let new_vector = vec![0.0, 0.0, 0.0, 1.0];
    let v = Value::string("v-new");
    let mut source = VecSource {
        docs: vec![rebuild_doc(&v, vector_property(new_vector.clone()))],
        index: 0,
    };
    manager
        .rebuild_vector_index(
            1,
            "user",
            "embedding",
            &mut source,
            VectorRebuildOptions::default(),
        )
        .await
        .expect("rebuild should succeed");
    assert_eq!(
        coordinator
            .backend()
            .count("space_1_user_embedding")
            .await
            .expect("count"),
        1,
        "publish must swap the converged temp over live"
    );
    let (live_points, _) = coordinator
        .backend()
        .scroll("space_1_user_embedding", 100, None, Some(false), Some(true))
        .await
        .expect("scroll");
    assert_eq!(live_points.len(), 1);
    assert_eq!(live_points[0].vector, new_vector);
    let backups: Vec<String> = coordinator
        .backend()
        .list_collections()
        .await
        .expect("collection scan")
        .into_iter()
        .filter(|name| name.starts_with("space_1_user_embedding.old-"))
        .collect();
    assert_eq!(backups.len(), 1, "field publish must retain one backup");
    assert_eq!(
        coordinator
            .backend()
            .count(&backups[0])
            .await
            .expect("backup count"),
        1
    );
    let temps = coordinator
        .backend()
        .list_temp_collections()
        .await
        .expect("temp scan");
    assert!(temps.is_empty(), "publish must leave no temp behind");

    let restored = coordinator
        .index_manager()
        .restore_promote_backup(1, "user", "embedding")
        .await
        .expect("bad-publish reverse should restore the backup");
    assert_eq!(restored, backups[0]);
    let (live_points, _) = coordinator
        .backend()
        .scroll("space_1_user_embedding", 100, None, Some(false), Some(true))
        .await
        .expect("scroll");
    assert_eq!(live_points.len(), 1);
    assert_eq!(live_points[0].vector, old_vector);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_publish_preserves_sibling_slices() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    let mut manager = make_manager(Arc::clone(&coordinator));
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    let config = coordinator
        .index_info(1, "user", "embedding")
        .expect("index info should exist")
        .config;
    coordinator.register_logical_index(1, "user", "bio", "space_1".to_string(), config, None);
    let sibling_vector = vec![0.0, 1.0, 0.0, 0.0];
    for (field, id, vector) in [
        ("embedding", "emb-old", vec![1.0, 0.0, 0.0, 0.0]),
        ("bio", "bio-point", sibling_vector.clone()),
    ] {
        coordinator
            .on_vector_change_batch(vec![VectorChangeContext::new(
                1,
                "user",
                field,
                VectorChangeType::Insert,
                VectorPointData {
                    id: id.to_string(),
                    vector,
                    payload: HashMap::new(),
                },
            )])
            .await
            .expect("point should upsert");
    }

    let new_vector = vec![0.0, 0.0, 1.0, 0.0];
    let v = Value::string("emb-new");
    let mut source = VecSource {
        docs: vec![rebuild_doc(&v, vector_property(new_vector.clone()))],
        index: 0,
    };
    manager
        .rebuild_vector_index(
            1,
            "user",
            "embedding",
            &mut source,
            VectorRebuildOptions::default(),
        )
        .await
        .expect("rebuild should succeed");
    assert_eq!(
        coordinator.backend().count("space_1").await.expect("count"),
        2,
        "slice publish must converge to one rebuilt point plus the sibling"
    );
    let embedding_hits = coordinator
        .search_by_location(1, "user", "embedding", new_vector.clone(), 10)
        .await
        .expect("search should succeed");
    assert_eq!(embedding_hits.len(), 1);
    let sibling_hits = coordinator
        .search_by_location(1, "user", "bio", sibling_vector, 10)
        .await
        .expect("search should succeed");
    assert_eq!(
        sibling_hits.len(),
        1,
        "sibling slice must survive the publish"
    );
    assert_eq!(sibling_hits[0].id.to_string(), "bio-point");
    for name in coordinator
        .backend()
        .list_collections()
        .await
        .expect("scan")
    {
        assert!(
            !name.contains(".rebuild-"),
            "publish must leave no temp behind, found {name}"
        );
        assert!(
            !name.contains(".old-"),
            "space publish must keep no backup, found {name}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovery_drops_orphan_temps() {
    let directory = tempfile::TempDir::new().expect("temporary directory");
    let coordinator = local_coordinator(&directory);
    coordinator.set_granularity(CollectionGranularity::Field);
    let mut manager = make_manager(Arc::clone(&coordinator));
    manager
        .configure_outbox(directory.path().join("outbox/outbox.sqlite"))
        .expect("outbox should configure");
    coordinator
        .create_vector_index(1, "user", "embedding", 4, DistanceMetric::Cosine)
        .await
        .expect("logical index should register");
    let temp = coordinator
        .index_manager()
        .create_temp_collection(1, "user", "embedding", 9)
        .await
        .expect("temp should build");
    assert!(coordinator.backend().index_exists(&temp));
    assert_eq!(
        manager
            .recover_stale_vector_rebuilds()
            .await
            .expect("recovery should run"),
        0,
        "orphan temps carry no generations to fail"
    );
    assert!(
        !coordinator.backend().index_exists(&temp),
        "recovery must drop orphan temps"
    );
    assert!(
        coordinator.index_exists(1, "user", "embedding"),
        "recovery must keep live registrations"
    );
}
