//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;

/// RollUpApply node – Grouped aggregation and data collection
///
/// Receive two inputs from the left and right. Group the data from the right according to the comparison column and collect it in a list.
/// Return the corresponding aggregate results for each row on the left side.
#[derive(Debug, Clone)]
pub struct RollUpApplyNode {
    id: i64,
    left_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    left_input_var: Option<String>,
    right_input_var: Option<String>,
    compare_cols: Vec<String>,
    collect_col: Option<String>,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

impl RollUpApplyNode {
    pub fn new(
        left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        compare_cols: Vec<String>,
        collect_col: Option<String>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = left_input.col_names().to_vec();
        let deps = vec![left_input.clone(), right_input.clone()];

        Ok(Self {
            id: -1,
            left_input: Box::new(left_input),
            right_input: Box::new(right_input),
            deps,
            left_input_var: None,
            right_input_var: None,
            compare_cols,
            collect_col,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn left_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    pub fn right_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.right_input
    }

    pub fn left_input_var(&self) -> Option<&String> {
        self.left_input_var.as_ref()
    }

    pub fn right_input_var(&self) -> Option<&String> {
        self.right_input_var.as_ref()
    }

    pub fn compare_cols(&self) -> &[String] {
        &self.compare_cols
    }

    pub fn collect_col(&self) -> Option<&String> {
        self.collect_col.as_ref()
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn type_name(&self) -> &'static str {
        "RollUpApply"
    }

    pub fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
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
        *self.left_input = dep.clone();
        self.deps.clear();
        self.deps.push(dep);
    }

    pub fn remove_dependency(&mut self, _id: i64) -> bool {
        false
    }

    pub fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    pub fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }

    pub fn set_left_input_var(&mut self, var: String) {
        self.left_input_var = Some(var);
    }

    pub fn set_right_input_var(&mut self, var: String) {
        self.right_input_var = Some(var);
    }

    pub fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(Self {
            id: self.id,
            left_input: self.left_input.clone(),
            right_input: self.right_input.clone(),
            deps: self.deps.clone(),
            left_input_var: self.left_input_var.clone(),
            right_input_var: self.right_input_var.clone(),
            compare_cols: self.compare_cols.clone(),
            collect_col: self.collect_col.clone(),
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        })
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(cloned)
    }
}

// Implement the PlanNode trait for RollUpApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for RollUpApplyNode {
    fn id(&self) -> i64 {
        self.id()
    }

    fn name(&self) -> &'static str {
        self.type_name()
    }

    fn category(&self) -> PlanNodeCategory {
        PlanNodeCategory::DataProcessing
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

    fn into_enum(self) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(self)
    }
}

// Implement the PlanNodeClonable trait for RollUpApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for RollUpApplyNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

// Implement the SingleInputNode trait for RollUpApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for RollUpApplyNode
{
    fn input(&self) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    fn input_mut(
        &mut self,
    ) -> &mut crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.left_input
    }

    fn set_input(
        &mut self,
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.left_input = input.clone();
        self.deps.clear();
        self.deps.push(input);
    }
}

impl MemoryEstimatable for RollUpApplyNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<RollUpApplyNode>();

        // Estimate left_input_var and right_input_var Option<String>
        // Uses capacity() to reflect actual heap allocation
        let input_var_size = std::mem::size_of::<Option<String>>() * 2
            + self
                .left_input_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0)
            + self
                .right_input_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        // Estimate compare_cols Vec<String>
        let compare_cols_size = std::mem::size_of::<Vec<String>>()
            + self
                .compare_cols
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        // Estimate collect_col Option<String>
        let collect_col_size = std::mem::size_of::<Option<String>>()
            + self
                .collect_col
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        // Estimate col_names
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

        // Estimate left and right Box<PlanNodeEnum>
        let left_right_size = std::mem::size_of::<
            Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >() * 2;

        // Estimate deps Vec<PlanNodeEnum>
        let deps_size = std::mem::size_of::<
            Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + input_var_size
            + compare_cols_size
            + collect_col_size
            + col_names_size
            + output_var_size
            + left_right_size
            + deps_size
    }
}
