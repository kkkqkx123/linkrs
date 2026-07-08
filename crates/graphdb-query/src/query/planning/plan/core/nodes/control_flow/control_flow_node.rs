//! Implementation of control flow nodes
//!
//! Plan nodes related to control flow, such as Start, Argument, Select, Loop, etc.

use std::sync::Arc;

use crate::core::types::{ContextualExpression, SerializableExpression};
use crate::define_plan_node;
use crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    PlanNode, PlanNodeClonable,
};
use crate::query::validator::context::ExpressionAnalysisContext;

define_plan_node! {
    pub struct ArgumentNode {
        var: String,
    }
    enum: Argument
    input: ZeroInputNode
}

impl ArgumentNode {
    pub fn new(id: i64, var: &str) -> Self {
        Self {
            id,
            var: var.to_string(),
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn var(&self) -> &str {
        &self.var
    }
}

define_plan_node! {
    pub struct PassThroughNode {
    }
    enum: PassThrough
    input: ZeroInputNode
}

impl PassThroughNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
        }
    }
}

/// “Select Node” – Choose the if-branch or the else-branch at runtime.
#[derive(Debug)]
pub struct SelectNode {
    id: i64,
    condition: ContextualExpression,
    condition_serializable: Option<SerializableExpression>,
    if_branch:
        Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
    else_branch:
        Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
    output_var: Option<String>,
    col_names: Vec<String>,
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
        }
    }

    pub fn set_if_branch(
        &mut self,
        branch: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.if_branch = Some(Box::new(branch));
    }

    pub fn set_else_branch(
        &mut self,
        branch: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.else_branch = Some(Box::new(branch));
    }

    pub fn if_branch(
        &self,
    ) -> &Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>
    {
        &self.if_branch
    }

    pub fn else_branch(
        &self,
    ) -> &Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>
    {
        &self.else_branch
    }

    pub fn if_branch_mut(
        &mut self,
    ) -> &mut Option<
        Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    > {
        &mut self.if_branch
    }

    pub fn else_branch_mut(
        &mut self,
    ) -> &mut Option<
        Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    > {
        &mut self.else_branch
    }

    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }

    pub fn set_condition(&mut self, condition: ContextualExpression) {
        self.condition = condition;
        self.condition_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        self.condition_serializable =
            Some(SerializableExpression::from_contextual(&self.condition));
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.condition_serializable {
            self.condition = ser_expr.clone().to_contextual(ctx);
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
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Select(
            self.clone(),
        )
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Select(
            cloned,
        )
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
        let serializable_size = std::mem::size_of::<Option<SerializableExpression>>();
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

/// Loop node: A branch that is executed multiple times during runtime.
#[derive(Debug)]
pub struct LoopNode {
    id: i64,
    condition: ContextualExpression,
    condition_serializable: Option<SerializableExpression>,
    body:
        Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl Clone for LoopNode {
    fn clone(&self) -> Self {
        LoopNode {
            id: self.id,
            condition: self.condition.clone(),
            condition_serializable: self.condition_serializable.clone(),
            body: self.body.clone(),
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
        }
    }
}

impl LoopNode {
    pub fn new(id: i64, condition: ContextualExpression) -> Self {
        Self {
            id,
            condition,
            condition_serializable: None,
            body: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn set_body(
        &mut self,
        body: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) {
        self.body = Some(Box::new(body));
    }

    pub fn body(
        &self,
    ) -> &Option<Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>
    {
        &self.body
    }

    pub fn body_mut(
        &mut self,
    ) -> &mut Option<
        Box<crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
    > {
        &mut self.body
    }

    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }

    pub fn set_condition(&mut self, condition: ContextualExpression) {
        self.condition = condition;
        self.condition_serializable = None;
    }

    pub fn prepare_for_serialization(&mut self) {
        self.condition_serializable =
            Some(SerializableExpression::from_contextual(&self.condition));
    }

    pub fn after_deserialization(&mut self, ctx: Arc<ExpressionAnalysisContext>) {
        if let Some(ref ser_expr) = self.condition_serializable {
            self.condition = ser_expr.clone().to_contextual(ctx);
        }
    }

    pub fn type_name(&self) -> &'static str {
        "Loop"
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
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Loop(
            self.clone(),
        )
    }

    pub fn clone_with_new_id(
        &self,
        new_id: i64,
    ) -> crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum::Loop(cloned)
    }
}

impl PlanNode for LoopNode {
    fn id(&self) -> i64 {
        self.id()
    }

    fn name(&self) -> &'static str {
        "Loop"
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
        PlanNodeEnum::Loop(self)
    }
}

impl PlanNodeClonable for LoopNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone_plan_node()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        self.clone_with_new_id(new_id)
    }
}

