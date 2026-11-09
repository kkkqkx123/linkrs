use super::common::{
    assert_search_result_contains, assert_search_result_not_contains, FulltextTestContext,
};

const SPACE_ID: u64 = 7;
const EDGE_TYPE: &str = "WROTE";
const FIELD: &str = "note";

#[tokio::test]
async fn test_edge_fulltext_lifecycle() {
    let ctx = FulltextTestContext::new();

    ctx.create_test_index(SPACE_ID, EDGE_TYPE, FIELD, None)
        .await
        .expect("edge type index creation should succeed");
    assert!(
        ctx.has_index(SPACE_ID, EDGE_TYPE, FIELD),
        "edge index should exist after creation"
    );

    ctx.manager
        .index_edge_property(
            SPACE_ID,
            EDGE_TYPE,
            FIELD,
            "alice->linkrs",
            "alice wrote a graph database manual",
        )
        .await
        .expect("edge property indexing should succeed");
    ctx.commit_all().await.expect("commit should succeed");

    let results = ctx
        .search(SPACE_ID, EDGE_TYPE, FIELD, "database manual", 10)
        .await
        .expect("edge search should succeed");
    assert_search_result_contains(&results, "alice->linkrs")
        .expect("edge document should be searchable");

    ctx.manager
        .delete_edge_index(SPACE_ID, EDGE_TYPE, "alice->linkrs")
        .await
        .expect("edge document delete should succeed");
    ctx.commit_all().await.expect("commit should succeed");
    let results = ctx
        .search(SPACE_ID, EDGE_TYPE, FIELD, "alice", 10)
        .await
        .expect("search after delete should succeed");
    assert_search_result_not_contains(&results, "alice->linkrs")
        .expect("deleted edge document should no longer match");

    ctx.drop_index(SPACE_ID, EDGE_TYPE, FIELD)
        .await
        .expect("edge index drop should succeed");
    assert!(
        !ctx.has_index(SPACE_ID, EDGE_TYPE, FIELD),
        "edge index should be gone after drop"
    );
}
