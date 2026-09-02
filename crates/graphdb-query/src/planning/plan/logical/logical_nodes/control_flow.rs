//! Logical control flow nodes: Argument, Loop, PassThrough, Select, BeginTransaction, Commit, Rollback.

use crate::define_logical_plan_node;
use crate::planning::plan::core::nodes::control_flow::control_flow_node::IsolationLevel;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use graphdb_core::types::expr::contextual::ContextualExpression;

define_logical_plan_node! {
    pub struct LogicalArgumentNode {
        var: String,
    }
    enum: Argument
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalPassThroughNode {}
    enum: PassThrough
    input: ZeroInputNode
}

/// Logical SelectNode – chooses if-branch or else-branch at runtime.
#[derive(Debug)]
pub struct LogicalSelectNode {
    pub id: i64,
    pub condition: ContextualExpression,
    pub if_branch: Option<Box<LogicalNodeEnum>>,
    pub else_branch: Option<Box<LogicalNodeEnum>>,
    pub output_var: Option<String>,
    pub col_names: Vec<String>,
    pub column_types: Vec<graphdb_core::DataType>,
}

impl Clone for LogicalSelectNode {
    fn clone(&self) -> Self {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        Self {
            id: next_node_id(),
            condition: self.condition.clone(),
            if_branch: self.if_branch.clone(),
            else_branch: self.else_branch.clone(),
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        }
    }
}

impl LogicalSelectNode {
    pub fn id(&self) -> i64 {
        self.id
    }
    pub fn type_name(&self) -> &'static str {
        "LogicalSelectNode"
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

    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }
    pub fn if_branch(&self) -> Option<&LogicalNodeEnum> {
        self.if_branch.as_deref()
    }
    pub fn else_branch(&self) -> Option<&LogicalNodeEnum> {
        self.else_branch.as_deref()
    }
    pub fn set_if_branch(&mut self, branch: LogicalNodeEnum) {
        self.if_branch = Some(Box::new(branch));
    }
    pub fn set_else_branch(&mut self, branch: LogicalNodeEnum) {
        self.else_branch = Some(Box::new(branch));
    }
    pub fn take_if_branch(&mut self) -> Option<Box<LogicalNodeEnum>> {
        self.if_branch.take()
    }
    pub fn take_else_branch(&mut self) -> Option<Box<LogicalNodeEnum>> {
        self.else_branch.take()
    }
}

impl crate::planning::plan::logical::logical_node_traits::LogicalNode for LogicalSelectNode {
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
        LogicalNodeEnum::Select(self)
    }
}

/// Logical LoopNode – loops body while condition holds.
#[derive(Debug)]
pub struct LogicalLoopNode {
    pub id: i64,
    pub condition: ContextualExpression,
    pub body: Option<Box<LogicalNodeEnum>>,
    pub output_var: Option<String>,
    pub col_names: Vec<String>,
    pub column_types: Vec<graphdb_core::DataType>,
}

impl Clone for LogicalLoopNode {
    fn clone(&self) -> Self {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        Self {
            id: next_node_id(),
            condition: self.condition.clone(),
            body: self.body.clone(),
            output_var: self.output_var.clone(),
            col_names: self.col_names.clone(),
            column_types: self.column_types.clone(),
        }
    }
}

impl LogicalLoopNode {
    pub fn id(&self) -> i64 {
        self.id
    }
    pub fn type_name(&self) -> &'static str {
        "LogicalLoopNode"
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

    pub fn condition(&self) -> &ContextualExpression {
        &self.condition
    }
    pub fn body(&self) -> Option<&LogicalNodeEnum> {
        self.body.as_deref()
    }
    pub fn body_mut(&mut self) -> Option<&mut LogicalNodeEnum> {
        self.body.as_deref_mut()
    }
    pub fn take_body(&mut self) -> Option<Box<LogicalNodeEnum>> {
        self.body.take()
    }
    pub fn set_body(&mut self, body: LogicalNodeEnum) {
        self.body = Some(Box::new(body));
    }

    /// Create a loop node with its body pre-attached.
    pub fn new_with_body(condition: ContextualExpression, body: LogicalNodeEnum) -> Self {
        use crate::planning::plan::core::node_id_generator::next_node_id;
        Self {
            id: next_node_id(),
            condition,
            body: Some(Box::new(body)),
            output_var: None,
            col_names: vec![],
            column_types: vec![],
        }
    }
}

impl crate::planning::plan::logical::logical_node_traits::LogicalNode for LogicalLoopNode {
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
        LogicalNodeEnum::Loop(self)
    }
}

define_logical_plan_node! {
    pub struct LogicalBeginTransactionNode {
        isolation_level: IsolationLevel,
        read_only: bool,
    }
    enum: BeginTransaction
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalCommitNode {}
    enum: Commit
    input: ZeroInputNode
}

define_logical_plan_node! {
    pub struct LogicalRollbackNode {
        savepoint: Option<String>,
    }
    enum: Rollback
    input: ZeroInputNode
}
