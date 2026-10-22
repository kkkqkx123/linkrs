//! Full-Text Search Planner
//!
//! Plans every full-text statement into its plan node:
//! - index management: `CREATE` / `DROP` / `ALTER` / `SHOW` / `DESCRIBE FULLTEXT INDEX`
//! - data access: `SEARCH` / `LOOKUP FULLTEXT` / `MATCH FULLTEXT`
//!
//! Only `CREATE` has a bound representation; the other variants reach the
//! planner as a bound AST statement and are dispatched here. Index locations
//! (`space_id` / `tag_name` / `field_name`) are resolved from plan-phase
//! metadata; an index that cannot be resolved is an error rather than a node
//! with empty location fields.

use std::sync::Arc;

use crate::metadata::{IndexMetadata, IndexType, MetadataContext};
use crate::parser::ast::fulltext::{
    AlterFulltextIndex, AlterIndexAction, CreateFulltextIndex, DescribeFulltextIndex,
    DropFulltextIndex, FulltextQueryExpr, IndexFieldDef, IndexOptions, LookupFulltext,
    MatchFulltext, SearchStatement, ShowFulltextIndex,
};
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::search::fulltext::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
    ShowFulltextIndexNode,
};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use linkrs_core::types::FulltextEngineType;

/// Full-text search planner
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
        // The full-text stack is opt-in. When the feature is disabled the
        // planner refuses every full-text statement with a readable message
        // instead of leaking "No planner for bound statement" or failing later
        // inside the operator.
        #[cfg(not(feature = "fulltext"))]
        {
            let _ = ctx;
            return Err(PlannerError::UnsupportedOperation(
                "fulltext feature is not enabled: rebuild linkrs with --features fulltext"
                    .to_string(),
            ));
        }

        #[cfg(feature = "fulltext")]
        {
            let space_name = ctx
                .qctx
                .space_name()
                .unwrap_or_else(|| "default".to_string());
            let space_id = ctx.qctx.space_id().unwrap_or(0);

            return match ctx.bound {
                crate::binder::BoundStatement::CreateFulltextIndex(create) => self
                    .transform_create(
                        &create.index_name,
                        &create.schema_name,
                        &create.fields,
                        create.engine_type,
                        &create.options,
                        create.if_not_exists,
                        &space_name,
                        space_id,
                        ctx.metadata,
                    ),
                crate::binder::BoundStatement::Other(stmt) => {
                    self.transform_bound(stmt, &space_name, space_id, ctx.metadata)
                }
                _ => Err(PlannerError::UnsupportedOperation(format!(
                    "FulltextSearchPlanner has no plan for bound statement: {}",
                    ctx.bound.kind()
                ))),
            };
        }
    }
}

impl FulltextSearchPlanner {
    /// Single-field-per-index engines cannot change indexed fields online.
    /// Fail fast for schema-changing actions; Optimize passes through.
    /// REBUILD is rejected in SQL: the executor carries the index manager
    /// but not the sync driver plus primary-storage scan needed for online
    /// rebuild. Use the async HTTP rebuild instead; the old clear-only
    /// behavior destroyed indexed data, so it must not run silently.
    pub(crate) const FULLTEXT_REBUILD_UNAVAILABLE: &str = "ALTER FULLTEXT INDEX REBUILD is not executed inline: trigger the async online rebuild via POST /v1/fulltext/indexes/rebuild and poll GET /v1/fulltext/rebuilds/{id}";

    /// Dispatch AST-level full-text statements with plan-phase metadata.
    ///
    /// `metadata_context` on the planner is the fallback for callers without
    /// plan-phase metadata.
    fn transform_bound(
        &self,
        stmt: &Stmt,
        space_name: &str,
        space_id: u64,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let metadata = metadata.or(self.metadata_context.as_deref());
        match stmt {
            Stmt::CreateFulltextIndex(create) => {
                self.transform_create_ast(create, space_name, space_id, metadata)
            }
            Stmt::DropFulltextIndex(drop) => self.transform_drop(drop),
            Stmt::AlterFulltextIndex(alter) => self.transform_alter(alter),
            Stmt::ShowFulltextIndex(show) => self.transform_show(show, space_name),
            Stmt::DescribeFulltextIndex(describe) => self.transform_describe(describe),
            Stmt::Search(search) => self.transform_search(search, metadata),
            Stmt::LookupFulltext(lookup) => self.transform_lookup(lookup, metadata),
            Stmt::MatchFulltext(matched) => self.transform_match(matched, space_id, metadata),
            _ => Err(PlannerError::PlanGenerationFailed(
                "Not a full-text statement".to_string(),
            )),
        }
    }

