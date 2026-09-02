//! Planner registration mechanism
//! Implement static registration using type-safe enumerations to completely eliminate dynamic distribution.
//!
//! # Explanation of the reconstruction process
//!
//! This module has been completely restructured, and the old mechanism for matching SentenceKind strings has been removed.
//! Now, use the direct enumeration mode to match the planner created from the Stmt.

use std::sync::Arc;

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::context::PlanContext;
use crate::planning::plan::ExecutionPlan;
use crate::planning::plan::SubPlan;
use crate::QueryContext;

// Re-exported for planner implementations that still reference it.
pub use crate::binder::validation::ValidatedStatement;

use crate::planning::fulltext_planner::FulltextSearchPlanner;
use crate::planning::statements::ddl::maintain_planner::MaintainPlanner;
use crate::planning::statements::ddl::use_planner::UsePlanner;
use crate::planning::statements::ddl::user_management_planner::UserManagementPlanner;
use crate::planning::statements::dml::assignment_planner::AssignmentPlanner;
use crate::planning::statements::dml::copy_planner::CopyPlanner;
use crate::planning::statements::dml::create_planner::CreatePlanner;
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

/// Match function type
pub type MatchFunc = fn(&Stmt) -> bool;

///  Planner Features
///
/// # Design Principles
/// The `transform` method accepts an `Arc<QueryContext>` and a `&ValidatedStatement`.
/// The `match_planner` method receives an `&Stmt` object, which is used for matching and making judgments.
pub trait Planner: std::fmt::Debug {
    /// Translate the verified statement into a sub-plan.
    ///
    /// # Parameters
    /// `validated`: A verified statement that contains ValidationInfo and Ast.
    /// `qctx`: Query context
    fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError>;

    /// Check whether this planner can handle the given sentence.
    fn match_planner(&self, stmt: &Stmt) -> bool;

    /// Use the verified statements to complete the translation.
    fn transform_with_full_context(
        &mut self,
        qctx: Arc<QueryContext>,
        validated: &ValidatedStatement,
    ) -> Result<ExecutionPlan, PlannerError> {
        let sub_plan = self.transform(validated, qctx)?;
        let plan = ExecutionPlan::new(sub_plan.root().clone());

        // Note: Plan optimization is handled by QueryPipelineManager
        Ok(plan)
    }

    /// Translate a bound statement directly into a plan sub-tree.
    ///
    /// The unified [`PlanContext`] bundles all inputs needed during
    /// bound-statement planning (bound statement, query context, metadata,
    /// validated statement). The default implementation returns
    /// `UnsupportedOperation` so that planners which have not yet been
    /// migrated to the bound pipeline continue to work unchanged.
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
    Match(MatchStatementPlanner),
    Go(GoPlanner),
    Lookup(LookupPlanner),
    Path(PathPlanner),
    Subgraph(SubgraphPlanner),
    FetchVertices(FetchVerticesPlanner),
    FetchEdges(FetchEdgesPlanner),
    Maintain(MaintainPlanner),
    UserManagement(UserManagementPlanner),
    CreateData(CreatePlanner),
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
    #[cfg(feature = "vector")]
    VectorSearch(VectorSearchPlanner),
}

