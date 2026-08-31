//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;

/// PatternApply node – Pattern matching application
///
/// Receive two inputs from the left and right sides. Determine whether the data on the left side matches the pattern on the right side based on the key columns.
/// Supports both forward matching (EXISTS) and reverse matching (NOT EXISTS).
///
/// The join keys are split per side: `hash_keys` are evaluated against the
/// left (outer) row layout, `probe_keys` against the right (subquery) row
/// layout. This mirrors the `SemiJoinNode` key convention so that
/// decorrelation is a direct passthrough.
#[derive(Debug, Clone)]
pub struct PatternApplyNode {
    id: i64,
    left_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    hash_keys: Vec<graphdb_core::types::ContextualExpression>,
    probe_keys: Vec<graphdb_core::types::ContextualExpression>,
    is_anti_predicate: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

impl PatternApplyNode {
    pub fn new(
        left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<graphdb_core::types::ContextualExpression>,
        probe_keys: Vec<graphdb_core::types::ContextualExpression>,
        is_anti_predicate: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = left_input.col_names().to_vec();
        let deps = vec![left_input.clone(), right_input.clone()];

        Ok(Self {
            id: -1,
            left_input: Box::new(left_input),
            right_input: Box::new(right_input),
            deps,
            hash_keys,
            probe_keys,
            is_anti_predicate,
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

    pub fn hash_keys(&self) -> &[graphdb_core::types::ContextualExpression] {
        &self.hash_keys
    }

    pub fn probe_keys(&self) -> &[graphdb_core::types::ContextualExpression] {
        &self.probe_keys
    }

    pub fn is_anti_predicate(&self) -> bool {
        self.is_anti_predicate
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn type_name(&self) -> &'static str {
        "PatternApply"
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

    pub fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(Self {
            id: self.id,
            left_input: self.left_input.clone(),
            right_input: self.right_input.clone(),
            deps: self.deps.clone(),
            hash_keys: self.hash_keys.clone(),
            probe_keys: self.probe_keys.clone(),
            is_anti_predicate: self.is_anti_predicate,
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(cloned)
    }
}

// Implement the PlanNode trait for PatternApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for PatternApplyNode {
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(self)
    }
}

// Implement the PlanNodeClonable trait for PatternApplyNode.
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for PatternApplyNode
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

// Implement the SingleInputNode trait for PatternApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for PatternApplyNode
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

impl MemoryEstimatable for PatternApplyNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<PatternApplyNode>();

        // Estimate hash_keys and probe_keys
        let keys_size = std::mem::size_of::<Vec<graphdb_core::types::ContextualExpression>>() * 2
            + (self.hash_keys.len() + self.probe_keys.len())
                * std::mem::size_of::<graphdb_core::types::ContextualExpression>();

        // Estimate is_anti_predicate bool
        let is_anti_size = std::mem::size_of::<bool>();

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

        base + keys_size
            + is_anti_size
            + col_names_size
            + output_var_size
            + left_right_size
            + deps_size
    }
}
