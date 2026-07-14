use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::ExecutionContext;
use crate::query::executor::build_error::PlanBuildError;
use crate::query::executor::streaming::operators::spec::FulltextSpec;
use crate::query::executor::streaming::plan::node::PhysicalNode;
use crate::query::parser::ast::fulltext::FulltextQueryExpr;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

pub fn build_fulltext_node(
    node: &PlanNodeEnum,
    context: &ExecutionContext,
) -> Result<PhysicalNode, PlanBuildError> {
    match node {
        PlanNodeEnum::FulltextManage(manage_node) => Ok(super::build_leaf_command(
            node.id(),
            FulltextSpec::FulltextManage {
                space_name: context.space_name.clone().unwrap_or_default(),
                command: manage_node.clone(),
            },
            PhysicalNode::Fulltext,
        )),

        PlanNodeEnum::FulltextSearch(search_node) => {
            let query_str = fulltext_query_to_string(&search_node.query);
            let space_id = context.current_space_id().unwrap_or(0);
            Ok(super::build_leaf_command(
                node.id(),
                FulltextSpec::FulltextSearch {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id,
                    index_name: search_node.index_name.clone(),
                    search_query: query_str,
                    tag_name: search_node.tag_name.clone(),
                    field_name: search_node.field_name.clone(),
                },
                PhysicalNode::Fulltext,
            ))
        }

        PlanNodeEnum::FulltextLookup(lookup_node) => {
            let space_id = context.current_space_id().unwrap_or(0);
            Ok(super::build_leaf_command(
                node.id(),
                FulltextSpec::FulltextLookup {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    space_id,
                    index_name: lookup_node.index_name.clone(),
                    search_query: lookup_node.query.clone(),
                    tag_name: lookup_node.tag_name.clone(),
                    field_name: lookup_node.field_name.clone(),
                },
                PhysicalNode::Fulltext,
            ))
        }

        PlanNodeEnum::MatchFulltext(match_node) => {
            let condition_str = fulltext_match_to_string(&match_node.fulltext_condition);
            Ok(super::build_leaf_command(
                node.id(),
                FulltextSpec::MatchFulltext {
                    space_name: context.space_name.clone().unwrap_or_default(),
                    match_expr: Expression::Literal(Value::String(condition_str)),
                    match_field: Some(match_node.field_name.clone()),
                    tag_name: match_node.tag_name.clone(),
                    field_name: match_node.field_name.clone(),
                },
                PhysicalNode::Fulltext,
            ))
        }

        _ => Err(PlanBuildError::unsupported(
            node.name(),
            node.id(),
            format!(
                "operator_plan_builder::fulltext does not handle node type: {}",
                node.name()
            ),
        )),
    }
}

fn fulltext_query_to_string(expr: &FulltextQueryExpr) -> String {
    match expr {
        FulltextQueryExpr::Simple(text) => text.clone(),
        FulltextQueryExpr::Field(field, text) => format!("{}:{}", field, text),
        FulltextQueryExpr::Phrase(text) => format!("\"{}\"", text),
        FulltextQueryExpr::Prefix(text) => format!("{}*", text),
        FulltextQueryExpr::Fuzzy(text, distance) => {
            if let Some(d) = distance {
                format!("{}~{}", text, d)
            } else {
                format!("{}~", text)
            }
        }
        FulltextQueryExpr::Wildcard(text) => text.clone(),
        FulltextQueryExpr::Boolean {
            must,
            should,
            must_not,
        } => {
            let mut parts = Vec::new();
            for e in must {
                parts.push(format!("+({})", fulltext_query_to_string(e)));
            }
            for e in should {
                parts.push(format!("({})", fulltext_query_to_string(e)));
            }
            for e in must_not {
                parts.push(format!("-({})", fulltext_query_to_string(e)));
            }
            parts.join(" ")
        }
        FulltextQueryExpr::MultiField(fields) => fields
            .iter()
            .map(|(f, t)| format!("{}:{}", f, t))
            .collect::<Vec<_>>()
            .join(" OR "),
        FulltextQueryExpr::Range {
            field,
            lower,
            upper,
            include_lower,
            include_upper,
        } => {
            let lower_bound = if *include_lower { "[" } else { "{" };
            let upper_bound = if *include_upper { "]" } else { "}" };
            let lower_val = lower.as_deref().unwrap_or("*");
            let upper_val = upper.as_deref().unwrap_or("*");
            format!(
                "{}:{}{} TO {}{}",
                field, lower_bound, lower_val, upper_val, upper_bound
            )
        }
    }
}

fn fulltext_match_to_string(
    condition: &crate::query::parser::ast::fulltext::FulltextMatchCondition,
) -> String {
    format!("{}:{}", condition.field, condition.query)
}
