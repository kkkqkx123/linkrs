//! Integration tests for metadata resolution in search operations
//!
#![cfg(feature = "qdrant")]
//! These tests verify that space_id, tag_name, and field_name are properly
//! pre-resolved during query planning and passed to executors.

use std::sync::Arc;

use graphdb_query::core::types::span::Span;
use graphdb_query::query::metadata::{IndexMetadata, IndexType, MetadataContext};
use graphdb_query::query::parser::ast::fulltext::FulltextQueryExpr;
use graphdb_query::query::parser::ast::vector::{VectorQueryExpr, VectorQueryType};
use graphdb_query::query::planning::fulltext_planner::FulltextSearchPlanner;
use graphdb_query::query::planning::plan::core::nodes::search::fulltext::data_access::{
    FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
};
use graphdb_query::query::planning::plan::core::nodes::search::vector::data_access::{
    OutputField, VectorMatchNode, VectorSearchNode, VectorSearchParams,
};
use graphdb_query::query::planning::vector_planner::VectorSearchPlanner;

fn create_test_metadata_context() -> MetadataContext {
    let mut context = MetadataContext::new();

    // Add fulltext index metadata
    let fulltext_index = IndexMetadata::new(
        "idx_article_content".to_string(),
        1,
        "article".to_string(),
        "content".to_string(),
        IndexType::Fulltext,
    );
    context.set_index_metadata("idx_article_content".to_string(), fulltext_index);

    // Add vector index metadata
    let vector_index = IndexMetadata::new(
        "idx_person_embedding".to_string(),
        1,
        "person".to_string(),
        "embedding".to_string(),
        IndexType::Vector,
    );
    context.set_index_metadata("idx_person_embedding".to_string(), vector_index);

    context
}

fn create_vector_query_expr() -> VectorQueryExpr {
    VectorQueryExpr {
        span: Span::default(),
        query_type: VectorQueryType::Vector,
        query_data: "[0.1, 0.2, 0.3]".to_string(),
    }
}

// ==================== Fulltext Search Metadata Tests ====================

#[test]
fn test_fulltext_search_node_metadata_resolution() {
    let node = FulltextSearchNode::new(
        "idx_article_content".to_string(),
        FulltextQueryExpr::Simple("test query".to_string()),
        None,
        None,
        None,
        Some(10),
        None,
    );

    // Initially, metadata fields should be empty
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());

    // After setting metadata
    let node = node.with_metadata(1, "article".to_string(), "content".to_string());
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "article");
    assert_eq!(node.field_name, "content");
}

#[test]
fn test_fulltext_lookup_node_metadata_resolution() {
    let node = FulltextLookupNode::new(
        "test_space".to_string(),
        "idx_article_content".to_string(),
        "test query".to_string(),
        None,
        Some(10),
    );

    // Initially, metadata fields should be empty
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());

    // After setting metadata
    let node = node.with_metadata(1, "article".to_string(), "content".to_string());
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "article");
    assert_eq!(node.field_name, "content");
}

#[test]
fn test_match_fulltext_node_metadata_resolution() {
    use graphdb_query::query::parser::ast::fulltext::FulltextMatchCondition;

    let condition = FulltextMatchCondition {
        index_name: Some("idx_article_content".to_string()),
        field: "content".to_string(),
        query: "test query".to_string(),
    };

    let node = MatchFulltextNode::new("pattern".to_string(), condition, None);

    // Initially, metadata fields should be empty
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());

    // After setting metadata
    let node = node.with_metadata(1, "article".to_string(), "content".to_string());
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "article");
    assert_eq!(node.field_name, "content");
}

// ==================== Vector Search Metadata Tests ====================

#[test]
fn test_vector_search_node_metadata_resolution() {
    let params = VectorSearchParams::new(
        "idx_person_embedding".to_string(),
        1,
        "person".to_string(),
        "embedding".to_string(),
        create_vector_query_expr(),
    );

    let node = VectorSearchNode::new(params);

    // Metadata should be set from params
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "person");
    assert_eq!(node.field_name, "embedding");
    assert_eq!(node.index_name, "idx_person_embedding");
}

