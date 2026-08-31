//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;

/// Apply node – Correlated subquery execution
///
/// Execute a correlated subquery for each row from the left input.
/// The right input (subquery) can reference columns from the left input.
/// This is the standard Apply operator used in query optimization.
#[derive(Debug, Clone)]
pub struct ApplyNode {
    id: i64,
    left_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    left_input_var: Option<String>,
    right_input_var: Option<String>,
    correlated_cols: Vec<String>,
    apply_kind: ApplyKind,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

/// Kind of Apply operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyKind {
    /// Standard Apply - execute subquery for each row
    Standard,
    /// Semi Apply - returns true if subquery returns at least one row
    Semi,
    /// Anti Apply - returns true if subquery returns no rows
    Anti,
    /// Single Apply - expect exactly one row from subquery
    Single,
    /// All Apply - for ALL subquery
    All,
}

impl ApplyNode {
    pub fn new(
        left_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        correlated_cols: Vec<String>,
        apply_kind: ApplyKind,
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
            correlated_cols,
            apply_kind,
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

    pub fn correlated_cols(&self) -> &[String] {
        &self.correlated_cols
    }

    pub fn apply_kind(&self) -> ApplyKind {
        self.apply_kind
    }

    pub fn is_semi(&self) -> bool {
        self.apply_kind == ApplyKind::Semi
    }

    pub fn is_anti(&self) -> bool {
        self.apply_kind == ApplyKind::Anti
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn type_name(&self) -> &'static str {
        "Apply"
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(Self {
            id: self.id,
            left_input: self.left_input.clone(),
            right_input: self.right_input.clone(),
            deps: self.deps.clone(),
            left_input_var: self.left_input_var.clone(),
            right_input_var: self.right_input_var.clone(),
            correlated_cols: self.correlated_cols.clone(),
            apply_kind: self.apply_kind,
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        })
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for ApplyNode {
    fn id(&self) -> i64 {
        self.id
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
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(self)
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for ApplyNode {
    fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(cloned)
    }
}

impl crate::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode for ApplyNode {
    fn left_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    fn right_input(
        &self,
    ) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.right_input
    }

    fn left_input_mut(
        &mut self,
    ) -> &mut crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.left_input
    }

    fn right_input_mut(
        &mut self,
    ) -> &mut crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.right_input
    }

    fn set_left_input(
        &mut self,
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.left_input = input;
    }

    fn set_right_input(
        &mut self,
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.right_input = input;
    }
}

impl MemoryEstimatable for ApplyNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<ApplyNode>();

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

        let correlated_cols_size = std::mem::size_of::<Vec<String>>()
            + self
                .correlated_cols
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        let col_names_size = std::mem::size_of::<Vec<String>>()
            + self
                .col_names
                .iter()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .sum::<usize>();

        let output_var_size = std::mem::size_of::<Option<String>>()
            + self
                .output_var
                .as_ref()
                .map(|s| std::mem::size_of::<String>() + s.capacity())
                .unwrap_or(0);

        let left_right_size = std::mem::size_of::<
            Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >() * 2;

        let deps_size = std::mem::size_of::<
            Vec<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + input_var_size
            + correlated_cols_size
            + col_names_size
            + output_var_size
            + left_right_size
            + deps_size
    }
}
