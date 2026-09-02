//! Logical graph operation nodes: DataCollect, Remove, PatternApply, RollUpApply, Union, Minus, Intersect, Unwind, Materialize, Assign, Apply, Dedup.

use crate::define_logical_join_node;
use crate::define_logical_plan_node_with_deps;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyKind;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_logical_plan_node_with_deps! {
    pub struct LogicalUnionNode {
        distinct: bool,
    }
    enum: Union
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalUnwindNode {
        alias: String,
        list_expression: ContextualExpression,
    }
    enum: Unwind
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalDataCollectNode {
        collect_kind: String,
    }
    enum: DataCollect
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalAssignNode {
        assignments: Vec<(String, ContextualExpression)>,
    }
    enum: Assign
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalRollUpApplyNode {
        left_input_var: Option<String>,
        right_input_var: Option<String>,
        compare_cols: Vec<String>,
        collect_col: Option<String>,
    }
    enum: RollUpApply
    input: SingleInputNode
}

define_logical_join_node! {
    pub struct LogicalPatternApplyNode {
        is_anti_predicate: bool,
    }
    enum: PatternApply
}

define_logical_join_node! {
    pub struct LogicalCorrelatedApplyNode {
        is_anti_predicate: bool,
    }
    enum: CorrelatedApply
}

define_logical_plan_node_with_deps! {
    pub struct LogicalMaterializeNode {}
    enum: Materialize
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalRemoveNode {
        remove_items: Vec<(String, ContextualExpression)>,
    }
    enum: Remove
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalMinusNode {}
    enum: Minus
    input: SingleInputNode
}

define_logical_plan_node_with_deps! {
    pub struct LogicalIntersectNode {}
    enum: Intersect
    input: SingleInputNode
}

/// Logical ApplyNode – binary input apply operation.
#[derive(Debug)]
pub struct LogicalApplyNode {
    pub id: i64,
    pub left: Box<LogicalNodeEnum>,
    pub right: Box<LogicalNodeEnum>,
    pub deps: Vec<LogicalNodeEnum>,
    pub left_input_var: Option<String>,
    pub right_input_var: Option<String>,
    pub correlated_cols: Vec<String>,
    pub apply_kind: ApplyKind,
    pub output_var: Option<String>,
    pub col_names: Vec<String>,
    pub column_types: Vec<graphdb_core::DataType>,
}

impl Clone for LogicalApplyNode {
    fn clone(&self) -> Self {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        Self {
            id: next_node_id(),
            left: self.left.clone(),
            right: self.right.clone(),
            deps: self.deps.clone(),
            left_input_var: self.left_input_var.clone(),
            right_input_var: self.right_input_var.clone(),
            correlated_cols: self.correlated_cols.clone(),
            apply_kind: self.apply_kind,
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        }
    }
}

impl LogicalApplyNode {
    pub fn id(&self) -> i64 {
        self.id
    }
    pub fn type_name(&self) -> &'static str {
        "LogicalApplyNode"
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

    pub fn left_input(&self) -> &LogicalNodeEnum {
        &self.left
    }
    pub fn right_input(&self) -> &LogicalNodeEnum {
        &self.right
    }
    pub fn left_input_mut(&mut self) -> &mut LogicalNodeEnum {
        &mut self.left
    }
    pub fn right_input_mut(&mut self) -> &mut LogicalNodeEnum {
        &mut self.right
    }
    pub fn set_left_input(&mut self, input: LogicalNodeEnum) {
        *self.left = input.clone();
        if !self.deps.is_empty() {
            self.deps[0] = input.clone();
        }
    }
    pub fn set_right_input(&mut self, input: LogicalNodeEnum) {
        *self.right = input.clone();
        if self.deps.len() > 1 {
            self.deps[1] = input.clone();
        }
    }
    pub fn apply_kind(&self) -> &ApplyKind {
        &self.apply_kind
    }
    pub fn correlated_cols(&self) -> &[String] {
        &self.correlated_cols
    }
}

impl crate::planning::plan::logical::logical_node_traits::LogicalNode for LogicalApplyNode {
    fn id(&self) -> i64 {
        self.id()
    }
    fn name(&self) -> &'static str {
        self.type_name()
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
    fn into_enum(self) -> LogicalNodeEnum {
        LogicalNodeEnum::Apply(self)
    }
}

impl crate::planning::plan::logical::logical_node_traits::LogicalBinaryInputNode
    for LogicalApplyNode
{
    fn left_input(&self) -> &LogicalNodeEnum {
        &self.left
    }
    fn right_input(&self) -> &LogicalNodeEnum {
        &self.right
    }
    fn left_input_mut(&mut self) -> &mut LogicalNodeEnum {
        &mut self.left
    }
    fn right_input_mut(&mut self) -> &mut LogicalNodeEnum {
        &mut self.right
    }
    fn set_left_input(&mut self, input: LogicalNodeEnum) {
        *self.left = input.clone();
        if !self.deps.is_empty() {
            self.deps[0] = input.clone();
        }
    }
    fn set_right_input(&mut self, input: LogicalNodeEnum) {
        *self.right = input.clone();
        if self.deps.len() > 1 {
            self.deps[1] = input.clone();
        }
    }
}