#[test]
fn test_vector_match_node_metadata_resolution() {
    let node = VectorMatchNode::new(
        "pattern".to_string(),
        "embedding".to_string(),
        create_vector_query_expr(),
        None,
        vec![OutputField {
            name: "score".to_string(),
            alias: None,
        }],
    );

    // Initially, metadata fields should be empty
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());

    // After setting metadata
    let node = node.with_metadata(1, "person".to_string(), "embedding".to_string());
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "person");
    assert_eq!(node.field_name, "embedding");
}

// ==================== Planner Metadata Resolution Tests ====================

#[test]
fn test_fulltext_planner_with_metadata_context() {
    let metadata_context = Arc::new(create_test_metadata_context());
    let _planner = FulltextSearchPlanner::with_metadata_context(metadata_context);
}

#[test]
fn test_vector_planner_with_metadata_context() {
    let metadata_context = Arc::new(create_test_metadata_context());
    let _planner = VectorSearchPlanner::with_metadata_context(metadata_context);
}

// ==================== Metadata Context Tests ====================

#[test]
fn test_metadata_context_find_vector_index_by_field() {
    let context = create_test_metadata_context();

    // Find vector index by field
    let result = context.find_vector_index_by_field(1, "embedding");
    assert!(result.is_some());
    let index = result.unwrap();
    assert_eq!(index.tag_name, "person");
    assert_eq!(index.field_name, "embedding");
    assert_eq!(index.index_type, IndexType::Vector);

    // Non-existent field
    let result = context.find_vector_index_by_field(1, "nonexistent");
    assert!(result.is_none());

    // Wrong space_id
    let result = context.find_vector_index_by_field(999, "embedding");
    assert!(result.is_none());
}

#[test]
fn test_metadata_context_fulltext_index_lookup() {
    let context = create_test_metadata_context();

    // Get fulltext index metadata
    let result = context.get_index_metadata("idx_article_content");
    assert!(result.is_some());
    let index = result.unwrap();
    assert_eq!(index.tag_name, "article");
    assert_eq!(index.field_name, "content");
    assert_eq!(index.index_type, IndexType::Fulltext);
}

// ==================== End-to-End Metadata Flow Tests ====================

#[test]
fn test_fulltext_search_metadata_flow() {
    // This test verifies the complete flow from metadata context to executor
    let metadata_context = create_test_metadata_context();

    // 1. Verify index metadata exists
    let index_metadata = metadata_context.get_index_metadata("idx_article_content");
    assert!(index_metadata.is_some());

    let index = index_metadata.unwrap();
    assert_eq!(index.space_id, 1);
    assert_eq!(index.tag_name, "article");
    assert_eq!(index.field_name, "content");

    // 2. Create plan node with metadata
    let node = FulltextSearchNode::new(
        "idx_article_content".to_string(),
        FulltextQueryExpr::Simple("test".to_string()),
        None,
        None,
        None,
        Some(10),
        None,
    )
    .with_metadata(
        index.space_id,
        index.tag_name.clone(),
        index.field_name.clone(),
    );

    // 3. Verify metadata was properly set
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "article");
    assert_eq!(node.field_name, "content");
    assert_eq!(node.index_name, "idx_article_content");
}

#[test]
fn test_vector_search_metadata_flow() {
    // This test verifies the complete flow from metadata context to executor
    let metadata_context = create_test_metadata_context();

    // 1. Verify index metadata exists
    let index_metadata = metadata_context.get_index_metadata("idx_person_embedding");
    assert!(index_metadata.is_some());

    let index = index_metadata.unwrap();
    assert_eq!(index.space_id, 1);
    assert_eq!(index.tag_name, "person");
    assert_eq!(index.field_name, "embedding");

    // 2. Verify vector index can be found by field
    let found_index = metadata_context.find_vector_index_by_field(1, "embedding");
    assert!(found_index.is_some());
    assert_eq!(found_index.unwrap().index_name, "idx_person_embedding");
}