impl MemoryEstimatable for LoopNode {
    fn estimate_memory(&self) -> usize {
        let base = std::mem::size_of::<LoopNode>();

        // Estimate condition (ContextualExpression)
        let condition_size = std::mem::size_of::<ContextualExpression>()
            + std::mem::size_of::<Arc<ExpressionAnalysisContext>>();

        // Estimate condition_serializable
        let serializable_size = std::mem::size_of::<Option<SerializableExpression>>();
        let serializable_data_size = if self.condition_serializable.is_some() {
            std::mem::size_of::<SerializableExpression>()
        } else {
            0
        };

        // Estimate body Option<Box<PlanNodeEnum>>
        let body_size = std::mem::size_of::<Option<Box<PlanNodeEnum>>>();

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
            + body_size
            + col_names_size
            + output_var_size
    }
}

// ============================================================================
// Transaction Control Nodes
// ============================================================================

/// Transaction isolation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationLevel {
    /// Read uncommitted - lowest isolation level
    ReadUncommitted,
    /// Read committed - default for most databases
    #[default]
    ReadCommitted,
    /// Repeatable read - ensures consistent reads within transaction
    RepeatableRead,
    /// Serializable - highest isolation level
    Serializable,
}

/// Begin Transaction Node
/// Starts a new transaction with specified isolation level
#[derive(Debug, Clone)]
pub struct BeginTransactionNode {
    id: i64,
    isolation_level: IsolationLevel,
    read_only: bool,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl BeginTransactionNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            isolation_level: IsolationLevel::default(),
            read_only: false,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn with_isolation_level(mut self, level: IsolationLevel) -> Self {
        self.isolation_level = level;
        self
    }

    pub fn with_read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    pub fn isolation_level(&self) -> IsolationLevel {
        self.isolation_level
    }

    pub fn read_only(&self) -> bool {
        self.read_only
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
}

impl PlanNode for BeginTransactionNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "BeginTransaction"
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
        PlanNodeEnum::BeginTransaction(self)
    }
}

impl PlanNodeClonable for BeginTransactionNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone().into_enum()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.into_enum()
    }
}

impl MemoryEstimatable for BeginTransactionNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<BeginTransactionNode>()
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}

/// Commit Node
/// Commits the current transaction
#[derive(Debug, Clone)]
pub struct CommitNode {
    id: i64,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl CommitNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
        }
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
}

impl PlanNode for CommitNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "Commit"
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
        PlanNodeEnum::Commit(self)
    }
}

impl PlanNodeClonable for CommitNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone().into_enum()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.into_enum()
    }
}

impl MemoryEstimatable for CommitNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<CommitNode>()
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}

/// Rollback Node
/// Rolls back the current transaction
#[derive(Debug, Clone)]
pub struct RollbackNode {
    id: i64,
    savepoint: Option<String>,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl RollbackNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            savepoint: None,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn with_savepoint(mut self, savepoint: String) -> Self {
        self.savepoint = Some(savepoint);
        self
    }

    pub fn savepoint(&self) -> Option<&str> {
        self.savepoint.as_deref()
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
}

impl PlanNode for RollbackNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "Rollback"
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
        PlanNodeEnum::Rollback(self)
    }
}

impl PlanNodeClonable for RollbackNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone().into_enum()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.into_enum()
    }
}

impl MemoryEstimatable for RollbackNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<RollbackNode>()
            + self.savepoint.as_ref().map(|s| s.capacity()).unwrap_or(0)
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_argument_node_creation() {
        let node = ArgumentNode::new(1, "var_name");
        assert_eq!(node.type_name(), "ArgumentNode");
        assert_eq!(node.id(), 1);
        assert_eq!(node.var(), "var_name");
    }

    #[test]
    fn test_select_node_creation() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable("condition".to_string()),
        );
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        let node = SelectNode::new(1, ctx_expr);
        assert_eq!(node.type_name(), "Select");
        assert_eq!(node.id(), 1);
        assert!(node.if_branch().is_none());
        assert!(node.else_branch().is_none());
    }

    #[test]
    fn test_loop_node_creation() {
        let ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_meta = crate::core::types::expr::ExpressionMeta::new(
            crate::core::Expression::Variable("condition".to_string()),
        );
        let id = ctx.register_expression(expr_meta);
        let ctx_expr = ContextualExpression::new(id, ctx);
        let node = LoopNode::new(1, ctx_expr);
        assert_eq!(node.type_name(), "Loop");
        assert_eq!(node.id(), 1);
        assert!(node.body().is_none());
    }

    #[test]
    fn test_pass_through_node_creation() {
        let node = PassThroughNode::new(1);
        assert_eq!(node.type_name(), "PassThroughNode");
        assert_eq!(node.id(), 1);
    }
}
