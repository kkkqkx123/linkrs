//! Full-Text Search Planner
//!
//! This module contains the planner for full-text search operations.
//! Provides functionality similar to VectorSearchPlanner but optimized for
//! full-text search operations.

use std::sync::Arc;

use crate::metadata::MetadataContext;
#[cfg(test)]
use crate::parser::ast::fulltext::CreateFulltextIndex;
#[cfg(test)]
use crate::parser::ast::fulltext::{FulltextQueryExpr, LookupFulltext};
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::search::fulltext::CreateFulltextIndexNode;
#[cfg(test)]
use crate::planning::plan::core::nodes::search::fulltext::FulltextLookupNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};

/// Full-text search planner
///
/// This planner handles all full-text search related statements including:
/// - Index management (CREATE/DROP/ALTER/SHOW/DESCRIBE FULLTEXT INDEX)
/// - Search operations (SEARCH, LOOKUP FULLTEXT, MATCH with full-text)
#[derive(Debug, Clone, Default)]
pub struct FulltextSearchPlanner {
    /// Metadata context for pre-resolved metadata (optional override).
    metadata_context: Option<Arc<MetadataContext>>,
}

impl FulltextSearchPlanner {
    /// Create a new full-text search planner
    pub fn new() -> Self {
        Self {
            metadata_context: None,
        }
    }

    /// Create a new full-text search planner with metadata context
    pub fn with_metadata_context(metadata_context: Arc<MetadataContext>) -> Self {
        Self {
            metadata_context: Some(metadata_context),
        }
    }
}

impl Planner for FulltextSearchPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let create = match bound {
            crate::binder::BoundStatement::CreateFulltextIndex(c) => c,
            // DROP/ALTER/SHOW/DESCRIBE/SEARCH variants have no bound
            // representation and are rejected here.
            _ => {
                return Err(PlannerError::UnsupportedOperation(format!(
                    "FulltextSearchPlanner has no plan for bound statement: {}",
                    bound.kind()
                )));
            }
        };

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let space_id = qctx.space_id().unwrap_or(0);
        let schema_name = if create.schema_name.is_empty() {
            space_name
        } else {
            create.schema_name.clone()
        };

        // Mirror the metadata-validated transform: reject unknown schemas
        // and duplicate index names when metadata is available.
        let planner_metadata = self.metadata_context.clone();
        let metadata = ctx.metadata.or(planner_metadata.as_deref());
        if let Some(metadata) = metadata {
            // Fulltext indexes share one namespace for vertex tags and edge
            // types: accept either, reject only unknown schema names.
            if !create.schema_name.is_empty()
                && !metadata.has_tag_metadata(&create.schema_name)
                && !metadata.has_edge_type_metadata(&create.schema_name)
            {
                return Err(PlannerError::TagNotFound(create.schema_name.clone()));
            }
            if !create.if_not_exists && metadata.has_index_metadata(&create.index_name) {
                return Err(PlannerError::InvalidOperation(format!(
                    "Index '{}' already exists",
                    create.index_name
                )));
            }
        }

        let node = CreateFulltextIndexNode::new(
            create.index_name.clone(),
            schema_name,
            create.fields.clone(),
            create.engine_type,
            create.options.clone(),
            create.if_not_exists,
            space_id,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }


}

impl FulltextSearchPlanner {
    // ============================================================================
    // Index Management Transformations
    // ============================================================================

