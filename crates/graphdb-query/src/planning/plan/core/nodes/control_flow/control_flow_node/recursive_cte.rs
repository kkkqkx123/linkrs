use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, PlanNodeClonable};

/// Recursive-CTE fixpoint node.
///
/// Runs `anchor` once, then repeatedly runs `step` (whose CTE-labeled scans
/// read the working table) until no new rows appear or `max_iterations` is
/// reached. `step` is `None` for a non-recursive CTE (plain inline view).
/// V1 supports single-column anchors; the planner rejects wider shapes.
#[derive(Debug)]
pub struct RecursiveCteNode {
    id: i64,
    cte_name: String,
    anchor: Box<PlanNodeEnum>,
    step: Option<Box<PlanNodeEnum>>,
    max_iterations: u64,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

impl Clone for RecursiveCteNode {
    fn clone(&self) -> Self {
        RecursiveCteNode {
            id: self.id,
            cte_name: self.cte_name.clone(),
            anchor: self.anchor.clone(),
            step: self.step.clone(),
            max_iterations: self.max_iterations,
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        }
    }
}

impl RecursiveCteNode {
    pub fn new(
        id: i64,
        cte_name: String,
        anchor: PlanNodeEnum,
        step: Option<PlanNodeEnum>,
        max_iterations: u64,
    ) -> Self {
        Self {
            id,
            cte_name,
            anchor: Box::new(anchor),
            step: step.map(Box::new),
            max_iterations,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn type_name(&self) -> &'static str {
        "RecursiveCte"
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn cte_name(&self) -> &str {
        &self.cte_name
    }

    pub fn anchor(&self) -> &PlanNodeEnum {
        &self.anchor
    }

    pub fn anchor_mut(&mut self) -> &mut PlanNodeEnum {
        &mut self.anchor
    }

    pub fn set_anchor(&mut self, anchor: PlanNodeEnum) {
        *self.anchor = anchor;
    }

    pub fn step(&self) -> Option<&PlanNodeEnum> {
        self.step.as_deref()
    }

    pub fn step_mut(&mut self) -> Option<&mut PlanNodeEnum> {
        self.step.as_deref_mut()
    }

    pub fn set_step(&mut self, step: PlanNodeEnum) {
        self.step = Some(Box::new(step));
    }

    pub fn max_iterations(&self) -> u64 {
        self.max_iterations
    }

    pub fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    pub fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    pub fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }

    pub fn set_column_types(&mut self, types: Vec<graphdb_core::DataType>) {
        self.column_types = types;
    }

    pub fn column_types(&self) -> &[graphdb_core::DataType] {
        &self.column_types
    }

    pub fn clone_plan_node(&self) -> PlanNodeEnum {
        PlanNodeEnum::RecursiveCte(self.clone())
    }

    pub fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        PlanNodeEnum::RecursiveCte(cloned)
    }
}

impl PlanNode for RecursiveCteNode {
    fn id(&self) -> i64 {
        self.id()
    }

    fn name(&self) -> &'static str {
        "RecursiveCte"
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::ControlFlow
    }

    fn output_var(&self) -> Option<&str> {
        self.output_var()
    }

    fn col_names(&self) -> &[String] {
        self.col_names()
    }

    fn set_output_var(&mut self, var: String) {
        self.set_output_var(var);
    }

    fn set_col_names(&mut self, names: Vec<String>) {
        self.set_col_names(names);
    }

    fn into_enum(self) -> PlanNodeEnum {
        PlanNodeEnum::RecursiveCte(self)
    }
}

impl PlanNodeClonable for RecursiveCteNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

impl MemoryEstimatable for RecursiveCteNode {
    fn estimate_memory(&self) -> usize {
        // Child plans are boxed behind PlanNodeEnum (no memory estimation
        // dispatch); account for the fixed enum footprint only.
        std::mem::size_of::<RecursiveCteNode>()
            + std::mem::size_of::<PlanNodeEnum>()
            + self.cte_name.capacity()
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}
