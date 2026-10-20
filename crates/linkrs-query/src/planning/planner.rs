//! Planner registration mechanism
//! Implement static registration using type-safe enumerations to completely eliminate dynamic distribution.
//!
//! # Explanation of the reconstruction process
//!
//! This module has been completely restructured, and the old mechanism for matching SentenceKind strings has been removed.
//! Now, use the direct enumeration mode to match the planner created from the Stmt.

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::context::PlanContext;
use crate::planning::plan::SubPlan;

use crate::planning::fulltext_planner::FulltextSearchPlanner;
use crate::planning::statements::ddl::maintain_planner::MaintainPlanner;
use crate::planning::statements::ddl::use_planner::UsePlanner;
use crate::planning::statements::ddl::user_management_planner::UserManagementPlanner;
use crate::planning::statements::dml::assignment_planner::AssignmentPlanner;
use crate::planning::statements::dml::copy_planner::CopyPlanner;
use crate::planning::statements::dml::delete_planner::DeletePlanner;
use crate::planning::statements::dml::insert_planner::InsertPlanner;
use crate::planning::statements::dml::merge_planner::MergePlanner;
use crate::planning::statements::dml::remove_planner::RemovePlanner;
use crate::planning::statements::dml::set_planner::SetPlanner;
use crate::planning::statements::dml::update_planner::UpdatePlanner;
use crate::planning::statements::dql::assign_variable_planner::AssignVariablePlanner;
use crate::planning::statements::dql::collect_planner::CollectPlanner;
use crate::planning::statements::dql::explain_planner::ExplainPlanner;
use crate::planning::statements::dql::fetch_edges_planner::FetchEdgesPlanner;
use crate::planning::statements::dql::fetch_vertices_planner::FetchVerticesPlanner;
use crate::planning::statements::dql::filter_planner::FilterPlanner;
use crate::planning::statements::dql::go_planner::GoPlanner;
use crate::planning::statements::dql::group_by_planner::GroupByPlanner;
use crate::planning::statements::dql::lookup_planner::LookupPlanner;
use crate::planning::statements::dql::path_planner::PathPlanner;
use crate::planning::statements::dql::pipe_planner::PipePlanner;
use crate::planning::statements::dql::return_planner::ReturnPlanner;
use crate::planning::statements::dql::set_operation_planner::SetOperationPlanner;
use crate::planning::statements::dql::subgraph_planner::SubgraphPlanner;
use crate::planning::statements::dql::unwind_planner::UnwindPlanner;
use crate::planning::statements::dql::with_planner::WithPlanner;
use crate::planning::statements::dql::yield_planner::YieldPlanner;
use crate::planning::statements::match_statement_planner::MatchStatementPlanner;
#[cfg(feature = "vector")]
use crate::planning::vector_planner::VectorSearchPlanner;
use crate::planning::vector_ddl_planner::VectorDdlPlanner;

///  Planner Configuration
#[derive(Debug, Clone)]
pub struct PlannerConfig {
    pub max_plan_depth: usize,
    pub enable_parallel_planning: bool,
    pub enable_rewrite: bool,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            max_plan_depth: 100,
            enable_parallel_planning: false,
            enable_rewrite: true,
        }
    }
}