    // ============================================================================
    // Index Management Transformations
    // ============================================================================

    fn transform_create_ast(
        &self,
        create: &CreateFulltextIndex,
        space_name: &str,
        space_id: u64,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        self.transform_create(
            &create.index_name,
            &create.schema_name,
            &create.fields,
            create.engine_type,
            &create.options,
            create.if_not_exists,
            space_name,
            space_id,
            metadata,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn transform_create(
        &self,
        index_name: &str,
        schema_name: &str,
        fields: &[IndexFieldDef],
        engine_type: FulltextEngineType,
        options: &IndexOptions,
        if_not_exists: bool,
        space_name: &str,
        space_id: u64,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Validate schema exists: a fulltext index may target a vertex tag
        // or an edge type, which share one index namespace.
        let schema_name = if schema_name.is_empty() {
            space_name.to_string()
        } else {
            if let Some(metadata) = metadata {
                if !metadata.has_tag_metadata(schema_name)
                    && !metadata.has_edge_type_metadata(schema_name)
                {
                    return Err(PlannerError::TagNotFound(schema_name.to_string()));
                }
            }
            schema_name.to_string()
        };

        // Validate that the index doesn't already exist (unless requested).
        if !if_not_exists {
            if let Some(metadata) = metadata {
                if metadata.has_index_metadata(index_name) {
                    return Err(PlannerError::InvalidOperation(format!(
                        "Index '{}' already exists",
                        index_name
                    )));
                }
            }
        }

        let node = CreateFulltextIndexNode::new(
            index_name.to_string(),
            schema_name,
            fields.to_vec(),
            engine_type,
            options.clone(),
            if_not_exists,
            space_id,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_drop(&self, drop: &DropFulltextIndex) -> Result<SubPlan, PlannerError> {
        let node = DropFulltextIndexNode::new(drop.index_name.clone(), drop.if_exists);
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_alter(&self, alter: &AlterFulltextIndex) -> Result<SubPlan, PlannerError> {
        // Rebuilding online needs the sync driver and a primary-storage scan,
        // neither of which the DDL operator owns; reject before any feature
        // gate so the guidance is identical in every build.
        if alter
            .actions
            .iter()
            .any(|action| matches!(action, AlterIndexAction::Rebuild))
        {
            return Err(PlannerError::UnsupportedOperation(
                Self::FULLTEXT_REBUILD_UNAVAILABLE.to_string(),
            ));
        }
        let node = AlterFulltextIndexNode::new(alter.index_name.clone(), alter.actions.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_show(
        &self,
        show: &ShowFulltextIndex,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let from_schema = if show.from_schema.is_none() {
            Some(space_name.to_string())
        } else {
            show.from_schema.clone()
        };
        let node = ShowFulltextIndexNode::new(show.pattern.clone(), from_schema);
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_describe(
        &self,
        describe: &DescribeFulltextIndex,
    ) -> Result<SubPlan, PlannerError> {
        let node = DescribeFulltextIndexNode::new(describe.index_name.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Search Operations Transformations
    // ============================================================================

    fn transform_search(
        &self,
        search: &SearchStatement,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let index = self.require_fulltext_index(&search.index_name, metadata)?;
        self.validate_query_expr(&search.query)?;

        let node = FulltextSearchNode::new(
            search.index_name.clone(),
            search.query.clone(),
            search.yield_clause.clone(),
            search.where_clause.clone(),
            search.order_clause.clone(),
            search.limit.as_ref().map(|limit| limit.count),
            search.skip.as_ref().map(|skip| skip.count),
        )
        .with_metadata(
            index.space_id,
            index.tag_name.clone(),
            index.field_name.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_lookup(
        &self,
        lookup: &LookupFulltext,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let index = self.require_fulltext_index(&lookup.index_name, metadata)?;

        let schema_name = if lookup.schema_name.is_empty() {
            index.tag_name.clone()
        } else {
            // Vertex tags and edge types share one fulltext index namespace,
            // so accept either.
            if let Some(metadata) = metadata {
                if !metadata.has_tag_metadata(&lookup.schema_name)
                    && !metadata.has_edge_type_metadata(&lookup.schema_name)
                {
                    return Err(PlannerError::TagNotFound(lookup.schema_name.clone()));
                }
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
            index.space_id,
            index.tag_name.clone(),
            index.field_name.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_match(
        &self,
        matched: &MatchFulltext,
        space_id: u64,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let condition = &matched.fulltext_condition;
        let index = match &condition.index_name {
            Some(index_name) => self.require_fulltext_index(index_name, metadata)?,
            None => {
                let metadata = metadata.ok_or_else(|| {
                    PlannerError::PlanGenerationFailed(format!(
                        "Fulltext index for field '{}' cannot be located: schema metadata is unavailable",
                        condition.field
                    ))
                })?;
                metadata
                    .find_fulltext_index_by_field(space_id, &condition.field)
                    .ok_or_else(|| {
                        PlannerError::IndexNotFound(format!(
                            "no fulltext index covers field '{}'",
                            condition.field
                        ))
                    })?
            }
        };

        let node = MatchFulltextNode::new(
            matched.pattern.clone(),
            condition.clone(),
            matched.yield_clause.clone(),
        )
        .with_metadata(
            index.space_id,
            index.tag_name.clone(),
            index.field_name.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Validation Helpers
    // ============================================================================

    /// Resolve a full-text index and its `(space, tag, field)` location.
    ///
    /// Search statements cannot run without that location, so unknown index
    /// names and missing metadata are errors instead of empty fields.
    fn require_fulltext_index<'a>(
        &self,
        index_name: &str,
        metadata: Option<&'a MetadataContext>,
    ) -> Result<&'a IndexMetadata, PlannerError> {
        let metadata = metadata.ok_or_else(|| {
            PlannerError::PlanGenerationFailed(format!(
                "Fulltext index '{}' cannot be located: schema metadata is unavailable",
                index_name
            ))
        })?;
        let index = metadata
            .get_index_metadata(index_name)
            .ok_or_else(|| PlannerError::IndexNotFound(index_name.to_string()))?;
        if index.index_type != IndexType::Fulltext {
            return Err(PlannerError::InvalidOperation(format!(
                "Index '{}' is a {} index, not a fulltext index",
                index_name, index.index_type
            )));
        }
        Ok(index)
    }

    /// Validate full-text query expression
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
    use crate::metadata::{EdgeTypeMetadata, IndexMetadata, TagMetadata};
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use linkrs_core::types::span::Span;
    use std::collections::HashMap;

    fn test_fields() -> Vec<IndexFieldDef> {
        vec![IndexFieldDef {
            field_name: "content".to_string(),
            analyzer: None,
            boost: None,
        }]
    }

    fn test_options() -> IndexOptions {
        IndexOptions {
            bm25_config: None,
            common_options: HashMap::new(),
        }
    }

    fn metadata_with_tag_and_edge() -> MetadataContext {
        let mut metadata = MetadataContext::new();
        metadata.set_tag_metadata("Article".to_string(), TagMetadata::new("Article".into(), 1));
        metadata.set_edge_type_metadata(
            "WROTE".to_string(),
            EdgeTypeMetadata::new("WROTE".into(), 1),
        );
        metadata.set_index_metadata(
            "idx".to_string(),
            IndexMetadata::new(
                "idx".to_string(),
                1,
                "Article".to_string(),
                "content".to_string(),
                IndexType::Fulltext,
            ),
        );
        metadata
    }

    fn planned_node(plan: SubPlan) -> PlanNodeEnum {
        plan.root.expect("plan must have a root node")
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

        let create = |schema: &str| {
            planner.transform_create(
                "idx2",
                schema,
                &test_fields(),
                FulltextEngineType::Bm25,
                &test_options(),
                false,
                "default",
                1,
                Some(&metadata),
            )
        };
        assert!(create("Article").is_ok());
        assert!(create("WROTE").is_ok());
        assert!(matches!(
            create("Missing"),
            Err(PlannerError::TagNotFound(_))
        ));
    }

    #[test]
    fn test_lookup_accepts_vertex_tag_and_edge_type() {
        let planner = FulltextSearchPlanner::new();
        let metadata = metadata_with_tag_and_edge();
        let lookup = |schema: &str| LookupFulltext {
            span: Span::default(),
            schema_name: schema.to_string(),
            index_name: "idx".to_string(),
            query: "graph".to_string(),
            yield_clause: None,
            limit: None,
        };

        assert!(planner
            .transform_lookup(&lookup("Article"), Some(&metadata))
            .is_ok());
        assert!(planner
            .transform_lookup(&lookup("WROTE"), Some(&metadata))
            .is_ok());
        assert!(matches!(
            planner.transform_lookup(&lookup("Missing"), Some(&metadata)),
            Err(PlannerError::TagNotFound(_))
        ));
    }

    #[test]
    fn test_search_requires_known_index() {
        let planner = FulltextSearchPlanner::new();
        let metadata = metadata_with_tag_and_edge();
        let search = SearchStatement {
            span: Span::default(),
            index_name: "missing".to_string(),
            query: FulltextQueryExpr::Simple("graph".to_string()),
            yield_clause: None,
            where_clause: None,
            order_clause: None,
            limit: None,
            skip: None,
        };

        assert!(matches!(
            planner.transform_search(&search, Some(&metadata)),
            Err(PlannerError::IndexNotFound(_))
        ));
        assert!(matches!(
            planner.transform_search(&search, None),
            Err(PlannerError::PlanGenerationFailed(_))
        ));

        let known = SearchStatement {
            index_name: "idx".to_string(),
            ..search
        };
        let node = planned_node(
            planner
                .transform_search(&known, Some(&metadata))
                .expect("known index plans"),
        );
        match node {
            PlanNodeEnum::FulltextSearch(node) => {
                assert_eq!(node.space_id, 1);
                assert_eq!(node.tag_name, "Article");
                assert_eq!(node.field_name, "content");
            }
            other => panic!("unexpected node: {:?}", other),
        }
    }

    #[test]
    fn test_match_resolves_index_by_field() {
        let planner = FulltextSearchPlanner::new();
        let metadata = metadata_with_tag_and_edge();
        let matched = MatchFulltext {
            span: Span::default(),
            pattern: "(n:Article)".to_string(),
            fulltext_condition: crate::parser::ast::fulltext::FulltextMatchCondition {
                field: "content".to_string(),
                query: "graph".to_string(),
                index_name: None,
            },
            yield_clause: None,
        };

        let node = planned_node(
            planner
                .transform_match(&matched, 1, Some(&metadata))
                .expect("field resolution plans"),
        );
        match node {
            PlanNodeEnum::MatchFulltext(node) => {
                assert_eq!(node.tag_name, "Article");
                assert_eq!(node.field_name, "content");
            }
            other => panic!("unexpected node: {:?}", other),
        }

        let unknown_field = MatchFulltext {
            fulltext_condition: crate::parser::ast::fulltext::FulltextMatchCondition {
                field: "missing".to_string(),
                query: "graph".to_string(),
                index_name: None,
            },
            ..matched
        };
        assert!(matches!(
            planner.transform_match(&unknown_field, 1, Some(&metadata)),
            Err(PlannerError::IndexNotFound(_))
        ));
    }

    #[test]
    fn test_alter_rejects_rebuild() {
        let planner = FulltextSearchPlanner::new();
        let alter = AlterFulltextIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            actions: vec![AlterIndexAction::Rebuild],
        };

        assert!(matches!(
            planner.transform_alter(&alter),
            Err(PlannerError::UnsupportedOperation(message))
                if message == FulltextSearchPlanner::FULLTEXT_REBUILD_UNAVAILABLE
        ));

        let optimize = AlterFulltextIndex {
            actions: vec![AlterIndexAction::Optimize],
            ..alter
        };
        assert!(planner.transform_alter(&optimize).is_ok());
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

        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Boolean {
                must,
                should: vec![],
                must_not: vec![],
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
