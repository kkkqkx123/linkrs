//! System and session operation planning (transactions, maintenance, LOAD/CALL).

use std::sync::Arc;

use crate::binder::validation::ValidatedStatement;
use crate::parser::ast::stmt::{
    AttachDatabaseStmt, CommentOnStmt, DetachDatabaseStmt, ExportDatabaseStmt, ImportDatabaseStmt,
    InQueryCallStmt, LoadFromStmt, ScanSource, ShowConfigsStmt,
};
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode;
use crate::planning::plan::core::nodes::management::space_nodes::{
    AttachDatabaseNode, CheckpointNode, CommentOnNode, DetachDatabaseNode, ExportDatabaseNode,
    ImportDatabaseNode,
};
use crate::planning::plan::core::nodes::management::system_nodes::{
    InQueryCallNode, LoadFromNode, ShowConfigsNode, ShowQueriesNode, ShowSessionsNode,
};
use crate::planning::plan::core::{ClearSpaceNode, PlanNodeEnum, ShowStatsNode, ShowStatsType};
use crate::planning::plan::{
    BeginTransactionNode, CommitNode, ReleaseSavepointNode, RollbackNode, SavepointNode, SubPlan,
};
use crate::planning::planner::{PlannerEnum, PlannerError};
use crate::QueryContext;

pub(super) fn plan_clear_space(space_name: &str) -> PlanNodeEnum {
    let node = ClearSpaceNode::new(next_node_id(), space_name.to_string());
    PlanNodeEnum::SpaceManage(SpaceManageNode::Clear(node))
}

pub(super) fn plan_show_configs(stmt: &ShowConfigsStmt) -> PlanNodeEnum {
    let node = ShowConfigsNode::new(next_node_id(), stmt.module.clone());
    PlanNodeEnum::ShowConfigs(node)
}

pub(super) fn plan_show_queries() -> PlanNodeEnum {
    PlanNodeEnum::ShowQueries(ShowQueriesNode::new(next_node_id()))
}

pub(super) fn plan_show_sessions() -> PlanNodeEnum {
    PlanNodeEnum::ShowSessions(ShowSessionsNode::new(next_node_id()))
}

pub(super) fn plan_begin_transaction() -> PlanNodeEnum {
    PlanNodeEnum::BeginTransaction(BeginTransactionNode::new(next_node_id()))
}

pub(super) fn plan_commit() -> PlanNodeEnum {
    PlanNodeEnum::Commit(CommitNode::new(next_node_id()))
}

pub(super) fn plan_rollback(savepoint_name: Option<&String>) -> PlanNodeEnum {
    let mut node = RollbackNode::new(next_node_id());
    if let Some(savepoint) = savepoint_name {
        node = node.with_savepoint(savepoint.clone());
    }
    PlanNodeEnum::Rollback(node)
}

pub(super) fn plan_savepoint(name: &str) -> PlanNodeEnum {
    PlanNodeEnum::Savepoint(SavepointNode::new(next_node_id(), name.to_string()))
}

pub(super) fn plan_release_savepoint(name: &str) -> PlanNodeEnum {
    PlanNodeEnum::ReleaseSavepoint(ReleaseSavepointNode::new(next_node_id(), name.to_string()))
}

pub(super) fn plan_migrate() -> PlanNodeEnum {
    PlanNodeEnum::ShowStats(ShowStatsNode::new(next_node_id(), ShowStatsType::Storage))
}

pub(super) fn plan_comment_on(stmt: &CommentOnStmt) -> PlanNodeEnum {
    let node = CommentOnNode::new(next_node_id(), stmt.target.clone(), stmt.comment.clone());
    PlanNodeEnum::SpaceManage(SpaceManageNode::CommentOn(node))
}

pub(super) fn plan_checkpoint() -> PlanNodeEnum {
    PlanNodeEnum::SpaceManage(SpaceManageNode::Checkpoint(CheckpointNode::new(
        next_node_id(),
    )))
}

pub(super) fn plan_export_database(stmt: &ExportDatabaseStmt) -> PlanNodeEnum {
    let node = ExportDatabaseNode::new(next_node_id(), stmt.path.clone(), stmt.options.clone());
    PlanNodeEnum::SpaceManage(SpaceManageNode::ExportDatabase(node))
}

pub(super) fn plan_import_database(stmt: &ImportDatabaseStmt) -> PlanNodeEnum {
    let node = ImportDatabaseNode::new(next_node_id(), stmt.path.clone());
    PlanNodeEnum::SpaceManage(SpaceManageNode::ImportDatabase(node))
}

pub(super) fn plan_attach_database(stmt: &AttachDatabaseStmt) -> PlanNodeEnum {
    let node = AttachDatabaseNode::new(
        next_node_id(),
        stmt.path.clone(),
        stmt.alias.clone(),
        stmt.db_type.clone(),
        stmt.options.clone(),
    );
    PlanNodeEnum::SpaceManage(SpaceManageNode::AttachDatabase(node))
}

pub(super) fn plan_detach_database(stmt: &DetachDatabaseStmt) -> PlanNodeEnum {
    let node = DetachDatabaseNode::new(next_node_id(), stmt.alias.clone());
    PlanNodeEnum::SpaceManage(SpaceManageNode::DetachDatabase(node))
}

