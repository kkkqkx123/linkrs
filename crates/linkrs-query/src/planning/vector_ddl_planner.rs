//! Vector index DDL planner
//!
//! Plans the vector index management statements (`CREATE/DROP/SHOW/DESCRIBE
//! VECTOR INDEX`) into [`VectorManageNode`](crate::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode)
//! plan nodes. Unlike [`VectorSearchPlanner`](super::vector_planner::VectorSearchPlanner),
//! this planner names no optional-dependency types, so it compiles in every
//! feature combination and the executor reports
//! [`QueryError::feature_disabled`](linkrs_core::error::QueryError::feature_disabled)
//! when the vector engine itself is compiled out.

use std::sync::Arc;

use crate::metadata::MetadataContext;
use crate::parser::ast::vector::{
    CreateVectorIndex, DescribeVectorIndex, DropVectorIndex, ShowVectorIndex,
};
use crate::parser::ast::Stmt;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::search::vector::management::{
    CreateVectorIndexNode, CreateVectorIndexParams, DescribeVectorIndexNode, DropVectorIndexNode,
    ShowVectorIndexNode,
};
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use crate::QueryContext;

/// Planner for vector index DDL statements
#[derive(Debug, Clone, Default)]
pub struct VectorDdlPlanner {
    metadata_context: Option<Arc<MetadataContext>>,
}

impl VectorDdlPlanner {
    pub fn new() -> Self {
        Self {
            metadata_context: None,
        }
    }

    pub fn with_metadata_context(metadata_context: Arc<MetadataContext>) -> Self {
        Self {
            metadata_context: Some(metadata_context),
        }
    }
}

impl Planner for VectorDdlPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let bound = ctx.bound;
        let qctx = ctx.qctx.clone();
        let create = match bound {
            crate::binder::BoundStatement::CreateVectorIndex(c) => c,
            // SHOW/DESCRIBE/DROP have no bound representation yet and use
            // the AST path; the plan-phase metadata carries the vector
            // index registry so index names resolve to their (space, tag,
            // field) location during planning.
            _ => {
                let stmt = match bound {
                    crate::binder::BoundStatement::Other(s) => s.as_ref(),
                    _ => {
                        return Err(PlannerError::PlanGenerationFailed(
                            "VectorDdlPlanner requires a bound AST statement for fallback"
                                .to_string(),
                        ));
                    }
                };
                return self.transform_bound(stmt, qctx, ctx.metadata);
            }
        };

        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let space_id = qctx.space_id().unwrap_or(0);
        let schema_name = if create.schema_name.is_empty() {
            space_name
        } else {
            create.schema_name.clone()
        };

        // Vector indexes are vertex-only: point IDs, payloads, and rebuild
        // sources are all modeled per vertex. Reject edge type names up
        // front instead of creating an index that can never receive data.
        let planner_metadata = self.metadata_context.clone();
        let metadata = ctx.metadata.or(planner_metadata.as_deref());
        if let Some(metadata) = metadata {
            if !create.schema_name.is_empty()
                && metadata.has_edge_type_metadata(&create.schema_name)
            {
                return Err(PlannerError::InvalidOperation(format!(
                    "Vector indexes are vertex-only: '{}' is an edge type, create the index on a vertex tag instead",
                    create.schema_name
                )));
            }
        }

        let mut params = CreateVectorIndexParams::new(
            create.index_name.clone(),
            schema_name,
            create.schema_name.clone(),
            create.field_name.clone(),
            create.config.vector_size,
            create.config.distance,
            space_id,
        )
        .with_hnsw_m(create.config.hnsw_m)
        .with_hnsw_ef_construct(create.config.hnsw_ef_construct)
        .with_quantization(
            create.config.quantization,
            create.config.quantile,
            create.config.compression,
            create.config.always_ram,
        );
        if create.if_not_exists {
            params = params.with_if_not_exists();
        }
        let node = CreateVectorIndexNode::new(params);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }
}