    #[cfg(test)]
    fn transform_create_fulltext_index_with_metadata(
        &self,
        create: &CreateFulltextIndex,
        space_name: &str,
        metadata_context: &MetadataContext,
        space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        // Validate schema exists: a fulltext index may target a vertex tag
        // or an edge type, which share one index namespace.
        let schema_name = if create.schema_name.is_empty() {
            space_name.to_string()
        } else {
            // Validate schema exists
            if !metadata_context.has_tag_metadata(&create.schema_name)
                && !metadata_context.has_edge_type_metadata(&create.schema_name)
            {
                return Err(PlannerError::TagNotFound(create.schema_name.clone()));
            }
            create.schema_name.clone()
        };

        // Validate that index doesn't already exist (unless if_not_exists is true)
        if !create.if_not_exists && metadata_context.has_index_metadata(&create.index_name) {
            return Err(PlannerError::InvalidOperation(format!(
                "Index '{}' already exists",
                create.index_name
            )));
        }

        let node = CreateFulltextIndexNode::new(
            create.index_name.clone(),
            schema_name,
            create.fields.clone(),
            create.engine_type,
            create.options.clone(),
            create.if_not_exists,
            space_id,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Search Operations Transformations
    // ============================================================================

    #[cfg(test)]
    fn transform_lookup_fulltext_with_metadata(
        &self,
        lookup: &LookupFulltext,
        space_name: &str,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate index exists and get metadata
        let index_metadata = metadata_context
            .get_index_metadata(&lookup.index_name)
            .ok_or_else(|| PlannerError::IndexNotFound(lookup.index_name.clone()))?;

        let schema_name = if lookup.schema_name.is_empty() {
            space_name.to_string()
        } else {
            // Validate schema exists: vertex tags and edge types share one
            // fulltext index namespace, so accept either.
            if !metadata_context.has_tag_metadata(&lookup.schema_name)
                && !metadata_context.has_edge_type_metadata(&lookup.schema_name)
            {
                return Err(PlannerError::TagNotFound(lookup.schema_name.clone()));
            }
            lookup.schema_name.clone()
        };

        let node = FulltextLookupNode::new(
            schema_name,
            lookup.index_name.clone(),
            lookup.query.clone(),
            lookup.yield_clause.clone(),
            lookup.limit,
        )
        .with_metadata(
            index_metadata.space_id,
            index_metadata.tag_name.clone(),
            index_metadata.field_name.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Validation Helpers
    // ============================================================================

    /// Single-field-per-index engines cannot change indexed fields online.
    /// Fail fast for schema-changing actions; Optimize passes through.
    /// REBUILD is rejected in SQL: the executor carries the index manager
    /// but not the sync driver plus primary-storage scan needed for online
    /// rebuild. Use the async HTTP rebuild instead; the old clear-only
    /// behavior destroyed indexed data, so it must not run silently.
    #[cfg(feature = "fulltext")]
    pub(crate) const FULLTEXT_REBUILD_UNAVAILABLE: &str = "ALTER FULLTEXT INDEX REBUILD is not executed inline: trigger the async online rebuild via POST /v1/fulltext/indexes/rebuild and poll GET /v1/fulltext/rebuilds/{id}";

    /// Validate full-text query expression
    #[cfg(test)]
    fn validate_query_expr(&self, expr: &FulltextQueryExpr) -> Result<(), PlannerError> {
        match expr {
            FulltextQueryExpr::Simple(text) => {
                if text.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Query text cannot be empty".to_string(),
                    ));
                }
            }
            FulltextQueryExpr::Field(field, query) => {
                if field.is_empty() || query.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Field name and query text cannot be empty".to_string(),
                    ));
                }
            }
            FulltextQueryExpr::MultiField(fields) => {
                if fields.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Multi-field query must have at least one field".to_string(),
                    ));
                }
                for (field, query) in fields {
                    if field.is_empty() || query.is_empty() {
                        return Err(PlannerError::InvalidOperation(
                            "Field name and query text cannot be empty".to_string(),
                        ));
                    }
                }
            }
            FulltextQueryExpr::Boolean {
                must,
                should,
                must_not,
            } => {
                if must.is_empty() && should.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Boolean query must have at least one must or should clause".to_string(),
                    ));
                }
                for q in must.iter().chain(should.iter()).chain(must_not.iter()) {
                    self.validate_query_expr(q)?;
                }
            }
            FulltextQueryExpr::Phrase(text) => {
                if text.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Phrase query text cannot be empty".to_string(),
                    ));
                }
            }
            FulltextQueryExpr::Prefix(prefix) => {
                if prefix.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Prefix cannot be empty".to_string(),
                    ));
                }
            }
            FulltextQueryExpr::Fuzzy(text, distance) => {
                if text.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Fuzzy query text cannot be empty".to_string(),
                    ));
                }
                if let Some(d) = distance {
                    if *d > 5 {
                        return Err(PlannerError::InvalidOperation(
                            "Fuzzy distance must be between 0 and 5".to_string(),
                        ));
                    }
                }
            }
            FulltextQueryExpr::Range {
                field,
                lower,
                upper,
                ..
            } => {
                if field.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Range field cannot be empty".to_string(),
                    ));
                }
                if lower.is_none() && upper.is_none() {
                    return Err(PlannerError::InvalidOperation(
                        "Range query must have at least one bound".to_string(),
                    ));
                }
            }
            FulltextQueryExpr::Wildcard(pattern) => {
                if pattern.is_empty() {
                    return Err(PlannerError::InvalidOperation(
                        "Wildcard pattern cannot be empty".to_string(),
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{EdgeTypeMetadata, TagMetadata};
    use crate::parser::ast::fulltext::{IndexFieldDef, IndexOptions};
    use linkrs_core::types::span::Span;
    use linkrs_core::types::FulltextEngineType;

    fn test_create_stmt(schema_name: &str) -> CreateFulltextIndex {
        CreateFulltextIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            schema_name: schema_name.to_string(),
            fields: vec![IndexFieldDef {
                field_name: "content".to_string(),
                analyzer: None,
                boost: None,
            }],
            engine_type: FulltextEngineType::Bm25,
            options: IndexOptions {
                bm25_config: None,
                common_options: std::collections::HashMap::new(),
            },
            if_not_exists: false,
        }
    }

    fn metadata_with_tag_and_edge() -> MetadataContext {
        let mut metadata = MetadataContext::new();
        metadata.set_tag_metadata("Article".to_string(), TagMetadata::new("Article".into(), 1));
        metadata.set_edge_type_metadata(
            "WROTE".to_string(),
            EdgeTypeMetadata::new("WROTE".into(), 1),
        );
        metadata
    }

    #[test]
    fn test_fulltext_search_planner_new() {
        let planner = FulltextSearchPlanner::new();
        assert!(planner.metadata_context.is_none());
    }

    #[test]
    fn test_fulltext_search_planner_with_metadata() {
        let metadata_context = Arc::new(MetadataContext::new());
        let planner = FulltextSearchPlanner::with_metadata_context(metadata_context);
        assert!(planner.metadata_context.is_some());
    }


    #[test]
    fn test_create_accepts_vertex_tag_and_edge_type() {
        let planner = FulltextSearchPlanner::new();
        let metadata = metadata_with_tag_and_edge();

        assert!(planner
            .transform_create_fulltext_index_with_metadata(
                &test_create_stmt("Article"),
                "default",
                &metadata,
                1
            )
            .is_ok());
        assert!(planner
            .transform_create_fulltext_index_with_metadata(
                &test_create_stmt("WROTE"),
                "default",
                &metadata,
                1
            )
            .is_ok());
        let unknown = planner.transform_create_fulltext_index_with_metadata(
            &test_create_stmt("Missing"),
            "default",
            &metadata,
            1,
        );
        assert!(matches!(unknown, Err(PlannerError::TagNotFound(_))));
    }

    #[test]
    fn test_lookup_accepts_vertex_tag_and_edge_type() {
        let planner = FulltextSearchPlanner::new();
        let metadata = metadata_with_tag_and_edge();
        // Lookup requires the named index to exist in metadata.
        let mut metadata = metadata;
        metadata.set_index_metadata(
            "idx".to_string(),
            crate::metadata::IndexMetadata::new(
                "idx".to_string(),
                1,
                "WROTE".to_string(),
                "content".to_string(),
                crate::metadata::IndexType::Fulltext,
            ),
        );

        let lookup = |schema: &str| LookupFulltext {
            span: Span::default(),
            schema_name: schema.to_string(),
            index_name: "idx".to_string(),
            query: "graph".to_string(),
            yield_clause: None,
            limit: None,
        };
        assert!(planner
            .transform_lookup_fulltext_with_metadata(&lookup("Article"), "default", &metadata)
            .is_ok());
        assert!(planner
            .transform_lookup_fulltext_with_metadata(&lookup("WROTE"), "default", &metadata)
            .is_ok());
        let unknown = planner.transform_lookup_fulltext_with_metadata(
            &lookup("Missing"),
            "default",
            &metadata,
        );
        assert!(matches!(unknown, Err(PlannerError::TagNotFound(_))));
    }

    #[test]
    fn test_validate_query_expr_simple() {
        let planner = FulltextSearchPlanner::new();

        // Valid simple query
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Simple("database".to_string()))
            .is_ok());

        // Empty query should fail
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Simple("".to_string()))
            .is_err());
    }

    #[test]
    fn test_validate_query_expr_field() {
        let planner = FulltextSearchPlanner::new();

        // Valid field query
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Field(
                "title".to_string(),
                "database".to_string()
            ))
            .is_ok());

        // Empty field should fail
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Field(
                "".to_string(),
                "query".to_string()
            ))
            .is_err());
    }

    #[test]
    fn test_validate_query_expr_boolean() {
        let planner = FulltextSearchPlanner::new();

        // Valid boolean query
        let must = vec![FulltextQueryExpr::Simple("database".to_string())];
        let should = vec![];
        let must_not = vec![];

        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Boolean {
                must,
                should,
                must_not,
            })
            .is_ok());

        // Empty boolean query should fail
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Boolean {
                must: vec![],
                should: vec![],
                must_not: vec![],
            })
            .is_err());
    }

    #[test]
    fn test_validate_query_expr_fuzzy() {
        let planner = FulltextSearchPlanner::new();

        // Valid fuzzy query
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Fuzzy("database".into(), Some(2)))
            .is_ok());

        // Distance too large should fail
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Fuzzy("database".into(), Some(10)))
            .is_err());
    }
}
