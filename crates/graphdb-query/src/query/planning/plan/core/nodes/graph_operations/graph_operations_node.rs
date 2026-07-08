//! Implementation of data processing nodes
//!
//! Plan nodes related to data processing, including Union, Unwind, Dedup, etc.

use crate::core::types::expr::contextual::ContextualExpression;
use crate::define_plan_node_with_deps;
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;

define_plan_node_with_deps! {
    pub struct UnionNode {
        distinct: bool,
    }
    enum: Union
    input: SingleInputNode
}

impl UnionNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        union_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        distinct: bool,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input, union_input],
            distinct,
            output_var: None,
            col_names,
        })
    }

    pub fn distinct(&self) -> bool {
        self.distinct
    }

    pub fn union_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.deps[1]
    }
}

define_plan_node_with_deps! {
    pub struct UnwindNode {
        alias: String,
        list_expression: ContextualExpression,
    }
    enum: Unwind
    input: SingleInputNode
}

impl UnwindNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        alias: &str,
        list_expression: ContextualExpression,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let mut col_names = input.col_names().to_vec();
        col_names.push(alias.to_string());

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            alias: alias.to_string(),
            list_expression,
            output_var: None,
            col_names,
        })
    }

    pub fn alias(&self) -> &str {
        &self.alias
    }

    pub fn list_expression(&self) -> &ContextualExpression {
        &self.list_expression
    }
}

define_plan_node_with_deps! {
    pub struct DedupNode {
    }
    enum: Dedup
    input: SingleInputNode
}

impl DedupNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            output_var: None,
            col_names,
        })
    }
}

define_plan_node_with_deps! {
    pub struct DataCollectNode {
        collect_kind: String,
    }
    enum: DataCollect
    input: SingleInputNode
}

impl DataCollectNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        collect_kind: &str,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            collect_kind: collect_kind.to_string(),
            output_var: None,
            col_names,
        })
    }

    pub fn collect_kind(&self) -> &str {
        &self.collect_kind
    }
}

define_plan_node_with_deps! {
    pub struct AssignNode {
        assignments: Vec<(String, ContextualExpression)>,
    }
    enum: Assign
    input: SingleInputNode
}

impl AssignNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        assignments: Vec<(String, ContextualExpression)>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            assignments,
            output_var: None,
            col_names,
        })
    }

    pub fn assignments(&self) -> &[(String, ContextualExpression)] {
        &self.assignments
    }
}

/// RollUpApply node – Grouped aggregation and data collection
///
/// Receive two inputs from the left and right. Group the data from the right according to the comparison column and collect it in a list.
/// Return the corresponding aggregate results for each row on the left side.
#[derive(Debug, Clone)]
pub struct RollUpApplyNode {
    id: i64,
    left_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    left_input_var: Option<String>,
    right_input_var: Option<String>,
    compare_cols: Vec<String>,
    collect_col: Option<String>,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl RollUpApplyNode {
    pub fn new(
        left_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        compare_cols: Vec<String>,
        collect_col: Option<String>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
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
        })
    }

    pub fn left_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    pub fn right_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
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
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    pub fn add_dependency(
        &mut self,
        dep: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(
            Self {
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
            },
        )
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(
            cloned,
        )
    }
}

// Implement the PlanNode trait for RollUpApplyNode
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode
    for RollUpApplyNode
{
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

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::RollUpApply(
            self,
        )
    }
}

// Implement the PlanNodeClonable trait for RollUpApplyNode
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for RollUpApplyNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

// Implement the SingleInputNode trait for RollUpApplyNode
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for RollUpApplyNode
{
    fn input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    fn input_mut(
        &mut self,
    ) -> &mut crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.left_input
    }

    fn set_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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
            Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >() * 2;

        // Estimate deps Vec<PlanNodeEnum>
        let deps_size = std::mem::size_of::<
            Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
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

/// PatternApply node – Pattern matching application
///
/// Receive two inputs from the left and right sides. Determine whether the data on the left side matches the pattern on the right side based on the key columns.
/// Supports both forward matching (EXISTS) and reverse matching (NOT EXISTS).
#[derive(Debug, Clone)]
pub struct PatternApplyNode {
    id: i64,
    left_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    left_input_var: Option<String>,
    right_input_var: Option<String>,
    key_cols: Vec<crate::core::types::ContextualExpression>,
    is_anti_predicate: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl PatternApplyNode {
    pub fn new(
        left_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        key_cols: Vec<crate::core::types::ContextualExpression>,
        is_anti_predicate: bool,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = left_input.col_names().to_vec();
        let deps = vec![left_input.clone(), right_input.clone()];

        Ok(Self {
            id: -1,
            left_input: Box::new(left_input),
            right_input: Box::new(right_input),
            deps,
            left_input_var: None,
            right_input_var: None,
            key_cols,
            is_anti_predicate,
            output_var: None,
            col_names,
        })
    }

    pub fn left_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    pub fn right_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.right_input
    }

    pub fn left_input_var(&self) -> Option<&String> {
        self.left_input_var.as_ref()
    }

    pub fn right_input_var(&self) -> Option<&String> {
        self.right_input_var.as_ref()
    }

    pub fn key_cols(&self) -> &[crate::core::types::ContextualExpression] {
        &self.key_cols
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
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    pub fn add_dependency(
        &mut self,
        dep: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(
            Self {
                id: self.id,
                left_input: self.left_input.clone(),
                right_input: self.right_input.clone(),
                deps: self.deps.clone(),
                left_input_var: self.left_input_var.clone(),
                right_input_var: self.right_input_var.clone(),
                key_cols: self.key_cols.clone(),
                is_anti_predicate: self.is_anti_predicate,
                output_var: self.output_var.clone(),
                col_names: self.col_names.clone(),
            },
        )
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(
            cloned,
        )
    }
}

// Implement the PlanNode trait for PatternApplyNode
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode
    for PatternApplyNode
{
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

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::PatternApply(
            self,
        )
    }
}