pub(super) fn plan_in_query_call(stmt: &InQueryCallStmt) -> PlanNodeEnum {
    let args_json = serde_json::to_string(
        &stmt
            .args
            .iter()
            .map(|a| a.to_expression_string())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| "[]".to_string());
    let yield_items: Vec<(String, String)> = stmt
        .yield_clause
        .as_ref()
        .map(|yc| {
            yc.items
                .iter()
                .map(|item| {
                    (
                        item.alias
                            .clone()
                            .unwrap_or_else(|| item.expression.to_expression_string()),
                        item.expression.to_expression_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let col_names: Vec<String> = yield_items.iter().map(|(a, _)| a.clone()).collect();
    let node = InQueryCallNode::new(
        next_node_id(),
        stmt.func_name.clone(),
        args_json,
        yield_items,
        col_names,
    );
    PlanNodeEnum::InQueryCall(node)
}

pub(super) fn plan_load_from(
    load_stmt: &LoadFromStmt,
    validated: &ValidatedStatement,
    qctx: Arc<QueryContext>,
) -> Result<SubPlan, PlannerError> {
    if let ScanSource::Query(inner) = &load_stmt.source {
        if !load_stmt.options.is_empty() {
            return Err(PlannerError::UnsupportedOperation(
                "LOAD FROM (query) does not accept file-only OPTIONS".to_string(),
            ));
        }
        if let Some(return_clause) = &load_stmt.return_clause {
            let star_only = return_clause.items.len() == 1
                && matches!(
                    &return_clause.items[0],
                    crate::parser::ast::stmt::ReturnItem::Expression {
                        expression,
                        alias: None,
                    } if expression.to_expression_string() == "*"
                );
            if !star_only {
                return Err(PlannerError::UnsupportedOperation(
                    "LOAD FROM (query) projects inside the inner query; outer RETURN must be absent or RETURN *".to_string(),
                ));
            }
        }
        let inner_planner = PlannerEnum::from_stmt_ref(inner.as_ref());
        let Some(mut planner) = inner_planner else {
            return Err(PlannerError::UnsupportedOperation(
                "LOAD FROM (query) inner statement is not plannable".to_string(),
            ));
        };
        let inner_ast = std::sync::Arc::new(crate::parser::ast::stmt::Ast::new(
            inner.as_ref().clone(),
            validated.ast.expr_context().clone(),
        ));
        let inner_validated = ValidatedStatement::new(inner_ast, validated.validation_info.clone());
        let mut sub_plan = planner.transform(&inner_validated, qctx)?;
        if !load_stmt.headers.is_empty() {
            let declared: Vec<String> = load_stmt.headers.iter().map(|h| h.name.clone()).collect();
            let current: Vec<String> = sub_plan
                .root()
                .as_ref()
                .map(|root| root.col_names().to_vec())
                .unwrap_or_default();
            if !current.is_empty() && current.len() != declared.len() {
                return Err(PlannerError::PlanGenerationFailed(format!(
                    "LOAD WITH HEADERS declares {} columns but query yields {}",
                    declared.len(),
                    current.len()
                )));
            }
            if current.is_empty() {
                return Err(PlannerError::UnsupportedOperation(
                    "LOAD WITH HEADERS over a query with unknown output columns is not supported; use AS aliases inside the inner query".to_string(),
                ));
            }
            let expr_ctx = validated.ast.expr_context().clone();
            let mut columns = Vec::with_capacity(current.len());
            for (old, new) in current.iter().zip(declared.iter()) {
                let meta = linkrs_core::types::expr::ExpressionMeta::new(
                    linkrs_core::Expression::Variable(old.clone()),
                );
                let id = expr_ctx.register_expression(meta);
                let expr = linkrs_core::types::ContextualExpression::new(id, expr_ctx.clone());
                columns.push(linkrs_core::YieldColumn::new(expr, new.clone()));
            }
            let inner_root = sub_plan.root().clone().ok_or_else(|| {
                PlannerError::PlanGenerationFailed(
                    "LOAD FROM (query) inner plan has no root".to_string(),
                )
            })?;
            let project =
                crate::planning::plan::core::nodes::ProjectNode::new(inner_root, columns)?;
            let tail = sub_plan.tail().clone();
            sub_plan = SubPlan::new(Some(PlanNodeEnum::Project(project)), tail);
        }
        return Ok(sub_plan);
    }
    let (source_kind, source_value, func_name, func_args_json) = match &load_stmt.source {
        ScanSource::File(path) => ("file".into(), path.clone(), None, None),
        ScanSource::Glob(pattern) => ("glob".into(), pattern.clone(), None, None),
        ScanSource::TableFunc { name, args } => {
            let args_json = serde_json::to_string(
                &args
                    .iter()
                    .map(|a| a.to_expression_string())
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".to_string());
            (
                "table_func".to_string(),
                String::new(),
                Some(name.clone()),
                Some(args_json),
            )
        }
        ScanSource::Query(_) => {
            return Err(PlannerError::UnsupportedOperation(
                "LOAD FROM (query) inner statement is not plannable".to_string(),
            ));
        }
    };
    let options: Vec<(String, String)> = load_stmt
        .options
        .iter()
        .map(|o| (o.key.clone(), o.value.clone()))
        .collect();
    let col_names: Vec<String> = load_stmt
        .return_clause
        .as_ref()
        .map(|rc| {
            rc.items
                .iter()
                .map(|item| match item {
                    crate::parser::ast::stmt::ReturnItem::Expression { expression, alias } => alias
                        .clone()
                        .unwrap_or_else(|| expression.to_expression_string()),
                })
                .collect()
        })
        .unwrap_or_default();
    let node = LoadFromNode::new(
        next_node_id(),
        source_kind,
        source_value,
        func_name,
        func_args_json,
        options,
        col_names,
        load_stmt.headers.iter().map(|h| h.name.clone()).collect(),
    );
    Ok(SubPlan::from_single_node(PlanNodeEnum::LoadFrom(node)))
}