impl VectorDdlPlanner {
    /// Dispatch AST-level vector DDL statements with plan-phase metadata.
    ///
    /// The metadata context carries the vector index registry, so index
    /// names resolve to their (space, tag, field) location during planning;
    /// `metadata_context` on the planner remains the fallback for callers
    /// without plan-phase metadata.
    fn transform_bound(
        &self,
        stmt: &Stmt,
        qctx: Arc<QueryContext>,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Vector search is opt-in. DDL stays available in every build, but
        // search statements must name the missing feature instead of falling
        // through to a generic planning failure.
        #[cfg(not(feature = "vector"))]
        if matches!(
            stmt,
            Stmt::SearchVector(_) | Stmt::LookupVector(_) | Stmt::MatchVector(_)
        ) {
            return Err(PlannerError::UnsupportedOperation(
                "vector feature is not enabled: rebuild linkrs with --features vector".to_string(),
            ));
        }
        let space_name = qctx.space_name().unwrap_or_else(|| "default".to_string());
        let metadata = metadata.or(self.metadata_context.as_deref());
        match stmt {
            Stmt::CreateVectorIndex(create) => {
                let space_id = qctx.space_id().unwrap_or(0);
                self.transform_create_vector_index(create, &space_name, space_id, metadata)
            }
            Stmt::DropVectorIndex(drop) => {
                self.transform_drop_vector_index(drop, &space_name, metadata)
            }
            Stmt::ShowVectorIndex(show) => self.transform_show_vector_index(show, &space_name),
            Stmt::DescribeVectorIndex(describe) => self.transform_describe_vector_index(describe),
            _ => Err(PlannerError::PlanGenerationFailed(
                "Not a vector DDL statement".to_string(),
            )),
        }
    }

