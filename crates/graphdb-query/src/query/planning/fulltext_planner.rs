//! Full-Text Search Planner
//!
//! This module contains the planner for full-text search operations.
//! Provides functionality similar to VectorSearchPlanner but optimized for
//! full-text search operations.

use std::sync::Arc;

use crate::query::metadata::MetadataContext;
use crate::query::parser::ast::fulltext::{
    AlterFulltextIndex, CreateFulltextIndex, DescribeFulltextIndex, DropFulltextIndex,
    FulltextQueryExpr, LookupFulltext, MatchFulltext, SearchStatement, ShowFulltextIndex,
};
use crate::query::parser::ast::Stmt;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::search::fulltext::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, FulltextLookupNode, FulltextSearchNode, MatchFulltextNode,
    ShowFulltextIndexNode,
};
use crate::query::planning::plan::SubPlan;
use crate::query::planning::planner::{Planner, PlannerError, ValidatedStatement};
use crate::query::QueryContext;

/// Full-text search planner
///
/// This planner handles all full-text search related statements including:
/// - Index management (CREATE/DROP/ALTER/SHOW/DESCRIBE FULLTEXT INDEX)
/// - Search operations (SEARCH, LOOKUP FULLTEXT, MATCH with full-text)
#[derive(Debug, Clone, Default)]
pub struct FulltextSearchPlanner {
    /// Metadata context for pre-resolved metadata (optional for backward compatibility)
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
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        self.transform_impl(validated, qctx, self.metadata_context.as_deref())
    }

    fn match_planner(&self, stmt: &Stmt) -> bool {
        matches!(
            stmt,
            Stmt::CreateFulltextIndex(_)
                | Stmt::DropFulltextIndex(_)
                | Stmt::AlterFulltextIndex(_)
                | Stmt::ShowFulltextIndex(_)
                | Stmt::DescribeFulltextIndex(_)
                | Stmt::Search(_)
                | Stmt::LookupFulltext(_)
                | Stmt::MatchFulltext(_)
        )
    }

    fn transform_with_metadata(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        self.transform_impl(validated, qctx, Some(metadata_context))
    }
}

impl FulltextSearchPlanner {
    fn transform_impl(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
        metadata_context: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let stmt = validated.stmt();
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());

        let space_id = qctx.space_id().unwrap_or(0);

        match stmt {
            Stmt::CreateFulltextIndex(create) => match metadata_context {
                Some(metadata_context) => self.transform_create_fulltext_index_with_metadata(
                    create,
                    &space_name,
                    metadata_context,
                    space_id,
                ),
                None => self.transform_create_fulltext_index(create, &space_name, space_id),
            },
            Stmt::DropFulltextIndex(drop) => self.transform_drop_fulltext_index(drop),
            Stmt::AlterFulltextIndex(alter) => match metadata_context {
                Some(metadata_context) => {
                    self.transform_alter_fulltext_index_with_metadata(alter, metadata_context)
                }
                None => self.transform_alter_fulltext_index(alter),
            },
            Stmt::ShowFulltextIndex(show) => self.transform_show_fulltext_index(show, &space_name),
            Stmt::DescribeFulltextIndex(describe) => match metadata_context {
                Some(metadata_context) => {
                    self.transform_describe_fulltext_index_with_metadata(describe, metadata_context)
                }
                None => self.transform_describe_fulltext_index(describe),
            },
            Stmt::Search(search) => match metadata_context {
                Some(metadata_context) => {
                    self.transform_search_with_metadata(search, metadata_context)
                }
                None => self.transform_search(search),
            },
            Stmt::LookupFulltext(lookup) => match metadata_context {
                Some(metadata_context) => self.transform_lookup_fulltext_with_metadata(
                    lookup,
                    &space_name,
                    metadata_context,
                ),
                None => self.transform_lookup_fulltext(lookup, &space_name),
            },
            Stmt::MatchFulltext(match_stmt) => match metadata_context {
                Some(metadata_context) => {
                    self.transform_match_fulltext_with_metadata(match_stmt, metadata_context)
                }
                None => self.transform_match_fulltext(match_stmt),
            },
            _ => Err(PlannerError::PlanGenerationFailed(
                "Not a full-text search statement".to_string(),
            )),
        }
    }
}

impl FulltextSearchPlanner {
    // ============================================================================
    // Index Management Transformations
    // ============================================================================