///  Planner Features
pub trait Planner: std::fmt::Debug {
    /// Translate a bound statement directly into a plan sub-tree.
    ///
    /// Sole entry point for statement planning.
    /// The unified [`PlanContext`] bundles all inputs needed during
    /// bound-statement planning (bound statement, query context, metadata).
    /// The default implementation returns `UnsupportedOperation`.
    fn plan_bound(&mut self, _ctx: &PlanContext<'_>) -> Result<SubPlan, PlannerError> {
        Err(PlannerError::UnsupportedOperation(
            "plan_bound not yet implemented for this planner".to_string(),
        ))
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

// ============================================================================
// Implementation of static registration – complete elimination of dynamic distribution
// ============================================================================

/// Planner Enumeration – Core for Static Distribution
/// Eliminate dynamic distribution completely and use compile-time polymorphism instead.
#[derive(Debug, Clone)]
pub enum PlannerEnum {
    Match(Box<MatchStatementPlanner>),
    Go(GoPlanner),
    Lookup(LookupPlanner),
    Path(PathPlanner),
    Subgraph(SubgraphPlanner),
    FetchVertices(FetchVerticesPlanner),
    FetchEdges(FetchEdgesPlanner),
    Maintain(MaintainPlanner),
    UserManagement(UserManagementPlanner),
    Assignment(AssignmentPlanner),
    Insert(InsertPlanner),
    Copy(CopyPlanner),
    Delete(DeletePlanner),
    Update(UpdatePlanner),
    Remove(RemovePlanner),
    Set(SetPlanner),
    Merge(MergePlanner),
    GroupBy(GroupByPlanner),
    Filter(FilterPlanner),
    Collect(CollectPlanner),
    SetOperation(SetOperationPlanner),
    Use(UsePlanner),
    Unwind(UnwindPlanner),
    With(WithPlanner),
    Return(ReturnPlanner),
    AssignVariable(AssignVariablePlanner),
    Yield(YieldPlanner),
    Pipe(PipePlanner),
    Explain(ExplainPlanner),
    FulltextSearch(FulltextSearchPlanner),
    VectorDdl(VectorDdlPlanner),
    #[cfg(feature = "vector")]
    VectorSearch(VectorSearchPlanner),
}

impl PlannerEnum {
    /// Create a planner from a BoundStatement.
    pub fn from_bound_statement(bound: &BoundStatement) -> Option<Self> {
        match bound {
            BoundStatement::Match(_) => Some(PlannerEnum::Match(Box::default())),
            BoundStatement::Go(_) => Some(PlannerEnum::Go(GoPlanner::new())),
            BoundStatement::Lookup(_) => Some(PlannerEnum::Lookup(LookupPlanner::new())),
            BoundStatement::FetchVertices(_) => {
                Some(PlannerEnum::FetchVertices(FetchVerticesPlanner::new()))
            }
            BoundStatement::FetchEdges(_) => {
                Some(PlannerEnum::FetchEdges(FetchEdgesPlanner::new()))
            }
            BoundStatement::FindPath(_) => Some(PlannerEnum::Path(PathPlanner::new())),
            BoundStatement::Subgraph(_) => Some(PlannerEnum::Subgraph(SubgraphPlanner::new())),
            BoundStatement::Return(_) => Some(PlannerEnum::Return(ReturnPlanner::new())),
            BoundStatement::With(_) => Some(PlannerEnum::With(WithPlanner::new())),
            BoundStatement::Use(_) => Some(PlannerEnum::Use(UsePlanner::new())),
            BoundStatement::Unwind(_) => Some(PlannerEnum::Unwind(UnwindPlanner::new())),
            BoundStatement::Pipe(_) => Some(PlannerEnum::Pipe(PipePlanner::new())),
            BoundStatement::SetOperation(_) => {
                Some(PlannerEnum::SetOperation(SetOperationPlanner::new()))
            }
            BoundStatement::GroupBy(_) => Some(PlannerEnum::GroupBy(GroupByPlanner::new())),
            BoundStatement::Filter(_) => Some(PlannerEnum::Filter(FilterPlanner::new())),
            BoundStatement::Yield(_) => Some(PlannerEnum::Yield(YieldPlanner::new())),
            BoundStatement::Collect(_) => Some(PlannerEnum::Collect(CollectPlanner::new())),
            BoundStatement::AssignVariable(_) => {
                Some(PlannerEnum::AssignVariable(AssignVariablePlanner::new()))
            }
            BoundStatement::Insert(_) => Some(PlannerEnum::Insert(InsertPlanner::new())),
            BoundStatement::Update(_) => Some(PlannerEnum::Update(UpdatePlanner::new())),
            BoundStatement::Delete(_) => Some(PlannerEnum::Delete(DeletePlanner::new())),
            BoundStatement::Merge(_) => Some(PlannerEnum::Merge(MergePlanner::new())),
            BoundStatement::Set(_) => Some(PlannerEnum::Set(SetPlanner::new())),
            BoundStatement::Remove(_) => Some(PlannerEnum::Remove(RemovePlanner::new())),
            BoundStatement::Copy(_) => Some(PlannerEnum::Copy(CopyPlanner::new())),
            BoundStatement::Drop(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Alter(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Show(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::ShowCreate(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Desc(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::ClearSpace(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::CreateUser(_)
            | BoundStatement::DropUser(_)
            | BoundStatement::AlterUser(_)
            | BoundStatement::ChangePassword(_)
            | BoundStatement::Grant(_)
            | BoundStatement::Revoke(_)
            | BoundStatement::DescribeUser(_)
            | BoundStatement::ShowUsers(_)
            | BoundStatement::ShowRoles(_) => {
                Some(PlannerEnum::UserManagement(UserManagementPlanner::new()))
            }
            BoundStatement::CreateFulltextIndex(_) => {
                Some(PlannerEnum::FulltextSearch(FulltextSearchPlanner::new()))
            }
            BoundStatement::CreateVectorIndex(_) => {
                Some(PlannerEnum::VectorDdl(VectorDdlPlanner::new()))
            }
            BoundStatement::Explain(_) => Some(PlannerEnum::Explain(ExplainPlanner::new())),
            BoundStatement::Profile(_) => Some(PlannerEnum::Explain(ExplainPlanner::new_profile())),
            BoundStatement::BeginTransaction(_)
            | BoundStatement::Commit(_)
            | BoundStatement::Rollback(_)
            | BoundStatement::Savepoint(_)
            | BoundStatement::ReleaseSavepoint(_) => {
                Some(PlannerEnum::Maintain(MaintainPlanner::new()))
            }
            BoundStatement::Assignment(_) => {
                Some(PlannerEnum::Assignment(AssignmentPlanner::new()))
            }
            BoundStatement::Other(stmt) => match stmt.as_ref() {
                Stmt::Create(_)
                | Stmt::CreateMacro(_)
                | Stmt::DropMacro(_)
                | Stmt::CreateType(_)
                | Stmt::DropType(_)
                | Stmt::AttachDatabase(_)
                | Stmt::DetachDatabase(_)
                | Stmt::PurgeExpired(_)
                | Stmt::Checkpoint(_)
                | Stmt::KillQuery(_)
                | Stmt::ExportDatabase(_)
                | Stmt::ImportDatabase(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
                Stmt::CreateVectorIndex(_)
                | Stmt::DropVectorIndex(_)
                | Stmt::ShowVectorIndex(_)
                | Stmt::DescribeVectorIndex(_) => {
                    Some(PlannerEnum::VectorDdl(VectorDdlPlanner::new()))
                }
                #[cfg(feature = "vector")]
                Stmt::SearchVector(_) | Stmt::LookupVector(_) | Stmt::MatchVector(_) => {
                    Some(PlannerEnum::VectorSearch(VectorSearchPlanner::new()))
                }
                _ => None,
            },
        }
    }

    /// Obtain the name of the planner.
    pub fn name(&self) -> &'static str {
        match self {
            PlannerEnum::Match(_) => "MatchPlanner",
            PlannerEnum::Go(_) => "GoPlanner",
            PlannerEnum::Lookup(_) => "LookupPlanner",
            PlannerEnum::Path(_) => "PathPlanner",
            PlannerEnum::Subgraph(_) => "SubgraphPlanner",
            PlannerEnum::FetchVertices(_) => "FetchVerticesPlanner",
            PlannerEnum::FetchEdges(_) => "FetchEdgesPlanner",
            PlannerEnum::Maintain(_) => "MaintainPlanner",
            PlannerEnum::UserManagement(_) => "UserManagementPlanner",
            PlannerEnum::Assignment(_) => "AssignmentPlanner",
            PlannerEnum::Insert(_) => "InsertPlanner",
            PlannerEnum::Copy(_) => "CopyPlanner",
            PlannerEnum::Delete(_) => "DeletePlanner",
            PlannerEnum::Update(_) => "UpdatePlanner",
            PlannerEnum::Remove(_) => "RemovePlanner",
            PlannerEnum::Set(_) => "SetPlanner",
            PlannerEnum::Merge(_) => "MergePlanner",
            PlannerEnum::GroupBy(_) => "GroupByPlanner",
            PlannerEnum::Filter(_) => "FilterPlanner",
            PlannerEnum::Collect(_) => "CollectPlanner",
            PlannerEnum::SetOperation(_) => "SetOperationPlanner",
            PlannerEnum::Use(_) => "UsePlanner",
            PlannerEnum::Unwind(_) => "UnwindPlanner",
            PlannerEnum::With(_) => "WithPlanner",
            PlannerEnum::Return(_) => "ReturnPlanner",
            PlannerEnum::AssignVariable(_) => "AssignVariablePlanner",
            PlannerEnum::Yield(_) => "YieldPlanner",
            PlannerEnum::Pipe(_) => "PipePlanner",
            PlannerEnum::Explain(_) => "ExplainPlanner",
            PlannerEnum::FulltextSearch(_) => "FulltextSearchPlanner",
            PlannerEnum::VectorDdl(_) => "VectorDdlPlanner",
            #[cfg(feature = "vector")]
            PlannerEnum::VectorSearch(_) => "VectorSearchPlanner",
        }
    }

    /// Plan from a BoundStatement, producing a SubPlan (PlanNodeEnum tree).
    pub fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        match self {
            PlannerEnum::Match(planner) => planner.plan_bound(ctx),
            PlannerEnum::Go(planner) => planner.plan_bound(ctx),
            PlannerEnum::Lookup(planner) => planner.plan_bound(ctx),
            PlannerEnum::Path(planner) => planner.plan_bound(ctx),
            PlannerEnum::Subgraph(planner) => planner.plan_bound(ctx),
            PlannerEnum::FetchVertices(planner) => planner.plan_bound(ctx),
            PlannerEnum::FetchEdges(planner) => planner.plan_bound(ctx),
            PlannerEnum::Maintain(planner) => planner.plan_bound(ctx),
            PlannerEnum::UserManagement(planner) => planner.plan_bound(ctx),
            PlannerEnum::Assignment(planner) => planner.plan_bound(ctx),
            PlannerEnum::Insert(planner) => planner.plan_bound(ctx),
            PlannerEnum::Copy(planner) => planner.plan_bound(ctx),
            PlannerEnum::Delete(planner) => planner.plan_bound(ctx),
            PlannerEnum::Update(planner) => planner.plan_bound(ctx),
            PlannerEnum::Remove(planner) => planner.plan_bound(ctx),
            PlannerEnum::Set(planner) => planner.plan_bound(ctx),
            PlannerEnum::Merge(planner) => planner.plan_bound(ctx),
            PlannerEnum::GroupBy(planner) => planner.plan_bound(ctx),
            PlannerEnum::Filter(planner) => planner.plan_bound(ctx),
            PlannerEnum::Collect(planner) => planner.plan_bound(ctx),
            PlannerEnum::SetOperation(planner) => planner.plan_bound(ctx),
            PlannerEnum::Use(planner) => planner.plan_bound(ctx),
            PlannerEnum::Unwind(planner) => planner.plan_bound(ctx),
            PlannerEnum::With(planner) => planner.plan_bound(ctx),
            PlannerEnum::Return(planner) => planner.plan_bound(ctx),
            PlannerEnum::AssignVariable(planner) => planner.plan_bound(ctx),
            PlannerEnum::Yield(planner) => planner.plan_bound(ctx),
            PlannerEnum::Pipe(planner) => planner.plan_bound(ctx),
            PlannerEnum::Explain(planner) => planner.plan_bound(ctx),
            PlannerEnum::FulltextSearch(planner) => planner.plan_bound(ctx),
            PlannerEnum::VectorDdl(planner) => planner.plan_bound(ctx),
            #[cfg(feature = "vector")]
            PlannerEnum::VectorSearch(planner) => planner.plan_bound(ctx),
        }
    }
}

/// Error handling macros
#[macro_export]
macro_rules! ng_return_if_error {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(e) => return Err(e.into()),
        }
    };
}

/// Error handling macro variants
#[macro_export]
macro_rules! ng_ok_or_err {
    ($expr:expr, $msg:expr) => {
        match $expr {
            Ok(val) => val,
            Err(_) => return Err(PlannerError::PlanGenerationFailed($msg.to_string())),
        }
    };
}

/// Planner error type
#[derive(Debug, thiserror::Error)]
pub enum PlannerError {
    #[error("No suitable planner found: {0}")]
    NoSuitablePlanner(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Plan generation failed: {0}")]
    PlanGenerationFailed(String),

    #[error("Join operation failed: {0}")]
    JoinFailed(String),

    #[error("Invalid AST context: {0}")]
    InvalidAstContext(String),

    #[error("Missing input: {0}")]
    MissingInput(String),

    #[error("Missing variable: {0}")]
    MissingVariable(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Unsupported vector filter: {0}")]
    UnsupportedVectorFilter(String),

    #[error("Tag not found: {0}")]
    TagNotFound(String),

    #[error("Edge type not found: {0}")]
    EdgeTypeNotFound(String),

    #[error("Metadata version mismatch: expected {expected}, got {actual}")]
    MetadataVersionMismatch { expected: u64, actual: u64 },

    #[error("Invalid metadata: {0}")]
    InvalidMetadata(String),
}

// Implement the From conversion for the DBError class.
impl From<linkrs_core::error::DBError> for PlannerError {
    fn from(err: linkrs_core::error::DBError) -> Self {
        PlannerError::PlanGenerationFailed(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_enum_from_bound() {
        use crate::binder::bound::BoundUse;

        let bound = BoundStatement::Use(BoundUse {
            space: "test".to_string(),
        });
        let planner = PlannerEnum::from_bound_statement(&bound);
        assert!(planner.is_some());
        assert_eq!(planner.expect("Planner should exist").name(), "UsePlanner");
    }

    #[test]
    fn test_planner_enum_bound_routing() {
        use crate::binder::bound::BoundUse;

        let bound = BoundStatement::Use(BoundUse {
            space: "test".to_string(),
        });
        let planner = PlannerEnum::from_bound_statement(&bound).expect("planner must exist");
        assert_eq!(planner.name(), "UsePlanner");
    }

    #[test]
    fn test_vector_ddl_routing_is_feature_independent() {
        use crate::parser::ast::vector::{DescribeVectorIndex, DropVectorIndex, ShowVectorIndex};
        use linkrs_core::types::Span;

        let stmts = [
            Stmt::DropVectorIndex(DropVectorIndex {
                span: Span::default(),
                index_name: "idx".to_string(),
                if_exists: false,
            }),
            Stmt::ShowVectorIndex(ShowVectorIndex {
                span: Span::default(),
                pattern: None,
                from_schema: None,
            }),
            Stmt::DescribeVectorIndex(DescribeVectorIndex {
                span: Span::default(),
                index_name: "idx".to_string(),
            }),
        ];
        for stmt in &stmts {
            let bound = BoundStatement::Other(Box::new(stmt.clone()));
            let planner = PlannerEnum::from_bound_statement(&bound);
            assert!(
                matches!(planner, Some(PlannerEnum::VectorDdl(_))),
                "vector DDL must route to VectorDdl in every feature combination, got: {:?}",
                planner.map(|p| p.name()),
            );
        }
    }

    #[cfg(feature = "vector")]
    #[test]
    fn test_vector_search_routing() {
        use crate::parser::ast::vector::{
            SearchVectorStatement, VectorQueryExpr, VectorQueryType,
        };
        use linkrs_core::types::Span;

        let stmt = Stmt::SearchVector(SearchVectorStatement {
            span: Span::default(),
            index_name: "idx".to_string(),
            query: VectorQueryExpr {
                span: Span::default(),
                query_type: VectorQueryType::Vector,
                query_data: "[0.1, 0.2]".to_string(),
            },
            threshold: None,
            where_clause: None,
            order_clause: None,
            limit: None,
            skip: None,
            yield_clause: None,
        });
        let bound = BoundStatement::Other(Box::new(stmt));
        let planner = PlannerEnum::from_bound_statement(&bound);
        assert!(
            matches!(planner, Some(PlannerEnum::VectorSearch(_))),
            "vector search must route to VectorSearch when the feature is on"
        );
    }
}
