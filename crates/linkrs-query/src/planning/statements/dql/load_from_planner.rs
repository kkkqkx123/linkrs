//! `LOAD FROM` statement planner.
//!
//! Plans the three executable sources — file, glob and table function — into
//! a physical-only leaf [`LoadFromNode`]. The statement is a terminal source:
//! it produces the loaded rows and has no input relation.

use crate::binder::BoundStatement;
use crate::parser::ast::Stmt;
use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::planning::plan::core::nodes::management::LoadFromNode;
use crate::planning::plan::SubPlan;
use crate::planning::planner::{Planner, PlannerError};
use crate::planning::statements::dql::argument_literals::literal_arguments_json;

/// Planner for `LOAD FROM`.
#[derive(Debug, Clone, Default)]
pub struct LoadFromPlanner;

impl LoadFromPlanner {
    pub fn new() -> Self {
        Self
    }
}

impl Planner for LoadFromPlanner {
    fn plan_bound(
        &mut self,
        ctx: &crate::planning::context::PlanContext<'_>,
    ) -> Result<SubPlan, PlannerError> {
        let load = match ctx.bound {
            BoundStatement::Other(stmt) => match stmt.as_ref() {
                Stmt::LoadFrom(load) => load,
                _ => {
                    return Err(PlannerError::PlanGenerationFailed(
                        "statement does not contain LOAD FROM".to_string(),
                    ))
                }
            },
            _ => {
                return Err(PlannerError::PlanGenerationFailed(
                    "statement does not contain LOAD FROM".to_string(),
                ))
            }
        };

        let (source_kind, source_value, func_name, func_args_json) = match &load.source {
            crate::parser::ast::ScanSource::File(path) => ("file", path.clone(), None, None),
            crate::parser::ast::ScanSource::Glob(pattern) => ("glob", pattern.clone(), None, None),
            crate::parser::ast::ScanSource::TableFunc { name, args } => (
                "table_func",
                String::new(),
                Some(name.clone()),
                Some(literal_arguments_json(args, "LOAD FROM table function")?),
            ),
        };

        let options = load
            .options
            .iter()
            .map(|option| (option.key.clone(), option.value.clone()))
            .collect();
        let headers: Vec<String> = load.headers.iter().map(|h| h.name.clone()).collect();

        let node = LoadFromNode::new(
            next_node_id(),
            source_kind.to_string(),
            source_value,
            func_name,
            func_args_json,
            options,
            Vec::new(),
            headers,
        );

        Ok(SubPlan::new(Some(node.into_enum()), None))
    }
}