// Implement the PlanNodeClonable trait for PatternApplyNode.
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for PatternApplyNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

// Implement the SingleInputNode trait for PatternApplyNode
impl crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for PatternApplyNode
{
    fn input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    fn input_mut(
        &mut self,
    ) -> &mut crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.left_input
    }

    fn set_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.left_input = input.clone();
        self.deps.clear();
        self.deps.push(input);
    }
}

impl MemoryEstimatable for PatternApplyNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<PatternApplyNode>();

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

        // Estimate key_cols Vec<ContextualExpression>
        let key_cols_size = std::mem::size_of::<Vec<crate::core::types::ContextualExpression>>()
            + self.key_cols.len() * std::mem::size_of::<crate::core::types::ContextualExpression>();

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
            Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >() * 2;

        // Estimate deps Vec<PlanNodeEnum>
        let deps_size = std::mem::size_of::<
            Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + input_var_size
            + key_cols_size
            + is_anti_size
            + col_names_size
            + output_var_size
            + left_right_size
            + deps_size
    }
}

define_plan_node_with_deps! {
    pub struct MaterializeNode {
    }
    enum: Materialize
    input: SingleInputNode
}

impl MaterializeNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();

        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            output_var: None,
            col_names,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_union_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );
        let start_node2 =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        let union_node = UnionNode::new(start_node, start_node2, true)
            .expect("Union node should be created successfully");

        assert_eq!(union_node.type_name(), "UnionNode");
        assert_eq!(union_node.dependencies().len(), 2);
        assert!(union_node.distinct());
    }

    #[test]
    fn test_unwind_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        use crate::core::types::expr::{ContextualExpression, Expression, ExpressionMeta};
        use crate::query::validator::context::ExpressionAnalysisContext;
        use std::sync::Arc;

        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let list_expr = Expression::Variable("list".to_string());
        let list_meta = ExpressionMeta::new(list_expr);
        let list_id = expr_ctx.register_expression(list_meta);
        let list_contextual = ContextualExpression::new(list_id, expr_ctx);

        let unwind_node = UnwindNode::new(start_node, "item", list_contextual)
            .expect("Unwind node should be created successfully");

        assert_eq!(unwind_node.type_name(), "UnwindNode");
        assert_eq!(unwind_node.dependencies().len(), 1);
        assert_eq!(unwind_node.alias(), "item");
    }

    #[test]
    fn test_dedup_node_creation() {
        let start_node =
            crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Start(
                StartNode::new(),
            );

        let dedup_node =
            DedupNode::new(start_node).expect("Dedup node should be created successfully");

        assert_eq!(dedup_node.type_name(), "DedupNode");
        assert_eq!(dedup_node.dependencies().len(), 1);
    }
}

/// Remove a node: Delete an attribute or a tag.
///
/// Attributes and labels used for deleting vertices or edges
#[derive(Debug, Clone)]
pub struct RemoveNode {
    id: i64,
    input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    remove_items: Vec<(String, ContextualExpression)>,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl RemoveNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        remove_items: Vec<(String, ContextualExpression)>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
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
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
        &self.deps
    }

    pub fn add_dependency(
        &mut self,
        dep: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode
    for RemoveNode
{
    fn input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.input
    }

    fn input_mut(
        &mut self,
    ) -> &mut crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.input
    }

    fn set_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.input = input;
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for RemoveNode {
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

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Remove(self)
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
            Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        // Estimate deps Vec<PlanNodeEnum>
        let deps_size = std::mem::size_of::<
            Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + remove_items_size + col_names_size + output_var_size + input_size + deps_size
    }
}

/// Apply node – Correlated subquery execution
///
/// Execute a correlated subquery for each row from the left input.
/// The right input (subquery) can reference columns from the left input.
/// This is the standard Apply operator used in query optimization.
#[derive(Debug, Clone)]
pub struct ApplyNode {
    id: i64,
    left_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    right_input: Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    deps: Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    left_input_var: Option<String>,
    right_input_var: Option<String>,
    correlated_cols: Vec<String>,
    apply_kind: ApplyKind,
    output_var: Option<String>,
    col_names: Vec<String>,
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
        left_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right_input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        correlated_cols: Vec<String>,
        apply_kind: ApplyKind,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
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
        })
    }

    pub fn left_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    pub fn right_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
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
    ) -> &[crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
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
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(Self {
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
        })
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for ApplyNode {
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

    fn into_enum(
        self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(self)
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable
    for ApplyNode
{
    fn clone_plan_node(
        &self,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Apply(cloned)
    }
}

impl crate::query::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode
    for ApplyNode
{
    fn left_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.left_input
    }

    fn right_input(
        &self,
    ) -> &crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.right_input
    }

    fn left_input_mut(
        &mut self,
    ) -> &mut crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.left_input
    }

    fn right_input_mut(
        &mut self,
    ) -> &mut crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &mut self.right_input
    }

    fn set_left_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        *self.left_input = input;
    }

    fn set_right_input(
        &mut self,
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
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
            Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >() * 2;

        let deps_size = std::mem::size_of::<
            Vec<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
        >();

        base + input_var_size
            + correlated_cols_size
            + col_names_size
            + output_var_size
            + left_right_size
            + deps_size
    }
}