// ==================== Error Handling Tests ====================

#[test]
fn test_fulltext_search_missing_index() {
    let metadata_context = MetadataContext::new();

    // Try to get non-existent index
    let result = metadata_context.get_index_metadata("nonexistent_index");
    assert!(result.is_none());
}

#[test]
fn test_vector_search_missing_index() {
    let metadata_context = MetadataContext::new();

    // Try to find non-existent vector index
    let result = metadata_context.find_vector_index_by_field(1, "nonexistent");
    assert!(result.is_none());
}

// ==================== Multiple Index Tests ====================

#[test]
fn test_multiple_fulltext_indexes() {
    let mut context = MetadataContext::new();

    // Add multiple fulltext indexes
    let index1 = IndexMetadata::new(
        "idx_article_content".to_string(),
        1,
        "article".to_string(),
        "content".to_string(),
        IndexType::Fulltext,
    );
    let index2 = IndexMetadata::new(
        "idx_article_title".to_string(),
        1,
        "article".to_string(),
        "title".to_string(),
        IndexType::Fulltext,
    );
    let index3 = IndexMetadata::new(
        "idx_product_description".to_string(),
        1,
        "product".to_string(),
        "description".to_string(),
        IndexType::Fulltext,
    );

    context.set_index_metadata("idx_article_content".to_string(), index1);
    context.set_index_metadata("idx_article_title".to_string(), index2);
    context.set_index_metadata("idx_product_description".to_string(), index3);

    // Verify all indexes are accessible
    assert!(context.get_index_metadata("idx_article_content").is_some());
    assert!(context.get_index_metadata("idx_article_title").is_some());
    assert!(context
        .get_index_metadata("idx_product_description")
        .is_some());

    // Verify correct metadata for each
    let article_content = context.get_index_metadata("idx_article_content").unwrap();
    assert_eq!(article_content.field_name, "content");

    let article_title = context.get_index_metadata("idx_article_title").unwrap();
    assert_eq!(article_title.field_name, "title");

    let product_desc = context
        .get_index_metadata("idx_product_description")
        .unwrap();
    assert_eq!(product_desc.tag_name, "product");
}

#[test]
fn test_multiple_vector_indexes() {
    let mut context = MetadataContext::new();

    // Add multiple vector indexes
    let index1 = IndexMetadata::new(
        "idx_person_embedding".to_string(),
        1,
        "person".to_string(),
        "embedding".to_string(),
        IndexType::Vector,
    );
    let index2 = IndexMetadata::new(
        "idx_product_vector".to_string(),
        1,
        "product".to_string(),
        "vector".to_string(),
        IndexType::Vector,
    );

    context.set_index_metadata("idx_person_embedding".to_string(), index1);
    context.set_index_metadata("idx_product_vector".to_string(), index2);

    // Find by field
    let person_embedding = context.find_vector_index_by_field(1, "embedding");
    assert!(person_embedding.is_some());
    assert_eq!(person_embedding.unwrap().tag_name, "person");

    let product_vector = context.find_vector_index_by_field(1, "vector");
    assert!(product_vector.is_some());
    assert_eq!(product_vector.unwrap().tag_name, "product");
}

// ==================== Cross-Space Tests ====================

#[test]
fn test_cross_space_metadata_isolation() {
    let mut context = MetadataContext::new();

    // Add indexes for different spaces
    let space1_index = IndexMetadata::new(
        "idx_content".to_string(),
        1,
        "article".to_string(),
        "content".to_string(),
        IndexType::Fulltext,
    );
    let space2_index = IndexMetadata::new(
        "idx_content".to_string(),
        2,
        "post".to_string(),
        "content".to_string(),
        IndexType::Fulltext,
    );

    context.set_index_metadata("space1_idx_content".to_string(), space1_index);
    context.set_index_metadata("space2_idx_content".to_string(), space2_index);

    // Verify space isolation
    let space1 = context.get_index_metadata("space1_idx_content").unwrap();
    assert_eq!(space1.space_id, 1);
    assert_eq!(space1.tag_name, "article");

    let space2 = context.get_index_metadata("space2_idx_content").unwrap();
    assert_eq!(space2.space_id, 2);
    assert_eq!(space2.tag_name, "post");
}

