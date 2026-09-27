//! Edge fulltext index lifecycle.
//!
//! Covers one edge type end to end: create the index on the edge type name,
//! index edge property text, search it, rebuild from a primary-storage-style
//! scan source, delete edge documents, and drop the index.

use super::common::{
    assert_search_result_contains, assert_search_result_not_contains, FulltextTestContext,
};
use async_trait::async_trait;
use graphdb_core::types::VertexId;
use graphdb_sync::batch::BatchConfig;
use graphdb_sync::builder::SyncManagerBuilder;
use graphdb_sync::coordinator::SyncCoordinator;
use graphdb_sync::{FulltextRebuildOptions, RebuildDoc, RebuildDocSource, RebuildEntity};
use std::sync::Arc;

const SPACE_ID: u64 = 7;
const EDGE_TYPE: &str = "WROTE";
const FIELD: &str = "note";

/// In-memory stand-in for the primary-storage scan: yields edge documents in
/// the same shape `StorageRebuildSource` produces for one edge type field.
struct EdgeScanSource {
    docs: Option<Vec<RebuildDoc>>,
}

impl EdgeScanSource {
    fn new() -> Self {
        let edge = |src: &str, dst: &str, ranking: i64, text: &str| RebuildDoc {
            entity: RebuildEntity::Edge {
                src: VertexId::try_from_string(src).expect("valid vertex id"),
                dst: VertexId::try_from_string(dst).expect("valid vertex id"),
                edge_type: EDGE_TYPE.to_string(),
                ranking,
            },
            text: text.to_string(),
        };
        Self {
            docs: Some(vec![
                edge("alice", "graphdb", 0, "alice wrote a graph database manual"),
                edge("bob", "graphdb", 0, "bob reviewed the graph database manual"),
            ]),
        }
    }
}

#[async_trait]
impl RebuildDocSource for EdgeScanSource {
    async fn next_batch(&mut self) -> Result<Option<Vec<RebuildDoc>>, String> {
        Ok(self.docs.take())
    }
}

#[tokio::test]
async fn test_edge_fulltext_lifecycle() {
    let ctx = FulltextTestContext::new();

    // Create the index directly on the edge type name.
    ctx.create_test_index(SPACE_ID, EDGE_TYPE, FIELD, None)
        .await
        .expect("edge type index creation should succeed");
    assert!(
        ctx.has_index(SPACE_ID, EDGE_TYPE, FIELD),
        "edge index should exist after creation"
    );

    // Index edge property text through the edge write path.
    ctx.manager
        .index_edge_property(
            SPACE_ID,
            EDGE_TYPE,
            FIELD,
            "alice->graphdb",
            "alice wrote a graph database manual",
        )
        .await
        .expect("edge property indexing should succeed");
    ctx.commit_all().await.expect("commit should succeed");

    let results = ctx
        .search(SPACE_ID, EDGE_TYPE, FIELD, "database manual", 10)
        .await
        .expect("edge search should succeed");
    assert_search_result_contains(&results, "alice->graphdb")
        .expect("edge document should be searchable");

    // Rebuild from a primary-storage-style scan source: the scratch engine
    // backfills both edge documents and publish swaps it in.
    let coordinator = Arc::new(SyncCoordinator::new(
        ctx.manager.clone(),
        BatchConfig::default(),
    ));
    let outbox_dir = tempfile::TempDir::new().expect("outbox temp dir should be created");
    let sync_manager = SyncManagerBuilder::new()
        .with_sync_coordinator(coordinator)
        .with_outbox_path(outbox_dir.path().join("outbox.sqlite"))
        .build()
        .expect("sync manager with outbox should build");
    let mut source = EdgeScanSource::new();
    let live_docs = sync_manager
        .rebuild_fulltext_index(
            SPACE_ID,
            EDGE_TYPE,
            FIELD,
            &mut source,
            FulltextRebuildOptions::default(),
        )
        .await
        .expect("edge index rebuild should succeed");
    assert_eq!(live_docs, 2, "rebuild should publish both edge documents");

    let results = ctx
        .search(SPACE_ID, EDGE_TYPE, FIELD, "graph database", 10)
        .await
        .expect("search after rebuild should succeed");
    assert_eq!(results.len(), 2, "both rebuilt edge documents should match");

    // Delete one backfilled edge document. Rebuild lands edge docs under
    // the receiver-formatted `{src}->{dst}` ID, matching the live path.
    ctx.manager
        .delete_edge_index(SPACE_ID, EDGE_TYPE, "alice->graphdb")
        .await
        .expect("edge document delete should succeed");
    ctx.commit_all().await.expect("commit should succeed");
    let results = ctx
        .search(SPACE_ID, EDGE_TYPE, FIELD, "alice", 10)
        .await
        .expect("search after delete should succeed");
    assert_search_result_not_contains(&results, "alice->graphdb")
        .expect("deleted edge document should no longer match");

    // Drop the index.
    ctx.drop_index(SPACE_ID, EDGE_TYPE, FIELD)
        .await
        .expect("edge index drop should succeed");
    assert!(
        !ctx.has_index(SPACE_ID, EDGE_TYPE, FIELD),
        "edge index should be gone after drop"
    );
}