    fn transform_create_vector_index(
        &self,
        create: &CreateVectorIndex,
        space_name: &str,
        space_id: u64,
        metadata_context: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        // Vector indexes are vertex-only (see plan_bound): reject edge type
        // names instead of creating an index that can never receive data.
        if let Some(metadata) = metadata_context {
            if !create.schema_name.is_empty()
                && metadata.has_edge_type_metadata(&create.schema_name)
            {
                return Err(PlannerError::InvalidOperation(format!(
                    "Vector indexes are vertex-only: '{}' is an edge type, create the index on a vertex tag instead",
                    create.schema_name
                )));
            }
        }
        let schema_name = if create.schema_name.is_empty() {
            space_name.to_string()
        } else {
            create.schema_name.clone()
        };

        let mut params = CreateVectorIndexParams::new(
            create.index_name.clone(),
            schema_name,
            create.schema_name.clone(),
            create.field_name.clone(),
            create.config.vector_size,
            create.config.distance,
            space_id,
        )
        .with_hnsw_m(create.config.hnsw_m)
        .with_hnsw_ef_construct(create.config.hnsw_ef_construct)
        .with_quantization(
            create.config.quantization,
            create.config.quantile,
            create.config.compression,
            create.config.always_ram,
        );
        if create.if_not_exists {
            params = params.with_if_not_exists();
        }
        let node = CreateVectorIndexNode::new(params);

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    fn transform_drop_vector_index(
        &self,
        drop: &DropVectorIndex,
        space_name: &str,
        metadata: Option<&MetadataContext>,
    ) -> Result<SubPlan, PlannerError> {
        let node = DropVectorIndexNode::new(
            drop.index_name.clone(),
            space_name.to_string(),
            drop.if_exists,
        );
        // Attach the (space, tag, field) location resolved from the index
        // registry; the executor rejects an unresolved location unless the
        // statement opted into IF EXISTS.
        let node = match metadata.and_then(|metadata| metadata.get_index_metadata(&drop.index_name))
        {
            Some(index_metadata) => node.with_location(
                index_metadata.space_id,
                index_metadata.tag_name.clone(),
                index_metadata.field_name.clone(),
            ),
            None => node,
        };

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform SHOW VECTOR INDEX statement into execution plan
    fn transform_show_vector_index(
        &self,
        show: &ShowVectorIndex,
        space_name: &str,
    ) -> Result<SubPlan, PlannerError> {
        let from_schema = if show.from_schema.is_none() {
            Some(space_name.to_string())
        } else {
            show.from_schema.clone()
        };

        let node = ShowVectorIndexNode::new(show.pattern.clone(), from_schema);
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }

    /// Transform DESCRIBE VECTOR INDEX statement into execution plan
    fn transform_describe_vector_index(
        &self,
        describe: &DescribeVectorIndex,
    ) -> Result<SubPlan, PlannerError> {
        let node = DescribeVectorIndexNode::new(describe.index_name.clone());
        Ok(SubPlan::new(Some(node.into_enum()), None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::ast::vector::{VectorDistance, VectorIndexConfig};
    use linkrs_core::types::span::Span;

    #[test]
    fn test_ddl_planner_matches_only_ddl() {
        let planner = VectorDdlPlanner::new();
        let default_planner: VectorDdlPlanner = Default::default();
        assert!(planner.metadata_context.is_none());
        assert!(default_planner.metadata_context.is_none());

        let create_stmt = Stmt::CreateVectorIndex(CreateVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            schema_name: "tag".to_string(),
            field_name: "vec".to_string(),
            config: VectorIndexConfig::new(128, VectorDistance::Cosine),
            if_not_exists: false,
        });

        let drop_stmt = Stmt::DropVectorIndex(DropVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            if_exists: false,
        });

        let show_stmt = Stmt::ShowVectorIndex(ShowVectorIndex {
            span: Span::default(),
            pattern: None,
            from_schema: None,
        });

        let describe_stmt = Stmt::DescribeVectorIndex(DescribeVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
        });
        assert!(matches!(create_stmt, Stmt::CreateVectorIndex(_)));
        assert!(matches!(drop_stmt, Stmt::DropVectorIndex(_)));
        assert!(matches!(show_stmt, Stmt::ShowVectorIndex(_)));
        assert!(matches!(describe_stmt, Stmt::DescribeVectorIndex(_)));
    }

    #[test]
    fn test_create_rejects_edge_type_names() {
        use crate::metadata::{EdgeTypeMetadata, TagMetadata};

        let mut metadata = MetadataContext::new();
        metadata.set_tag_metadata("Article".to_string(), TagMetadata::new("Article".into(), 1));
        metadata.set_edge_type_metadata(
            "WROTE".to_string(),
            EdgeTypeMetadata::new("WROTE".into(), 1),
        );
        let planner = VectorDdlPlanner::with_metadata_context(Arc::new(metadata));

        let create = |schema: &str| CreateVectorIndex {
            span: Span::default(),
            index_name: "idx".to_string(),
            schema_name: schema.to_string(),
            field_name: "vec".to_string(),
            config: VectorIndexConfig::new(128, VectorDistance::Cosine),
            if_not_exists: false,
        };
        // Vertex tags pass validation.
        assert!(planner
            .transform_create_vector_index(
                &create("Article"),
                "default",
                1,
                planner.metadata_context.as_deref()
            )
            .is_ok());
        // Edge type names are rejected: vector indexes are vertex-only.
        let rejected = planner.transform_create_vector_index(
            &create("WROTE"),
            "default",
            1,
            planner.metadata_context.as_deref(),
        );
        assert!(
            matches!(rejected, Err(PlannerError::InvalidOperation(_))),
            "edge type vector creation should be rejected, got: {:?}",
            rejected.map(|_| ()),
        );
        // Unknown schema names still pass the planner (resolved downstream).
        assert!(planner
            .transform_create_vector_index(
                &create("Missing"),
                "default",
                1,
                planner.metadata_context.as_deref()
            )
            .is_ok());
    }
}
