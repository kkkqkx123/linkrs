use crate::executor::base::ExecutionContext;
use crate::executor::build_error::PlanBuildError;
use crate::executor::streaming::operators::spec::{
    FulltextManageCommand, FulltextSpec
};

use linkrs_core::types::expr::Expression;

pub(crate) fn fulltext_manage_to_command(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
) -> FulltextManageCommand {
    use crate::executor::streaming::operators::spec::FulltextManageCommand;
    use crate::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode::*;
    match node {
        Create(n) => FulltextManageCommand::Create {
            index_name: n.index_name.clone(),
            schema_name: n.schema_name.clone(),
            fields: n.fields.clone(),
            engine_type: n.engine_type,
            options: n.options.clone(),
            if_not_exists: n.if_not_exists,
            space_id: n.space_id,
        },
        Drop(n) => FulltextManageCommand::Drop {
            index_name: n.index_name.clone(),
            if_exists: n.if_exists,
        },
        Alter(n) => FulltextManageCommand::Alter {
            index_name: n.index_name.clone(),
            actions: n.actions.clone(),
        },
        Show(n) => FulltextManageCommand::Show {
            pattern: n.pattern.clone(),
            from_schema: n.from_schema.clone(),
        },
        Describe(n) => FulltextManageCommand::Describe {
            index_name: n.index_name.clone(),
        },
    }
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_fulltext_manage_spec(
    node: &crate::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextManage {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        command: fulltext_manage_to_command(node),
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_fulltext_search_spec(
    node: &crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    validate_fulltext_query_fields(&node.query, &node.field_name)?;
    Ok(FulltextSpec::FulltextSearch {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        structured_query: node.query.clone(),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        limit: node.limit,
        ryw_config: exec_ctx.ryw_config,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_fulltext_lookup_spec(
    node: &crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::FulltextLookup {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: exec_ctx.current_space_id().unwrap_or(0),
        index_name: node.index_name.clone(),
        structured_query: crate::parser::ast::fulltext::FulltextQueryExpr::Simple(
            node.query.clone(),
        ),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        limit: node.limit,
        ryw_config: exec_ctx.ryw_config,
    })
}

pub(in crate::executor::streaming::plan::arena_builder) fn build_match_fulltext_spec(
    node: &crate::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode,
    exec_ctx: &ExecutionContext,
) -> Result<FulltextSpec, PlanBuildError> {
    Ok(FulltextSpec::MatchFulltext {
        space_name: exec_ctx.space_name.clone().unwrap_or_default(),
        space_id: node.space_id,
        match_expr: Expression::Literal(linkrs_core::Value::string(format!(
            "{}:{}",
            node.fulltext_condition.field, node.fulltext_condition.query
        ))),
        match_field: Some(node.field_name.clone()),
        structured_query: crate::parser::ast::fulltext::FulltextQueryExpr::Simple(
            node.fulltext_condition.query.clone(),
        ),
        tag_name: node.tag_name.clone(),
        field_name: node.field_name.clone(),
        limit: None,
        ryw_config: exec_ctx.ryw_config,
    })
}

// ── Vector spec builders ──────────────────────────────────────────────────────

pub(crate) fn validate_fulltext_query_fields(
    expr: &crate::parser::ast::fulltext::FulltextQueryExpr,
    expected_field: &str,
) -> Result<(), PlanBuildError> {
    use crate::parser::ast::fulltext::FulltextQueryExpr;
    match expr {
        FulltextQueryExpr::Simple(_)
        | FulltextQueryExpr::Phrase(_)
        | FulltextQueryExpr::Prefix(_)
        | FulltextQueryExpr::Fuzzy(_, _)
        | FulltextQueryExpr::Wildcard(_) => Ok(()),
        FulltextQueryExpr::Boolean {
            must,
            should,
            must_not,
        } => {
            for q in must.iter().chain(should.iter()).chain(must_not.iter()) {
                validate_fulltext_query_fields(q, expected_field)?;
            }
            Ok(())
        }
        FulltextQueryExpr::Range {
            field,
            lower: _,
            upper: _,
            include_lower: _,
            include_upper: _,
        } => {
            if !field.is_empty() && field != expected_field {
                return Err(PlanBuildError::capability(
                    "fulltext-field-mismatch",
                    format!(
                        "Range query targets field '{}' but index serves field '{}'",
                        field, expected_field
                    ),
                ));
            }
            Ok(())
        }
        FulltextQueryExpr::Field(field, _) => {
            if field != expected_field {
                return Err(PlanBuildError::capability(
                    "fulltext-field-mismatch",
                    format!(
                        "Query targets field '{}' but index serves field '{}'",
                        field, expected_field
                    ),
                ));
            }
            Ok(())
        }
        FulltextQueryExpr::MultiField(fields) => {
            for (field, _) in fields {
                if field != expected_field {
                    return Err(PlanBuildError::capability(
                        "fulltext-field-mismatch",
                        format!(
                            "Multi-field query targets field '{}' but index serves field '{}'",
                            field, expected_field
                        ),
                    ));
                }
            }
            Ok(())
        }
    }
}

#[cfg(test)]