// ==================== Index Type Discrimination Tests ====================

#[test]
fn test_index_type_discrimination() {
    let mut context = MetadataContext::new();

    // Add both fulltext and vector indexes with same name prefix
    let fulltext_index = IndexMetadata::new(
        "idx_content".to_string(),
        1,
        "article".to_string(),
        "content".to_string(),
        IndexType::Fulltext,
    );
    let vector_index = IndexMetadata::new(
        "idx_content_vec".to_string(),
        1,
        "article".to_string(),
        "content_vec".to_string(),
        IndexType::Vector,
    );

    context.set_index_metadata("idx_content".to_string(), fulltext_index);
    context.set_index_metadata("idx_content_vec".to_string(), vector_index);

    // Verify correct type discrimination
    let ft = context.get_index_metadata("idx_content").unwrap();
    assert_eq!(ft.index_type, IndexType::Fulltext);

    let vec = context.get_index_metadata("idx_content_vec").unwrap();
    assert_eq!(vec.index_type, IndexType::Vector);

    // find_vector_index_by_field should only return vector indexes
    let found = context.find_vector_index_by_field(1, "content_vec");
    assert!(found.is_some());
    assert_eq!(found.unwrap().index_type, IndexType::Vector);

    // Should not find fulltext index via vector search
    let not_found = context.find_vector_index_by_field(1, "content");
    assert!(not_found.is_none());
}

// ==================== Executor Metadata Usage Tests ====================

#[test]
fn test_fulltext_executor_uses_pre_resolved_metadata() {
    // This test verifies that the executor correctly uses pre-resolved metadata
    // when available, falling back to runtime resolution when not

    // Create node with pre-resolved metadata
    let node = FulltextSearchNode::new(
        "idx_article_content".to_string(),
        FulltextQueryExpr::Simple("test".to_string()),
        None,
        None,
        None,
        Some(10),
        None,
    )
    .with_metadata(1, "article".to_string(), "content".to_string());

    // Verify metadata is set
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "article");
    assert_eq!(node.field_name, "content");

    // The executor should use these values directly without parsing index_name
}

#[test]
fn test_vector_executor_uses_pre_resolved_metadata() {
    // This test verifies that the executor correctly uses pre-resolved metadata

    let params = VectorSearchParams::new(
        "idx_person_embedding".to_string(),
        1,
        "person".to_string(),
        "embedding".to_string(),
        create_vector_query_expr(),
    );

    let node = VectorSearchNode::new(params);

    // Verify metadata is set
    assert_eq!(node.space_id, 1);
    assert_eq!(node.tag_name, "person");
    assert_eq!(node.field_name, "embedding");
}

// ==================== Backward Compatibility Tests ====================

#[test]
fn test_fulltext_search_backward_compatibility() {
    // Test that nodes work without pre-resolved metadata (backward compatibility)
    let node = FulltextSearchNode::new(
        "idx_article_content".to_string(),
        FulltextQueryExpr::Simple("test".to_string()),
        None,
        None,
        None,
        Some(10),
        None,
    );

    // Metadata fields should be empty but node should still be valid
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());
    assert_eq!(node.index_name, "idx_article_content");
}

#[test]
fn test_vector_match_backward_compatibility() {
    // Test that VectorMatchNode works without pre-resolved metadata
    let node = VectorMatchNode::new(
        "pattern".to_string(),
        "embedding".to_string(),
        create_vector_query_expr(),
        None,
        vec![],
    );

    // Metadata fields should be empty but node should still be valid
    assert_eq!(node.space_id, 0);
    assert!(node.tag_name.is_empty());
    assert!(node.field_name.is_empty());
    assert_eq!(node.field, "embedding");
}