    fn transform_create_fulltext_index(
        &self,
        create: &CreateFulltextIndex,
        space_name: &str,
        space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        let schema_name = if create.schema_name.is_empty() {
            space_name.to_string()
        } else {
            create.schema_name.clone()
        };

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

    fn transform_create_fulltext_index_with_metadata(
        &self,
        create: &CreateFulltextIndex,
        space_name: &str,
        metadata_context: &MetadataContext,
        space_id: u64,
    ) -> Result<SubPlan, PlannerError> {
        // Validate that the schema exists in metadata context
        let schema_name = if create.schema_name.is_empty() {
            space_name.to_string()
        } else {
            // Validate schema exists
            if !metadata_context.has_tag_metadata(&create.schema_name) {
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

    fn transform_drop_fulltext_index(
        &self,
        drop: &DropFulltextIndex,
    ) -> Result<SubPlan, PlannerError> {
        let node = DropFulltextIndexNode::new(drop.index_name.clone(), drop.if_exists);
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_alter_fulltext_index(
        &self,
        alter: &AlterFulltextIndex,
    ) -> Result<SubPlan, PlannerError> {
        let node = AlterFulltextIndexNode::new(alter.index_name.clone(), alter.actions.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_alter_fulltext_index_with_metadata(
        &self,
        alter: &AlterFulltextIndex,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate that the index exists
        if metadata_context
            .get_index_metadata(&alter.index_name)
            .is_none()
        {
            return Err(PlannerError::IndexNotFound(alter.index_name.clone()));
        }

        let node = AlterFulltextIndexNode::new(alter.index_name.clone(), alter.actions.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_show_fulltext_index(
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

    fn transform_describe_fulltext_index(
        &self,
        describe: &DescribeFulltextIndex,
    ) -> Result<SubPlan, PlannerError> {
        let node = DescribeFulltextIndexNode::new(describe.index_name.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_describe_fulltext_index_with_metadata(
        &self,
        describe: &DescribeFulltextIndex,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate that the index exists
        if metadata_context
            .get_index_metadata(&describe.index_name)
            .is_none()
        {
            return Err(PlannerError::IndexNotFound(describe.index_name.clone()));
        }

        let node = DescribeFulltextIndexNode::new(describe.index_name.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Search Operations Transformations
    // ============================================================================

    fn transform_search(&self, search: &SearchStatement) -> Result<SubPlan, PlannerError> {
        // Validate query expression
        self.validate_query_expr(&search.query)?;

        let node = FulltextSearchNode::new(
            search.index_name.clone(),
            search.query.clone(),
            search.yield_clause.clone(),
            search.where_clause.clone(),
            search.order_clause.clone(),
            search.limit,
            search.offset,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_search_with_metadata(
        &self,
        search: &SearchStatement,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate index exists and get metadata
        let index_metadata = metadata_context
            .get_index_metadata(&search.index_name)
            .ok_or_else(|| PlannerError::IndexNotFound(search.index_name.clone()))?;

        // Validate query expression
        self.validate_query_expr(&search.query)?;

        // Validate and optimize WHERE clause if present
        let where_clause = search.where_clause.clone();

        let node = FulltextSearchNode::new(
            search.index_name.clone(),
            search.query.clone(),
            search.yield_clause.clone(),
            where_clause,
            search.order_clause.clone(),
            search.limit,
            search.offset,
        )
        .with_metadata(
            index_metadata.space_id,
            index_metadata.tag_name.clone(),
            index_metadata.field_name.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_lookup_fulltext(
        &self,
        lookup: &LookupFulltext,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let schema_name = if lookup.schema_name.is_empty() {
            space_name.to_string()
        } else {
            lookup.schema_name.clone()
        };

        let node = FulltextLookupNode::new(
            schema_name,
            lookup.index_name.clone(),
            lookup.query.clone(),
            lookup.yield_clause.clone(),
            lookup.limit,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

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
            // Validate schema exists
            if !metadata_context.has_tag_metadata(&lookup.schema_name) {
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

    fn transform_match_fulltext(
        &self,
        match_stmt: &MatchFulltext,
    ) -> Result<SubPlan, PlannerError> {
        let node = MatchFulltextNode::new(
            match_stmt.pattern.clone(),
            match_stmt.fulltext_condition.clone(),
            match_stmt.yield_clause.clone(),
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_match_fulltext_with_metadata(
        &self,
        match_stmt: &MatchFulltext,
        metadata_context: &MetadataContext,
    ) -> Result<SubPlan, PlannerError> {
        // Validate that the field exists if an index is specified
        let mut space_id = 0u64;
        let mut tag_name = String::new();
        let mut field_name = String::new();

        if let Some(ref index_name) = match_stmt.fulltext_condition.index_name {
            let index_metadata = metadata_context
                .get_index_metadata(index_name)
                .ok_or_else(|| PlannerError::IndexNotFound(index_name.clone()))?;
            space_id = index_metadata.space_id;
            tag_name = index_metadata.tag_name.clone();
            field_name = index_metadata.field_name.clone();
        }

        // Validate that the field name is not empty
        if match_stmt.fulltext_condition.field.is_empty() {
            return Err(PlannerError::InvalidOperation(
                "Full-text field name cannot be empty".to_string(),
            ));
        }

        let node = MatchFulltextNode::new(
            match_stmt.pattern.clone(),
            match_stmt.fulltext_condition.clone(),
            match_stmt.yield_clause.clone(),
        )
        .with_metadata(space_id, tag_name, field_name);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    // ============================================================================
    // Validation Helpers
    // ============================================================================

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
    use crate::core::types::span::Span;
    use crate::core::types::FulltextEngineType;
    use crate::query::parser::ast::fulltext::{IndexFieldDef, IndexOptions};

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
    fn test_match_planner() {
        let planner = FulltextSearchPlanner::new();

        let create_stmt = Stmt::CreateFulltextIndex(CreateFulltextIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            schema_name: "tag".to_string(),
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
        });
        assert!(planner.match_planner(&create_stmt));

        let drop_stmt = Stmt::DropFulltextIndex(DropFulltextIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            if_exists: false,
        });
        assert!(planner.match_planner(&drop_stmt));
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
            .validate_query_expr(&FulltextQueryExpr::Fuzzy("database".to_string(), Some(2)))
            .is_ok());

        // Distance too large should fail
        assert!(planner
            .validate_query_expr(&FulltextQueryExpr::Fuzzy("database".to_string(), Some(10)))
            .is_err());
    }
}
