//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

/// Remove a node: Delete an attribute or a tag.
///
/// Attributes and labels used for deleting vertices or edges
#[derive(Debug, Clone)]
pub struct RemoveNode {
    id: i64,
    input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    remove_items: Vec<(String, ContextualExpression)>,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl RemoveNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        remove_items: Vec<(String, ContextualExpression)>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Box::new(input.clone()),
            deps: vec![input],
            remove_items,
            output_var: None,
            col_names,
        })
    }

    pub fn remove_items(&self) -> &[(String, ContextualExpression)] {
        &self.remove_items
    }

    pub fn dependencies(
        &self,
    ) -> &[crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    pub fn add_dependency(
        &mut self,
        dep: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.input = dep.clone();
        self.deps.clear();
        self.deps.push(dep);
    }

    pub fn remove_dependency(&mut self, _id: i64) -> bool {
        false
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode for RemoveNode {
    fn input(&self) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.input
    }

    fn input_mut(
        &mut self,
    ) -> &mut crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.input
    }

    fn set_input(
        &mut self,
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.input = input;
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for RemoveNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "RemoveNode"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataProcessing
    }

    fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    fn col_names(&self) -> &[String] {
        &self.col_names
    }

    fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Remove(self)
    }
}

impl MemoryEstimatable for RemoveNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<RemoveNode>();

        // Estimate remove_items Vec<(String, ContextualExpression)>
        // Note: This is a conservative estimate, actual String capacity may vary
        let remove_items_size = std::mem::size_of::<Vec<(String, ContextualExpression)>>()
            + self.remove_items.len()
                * (std::mem::size_of::<String>() + std::mem::size_of::<ContextualExpression>());

        // Estimate col_names
        // Uses capacity() to reflect actual heap allocation
        let col_names_size = std::mem::size_of::<Vec<String>>()
            + self
                .col_names
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        // Estimate output_var
        let output_var_size = std::mem::size_of::<Option<String>>()
            + self
                .output_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        // Estimate input Box<PlanNodeEnum>
        let input_size = std::mem::size_of::<
            Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        // Estimate deps Vec<PlanNodeEnum>
        let deps_size = std::mem::size_of::<
            Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + remove_items_size + col_names_size + output_var_size + input_size + deps_size
    }
}
