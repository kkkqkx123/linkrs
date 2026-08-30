//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::define_plan_node_with_deps;
use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::statements::clauses::exists_planner::PlannedSubquery;
use graphdb_core::types::expr::contextual::ContextualExpression;

/// CorrelatedApply node – per-row correlated subquery re-execution
///
/// The left input is the outer plan; the right input is a self-contained
/// subquery plan rooted at an `Argument` source. For each left row the right
/// subtree is re-executed with the outer row bound as the correlation frame;
/// the existence of any matching row decides whether the left row passes
/// through (semi) or is rejected (anti).
///
/// The input contract is unary: only the left input participates in the
/// external fragment graph, the right subtree is rebuilt per row at runtime.
#[derive(Debug, Clone)]
pub struct CorrelatedApplyNode {
    id: i64,
    left_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    is_anti_predicate: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

impl CorrelatedApplyNode {
    pub fn new(
        left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        is_anti_predicate: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = left_input.col_names().to_vec();
        let deps = vec![left_input.clone(), right_input.clone()];

        Ok(Self {
            id: -1,
            left_input: Box::new(left_input),
            right_input: Box::new(right_input),
            deps,
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

    pub fn is_anti_predicate(&self) -> bool {
        self.is_anti_predicate
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn type_name(&self) -> &'static str {
        "CorrelatedApply"
    }

    pub fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    pub fn column_types(&self) -> &[graphdb_core::DataType] {
        &self.column_types
    }

    pub fn set_column_types(&mut self, types: Vec<graphdb_core::DataType>) {
        self.column_types = types;
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::CorrelatedApply(
            Self {
                id: self.id,
                left_input: self.left_input.clone(),
                right_input: self.right_input.clone(),
                deps: self.deps.clone(),
                is_anti_predicate: self.is_anti_predicate,
                output_var: self.output_var.clone(),
                col_names: self.col_names.clone(),
                column_types: self.column_types.clone(),
            },
        )
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::CorrelatedApply(
            cloned,
        )
    }
}

// Implement the PlanNode trait for CorrelatedApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for CorrelatedApplyNode {
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::CorrelatedApply(
            self,
        )
    }
}

// Implement the PlanNodeClonable trait for CorrelatedApplyNode.
impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for CorrelatedApplyNode
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

// Implement the SingleInputNode trait for CorrelatedApplyNode
impl crate::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for CorrelatedApplyNode
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

impl MemoryEstimatable for CorrelatedApplyNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<CorrelatedApplyNode>();

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

        base + is_anti_size + col_names_size + output_var_size + left_right_size + deps_size
    }
}
