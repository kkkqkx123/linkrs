use std::sync::Arc;

use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, PlanNodeClonable};
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::types::{ContextualExpression, SerializableExpression};

/// “Select Node” – Choose the if-branch or the else-branch at runtime.
#[derive(Debug)]
pub struct SelectNode {
    id: i64,
    condition: ContextualExpression,
    condition_serializable: Option<Box<SerializableExpression>>,
    if_branch: Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
    else_branch:
        Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
    output_var: Option<String>,
    col_names: Vec<String>,
    column_types: Vec<graphdb_core::DataType>,
}

impl Clone for SelectNode {
    fn clone(&self) -> Self {
        SelectNode {
            id: self.id,
            condition: self.condition.clone(),
            condition_serializable: self.condition_serializable.clone(),
            if_branch: self.if_branch.clone(),
            else_branch: self.else_branch.clone(),
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        }
    }
}

impl SelectNode {
    pub fn new(id: i64, condition: ContextualExpression) -> Self {
        Self {
            id,
            condition,
            condition_serializable: None,
            if_branch: None,
            else_branch: None,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn set_if_branch(
        &mut self,
        branch: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.if_branch = Some(Box::new(branch));
    }

    pub fn set_else_branch(
        &mut self,
        branch: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.else_branch = Some(Box::new(branch));
    }

    pub fn if_branch(
        &self,
    ) -> &Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>> {
        &self.if_branch
    }

    pub fn else_branch(
        &self,
    ) -> &Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>> {
        &self.else_branch
    }

    pub fn if_branch_mut(
        &mut self,
    ) -> &mut Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>
    {
        &mut self.if_branch
    }

    pub fn else_branch_mut(
        &mut self,
    ) -> &mut Option<Box<crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>
    {
        &mut self.else_branch
    }

    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }

    pub fn set_condition(&mut self, condition: ContextualExpression) {
        self.condition = condition;
        self.condition_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) -> Result<(), String> {
        self.condition_serializable = Some(Box::new(SerializableExpression::from_contextual(
            &self.condition,
        )?));
        Ok(())
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.condition_serializable {
            self.condition = ser_expr.as_ref().clone().to_contextual(ctx);
        }
    }

    pub fn type_name(&self) -> &'static str {
        "Select"
    }

    pub fn id(&self) -> i64 {
        self.id
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

    pub fn clone_plan_node(
        &self,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Select(self.clone())
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Select(cloned)
    }
}

impl PlanNode for SelectNode {
    fn id(&self) -> i64 {
        self.id()
    }

    fn name(&self) -> &'static str {
        "Select"
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
        PlanNodeEnum::Select(self)
    }
}

impl PlanNodeClonable for SelectNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

impl MemoryEstimatable for SelectNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<SelectNode>();

        // Estimate condition (ContextualExpression)
        let condition_size = std::mem::size_of::<ContextualExpression>()
            + std::mem::size_of::<Arc<ExpressionAnalysisContext>>();

        // Estimate condition_serializable
        let serializable_size = std::mem::size_of::<Option<Box<SerializableExpression>>>();
        let serializable_data_size = if self.condition_serializable.is_some() {
            std::mem::size_of::<SerializableExpression>()
        } else {
            0
        };

        // Estimate if_branch and else_branch Option<Box<PlanNodeEnum>>
        let branch_size = std::mem::size_of::<Option<Box<PlanNodeEnum>>>() * 2;

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

        base + condition_size
            + serializable_size
            + serializable_data_size
            + branch_size
            + col_names_size
            + output_var_size
    }
}