impl PlannerEnum {
    /// Create a planner directly from Arc<Stmt> (the recommended method).
    /// Use the enumeration pattern for matching to completely eliminate the need for string matching.
    pub fn from_stmt(stmt: &Arc<Stmt>) -> Option<Self> {
        match stmt.as_ref() {
            Stmt::Match(_) => Some(PlannerEnum::Match(MatchStatementPlanner::new())),
            Stmt::Go(_) => Some(PlannerEnum::Go(GoPlanner::new())),
            Stmt::Lookup(_) => Some(PlannerEnum::Lookup(LookupPlanner::new())),
            Stmt::FindPath(_) => Some(PlannerEnum::Path(PathPlanner::new())),
            Stmt::Subgraph(_) => Some(PlannerEnum::Subgraph(SubgraphPlanner::new())),
            Stmt::Fetch(fetch_stmt) => match &fetch_stmt.target {
                crate::parser::ast::FetchTarget::Vertices { .. } => {
                    Some(PlannerEnum::FetchVertices(FetchVerticesPlanner::new()))
                }
                crate::parser::ast::FetchTarget::Edges { .. } => {
                    Some(PlannerEnum::FetchEdges(FetchEdgesPlanner::new()))
                }
            },
            Stmt::Insert(_) => Some(PlannerEnum::Insert(InsertPlanner::new())),
            Stmt::Copy(_) => Some(PlannerEnum::Copy(CopyPlanner::new())),
            Stmt::Delete(_) => Some(PlannerEnum::Delete(DeletePlanner::new())),
            Stmt::Update(_) => Some(PlannerEnum::Update(UpdatePlanner::new())),
            Stmt::Remove(_) => Some(PlannerEnum::Remove(RemovePlanner::new())),
            Stmt::Set(_) => Some(PlannerEnum::Set(SetPlanner::new())),
            Stmt::Merge(_) => Some(PlannerEnum::Merge(MergePlanner::new())),
            Stmt::Assignment(_) => Some(PlannerEnum::Assignment(AssignmentPlanner::new())),
            Stmt::GroupBy(_) => Some(PlannerEnum::GroupBy(GroupByPlanner::new())),
            Stmt::Filter(_) => Some(PlannerEnum::Filter(FilterPlanner::new())),
            Stmt::Collect(_) => Some(PlannerEnum::Collect(CollectPlanner::new())),
            Stmt::SetOperation(_) => Some(PlannerEnum::SetOperation(SetOperationPlanner::new())),
            Stmt::Use(_) => Some(PlannerEnum::Use(UsePlanner::new())),
            Stmt::Unwind(_) => Some(PlannerEnum::Unwind(UnwindPlanner::new())),
            Stmt::With(_) => Some(PlannerEnum::With(WithPlanner::new())),
            Stmt::Return(_) => Some(PlannerEnum::Return(ReturnPlanner::new())),
            Stmt::AssignVariable(_) => {
                Some(PlannerEnum::AssignVariable(AssignVariablePlanner::new()))
            }
            Stmt::Yield(_) => Some(PlannerEnum::Yield(YieldPlanner::new())),
            Stmt::Pipe(_) => Some(PlannerEnum::Pipe(PipePlanner::new())),
            Stmt::Explain(_) => Some(PlannerEnum::Explain(ExplainPlanner::new())),
            Stmt::Profile(_) => Some(PlannerEnum::Explain(ExplainPlanner::new_profile())),
            // Full-text search statements
            Stmt::CreateFulltextIndex(_)
            | Stmt::DropFulltextIndex(_)
            | Stmt::AlterFulltextIndex(_)
            | Stmt::ShowFulltextIndex(_)
            | Stmt::DescribeFulltextIndex(_)
            | Stmt::Search(_)
            | Stmt::LookupFulltext(_)
            | Stmt::MatchFulltext(_) => {
                Some(PlannerEnum::FulltextSearch(FulltextSearchPlanner::new()))
            }
            #[cfg(feature = "vector")]
            Stmt::CreateVectorIndex(_)
            | Stmt::DropVectorIndex(_)
            | Stmt::SearchVector(_)
            | Stmt::LookupVector(_)
            | Stmt::MatchVector(_) => Some(PlannerEnum::VectorSearch(VectorSearchPlanner::new())),
            Stmt::Create(create_stmt) => match &create_stmt.target {
                crate::parser::ast::CreateTarget::Node { .. }
                | crate::parser::ast::CreateTarget::Edge { .. }
                | crate::parser::ast::CreateTarget::Path { .. } => {
                    Some(PlannerEnum::CreateData(CreatePlanner::new()))
                }
                _ => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            },
            Stmt::CreateUser(_)
            | Stmt::DropUser(_)
            | Stmt::AlterUser(_)
            | Stmt::ChangePassword(_)
            | Stmt::Grant(_)
            | Stmt::Revoke(_)
            | Stmt::DescribeUser(_)
            | Stmt::ShowUsers(_)
            | Stmt::ShowRoles(_) => Some(PlannerEnum::UserManagement(UserManagementPlanner::new())),
            Stmt::Drop(_)
            | Stmt::Show(_)
            | Stmt::Desc(_)
            | Stmt::Alter(_)
            | Stmt::ShowCreate(_)
            | Stmt::ShowSessions(_)
            | Stmt::ShowQueries(_)
            | Stmt::KillQuery(_)
            | Stmt::ShowConfigs(_)
            | Stmt::UpdateConfigs(_)
            | Stmt::ClearSpace(_)
            | Stmt::BeginTransaction(_)
            | Stmt::CommitTransaction(_)
            | Stmt::RollbackTransaction(_)
            | Stmt::Savepoint(_)
            | Stmt::ReleaseSavepoint(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            // The type of the following sentence does not currently support direct planning.
            _ => None,
        }
    }

    /// Create a planner from Arc<Ast>.
    /// This is the new recommendation method; the context of the expressions is defined within Ast.
    pub fn from_ast(ast: &Arc<crate::parser::ast::stmt::Ast>) -> Option<Self> {
        Self::from_stmt(&Arc::new(ast.stmt.clone()))
    }

    /// Convert the verified statement into an execution plan.
    pub fn transform(
        &mut self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> Result<SubPlan, PlannerError> {
        match self {
            PlannerEnum::Match(planner) => planner.transform(validated, qctx),
            PlannerEnum::Go(planner) => planner.transform(validated, qctx),
            PlannerEnum::Lookup(planner) => planner.transform(validated, qctx),
            PlannerEnum::Path(planner) => planner.transform(validated, qctx),
            PlannerEnum::Subgraph(planner) => planner.transform(validated, qctx),
            PlannerEnum::FetchVertices(planner) => planner.transform(validated, qctx),
            PlannerEnum::FetchEdges(planner) => planner.transform(validated, qctx),
            PlannerEnum::Maintain(planner) => planner.transform(validated, qctx),
            PlannerEnum::UserManagement(planner) => planner.transform(validated, qctx),
            PlannerEnum::CreateData(planner) => planner.transform(validated, qctx),
            PlannerEnum::Assignment(planner) => planner.transform(validated, qctx),
            PlannerEnum::Insert(planner) => planner.transform(validated, qctx),
            PlannerEnum::Copy(planner) => planner.transform(validated, qctx),
            PlannerEnum::Delete(planner) => planner.transform(validated, qctx),
            PlannerEnum::Update(planner) => planner.transform(validated, qctx),
            PlannerEnum::Remove(planner) => planner.transform(validated, qctx),
            PlannerEnum::Set(planner) => planner.transform(validated, qctx),
            PlannerEnum::Merge(planner) => planner.transform(validated, qctx),
            PlannerEnum::GroupBy(planner) => planner.transform(validated, qctx),
            PlannerEnum::Filter(planner) => planner.transform(validated, qctx),
            PlannerEnum::Collect(planner) => planner.transform(validated, qctx),
            PlannerEnum::SetOperation(planner) => planner.transform(validated, qctx),
            PlannerEnum::Use(planner) => planner.transform(validated, qctx),
            PlannerEnum::Unwind(planner) => planner.transform(validated, qctx),
            PlannerEnum::With(planner) => planner.transform(validated, qctx),
            PlannerEnum::Return(planner) => planner.transform(validated, qctx),
            PlannerEnum::AssignVariable(planner) => planner.transform(validated, qctx),
            PlannerEnum::Yield(planner) => planner.transform(validated, qctx),
            PlannerEnum::Pipe(planner) => planner.transform(validated, qctx),
            PlannerEnum::Explain(planner) => planner.transform(validated, qctx),
            PlannerEnum::FulltextSearch(planner) => planner.transform(validated, qctx),
            #[cfg(feature = "vector")]
            PlannerEnum::VectorSearch(planner) => planner.transform(validated, qctx),
        }
    }

    /// Create a planner from a BoundStatement.
    pub fn from_bound_statement(bound: &BoundStatement) -> Option<Self> {
        match bound {
            BoundStatement::Match(_) => Some(PlannerEnum::Match(MatchStatementPlanner::new())),
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
            BoundStatement::Create(c) => match &c.target {
                crate::binder::bound::BoundCreateTarget::Node { .. }
                | crate::binder::bound::BoundCreateTarget::Edge { .. }
                | crate::binder::bound::BoundCreateTarget::Path { .. } => {
                    Some(PlannerEnum::CreateData(CreatePlanner::new()))
                }
            },
            BoundStatement::Drop(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Alter(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Show(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::ShowCreate(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Desc(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::ClearSpace(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::CreateUser(_)
            | BoundStatement::DropUser(_)
            | BoundStatement::AlterUser(_) => {
                Some(PlannerEnum::UserManagement(UserManagementPlanner::new()))
            }
            BoundStatement::CreateFulltextIndex(_) => {
                Some(PlannerEnum::FulltextSearch(FulltextSearchPlanner::new()))
            }
            BoundStatement::CreateVectorIndex(_) => {
                #[cfg(feature = "vector")]
                {
                    Some(PlannerEnum::VectorSearch(VectorSearchPlanner::new()))
                }
                #[cfg(not(feature = "vector"))]
                {
                    None
                }
            }
            BoundStatement::Explain(_) => Some(PlannerEnum::Explain(ExplainPlanner::new())),
            BoundStatement::Profile(_) => Some(PlannerEnum::Explain(ExplainPlanner::new_profile())),
            BoundStatement::BeginTransaction(_)
            | BoundStatement::Commit(_)
            | BoundStatement::Rollback(_) => Some(PlannerEnum::Maintain(MaintainPlanner::new())),
            BoundStatement::Other(stmt) => Self::from_stmt(&Arc::new(*stmt.clone())),
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
            PlannerEnum::CreateData(_) => "CreateDataPlanner",
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
            #[cfg(feature = "vector")]
            PlannerEnum::VectorSearch(_) => "VectorSearchPlanner",
        }
    }

    /// Check whether there is a match.
    pub fn matches(&self, stmt: &Stmt) -> bool {
        match self {
            PlannerEnum::Match(planner) => planner.match_planner(stmt),
            PlannerEnum::Go(planner) => planner.match_planner(stmt),
            PlannerEnum::Lookup(planner) => planner.match_planner(stmt),
            PlannerEnum::Path(planner) => planner.match_planner(stmt),
            PlannerEnum::Subgraph(planner) => planner.match_planner(stmt),
            PlannerEnum::FetchVertices(planner) => planner.match_planner(stmt),
            PlannerEnum::FetchEdges(planner) => planner.match_planner(stmt),
            PlannerEnum::Maintain(planner) => planner.match_planner(stmt),
            PlannerEnum::UserManagement(planner) => planner.match_planner(stmt),
            PlannerEnum::CreateData(planner) => planner.match_planner(stmt),
            PlannerEnum::Assignment(planner) => planner.match_planner(stmt),
            PlannerEnum::Insert(planner) => planner.match_planner(stmt),
            PlannerEnum::Copy(planner) => planner.match_planner(stmt),
            PlannerEnum::Delete(planner) => planner.match_planner(stmt),
            PlannerEnum::Update(planner) => planner.match_planner(stmt),
            PlannerEnum::Remove(planner) => planner.match_planner(stmt),
            PlannerEnum::Set(planner) => planner.match_planner(stmt),
            PlannerEnum::Merge(planner) => planner.match_planner(stmt),
            PlannerEnum::GroupBy(planner) => planner.match_planner(stmt),
            PlannerEnum::Filter(planner) => planner.match_planner(stmt),
            PlannerEnum::Collect(planner) => planner.match_planner(stmt),
            PlannerEnum::SetOperation(planner) => planner.match_planner(stmt),
            PlannerEnum::Use(planner) => planner.match_planner(stmt),
            PlannerEnum::Unwind(planner) => planner.match_planner(stmt),
            PlannerEnum::With(planner) => planner.match_planner(stmt),
            PlannerEnum::Return(planner) => planner.match_planner(stmt),
            PlannerEnum::AssignVariable(planner) => planner.match_planner(stmt),
            PlannerEnum::Yield(planner) => planner.match_planner(stmt),
            PlannerEnum::Pipe(planner) => planner.match_planner(stmt),
            PlannerEnum::Explain(planner) => planner.match_planner(stmt),
            PlannerEnum::FulltextSearch(planner) => planner.match_planner(stmt),
            #[cfg(feature = "vector")]
            PlannerEnum::VectorSearch(planner) => planner.match_planner(stmt),
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
            PlannerEnum::CreateData(planner) => planner.plan_bound(ctx),
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
impl From<graphdb_core::error::DBError> for PlannerError {
    fn from(err: graphdb_core::error::DBError) -> Self {
        PlannerError::PlanGenerationFailed(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_planner_enum_from_stmt() {
        // Testing the creation of a planner from a Stmt
        let match_stmt = Stmt::Match(crate::parser::ast::MatchStmt {
            span: graphdb_core::types::Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let planner = PlannerEnum::from_stmt(&Arc::new(match_stmt));
        assert!(planner.is_some());
        assert_eq!(
            planner.expect("Planner should exist").name(),
            "MatchPlanner"
        );
    }

    #[test]
    fn test_planner_enum_matches() {
        let match_stmt = Stmt::Match(crate::parser::ast::MatchStmt {
            span: graphdb_core::types::Span::default(),
            patterns: vec![],
            where_clause: None,
            return_clause: None,
            order_by: None,
            limit: None,
            skip: None,
            optional: false,
            delete_clause: None,
        });

        let planner = PlannerEnum::Match(MatchStatementPlanner::new());
        assert!(planner.matches(&match_stmt));
    }
}
