use crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable;
use crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::planning::plan::core::nodes::base::plan_node_traits::{PlanNode, PlanNodeClonable};

// ============================================================================
// Transaction Control Nodes
// ============================================================================
//
// Kept inside the plan-node system on purpose. Although transaction boundaries
// are conceptually session-level concerns, these nodes:
// 1. Are compiled into `TxnOperator` in the streaming executor
//    (arena_builder/assembler/conversion.rs) and produce the structured
//    command result row;
// 2. Act as partition boundaries in the optimizer
//    (optimizer/partitioning.rs `has_transaction_boundary`).
// The actual TransactionManager side effects (begin/commit/rollback/savepoint)
// run in graphdb-server before the plan executes; the nodes carry the command
// through scheduling and execution, so they must remain reachable from
// PlanNodeEnum.

/// Transaction isolation level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IsolationLevel {
    /// Read committed - snapshot isolation per statement
    #[default]
    ReadCommitted,
    /// Repeatable read - consistent snapshot across entire transaction
    RepeatableRead,
    /// Serializable - highest isolation level, certifies read-write conflicts
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

/// Savepoint Node
/// Creates a savepoint within the current transaction.
#[derive(Debug, Clone)]
pub struct SavepointNode {
    id: i64,
    savepoint: String,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl SavepointNode {
    pub fn new(id: i64, savepoint: String) -> Self {
        Self {
            id,
            savepoint,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn savepoint(&self) -> &str {
        &self.savepoint
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

impl PlanNode for SavepointNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "Savepoint"
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
        PlanNodeEnum::Savepoint(self)
    }
}

impl PlanNodeClonable for SavepointNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone().into_enum()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.into_enum()
    }
}

impl MemoryEstimatable for SavepointNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<SavepointNode>()
            + self.savepoint.capacity()
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}

/// Release Savepoint Node
/// Releases a savepoint within the current transaction.
#[derive(Debug, Clone)]
pub struct ReleaseSavepointNode {
    id: i64,
    savepoint: String,
    output_var: Option<String>,
    col_names: Vec<String>,
}

impl ReleaseSavepointNode {
    pub fn new(id: i64, savepoint: String) -> Self {
        Self {
            id,
            savepoint,
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn savepoint(&self) -> &str {
        &self.savepoint
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

impl PlanNode for ReleaseSavepointNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "ReleaseSavepoint"
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
        PlanNodeEnum::ReleaseSavepoint(self)
    }
}

impl PlanNodeClonable for ReleaseSavepointNode {
    fn clone_plan_node(&self) -> PlanNodeEnum {
        self.clone().into_enum()
    }

    fn clone_with_new_id(&self, new_id: i64) -> PlanNodeEnum {
        let mut cloned = self.clone();
        cloned.id = new_id;
        cloned.into_enum()
    }
}

impl MemoryEstimatable for ReleaseSavepointNode {
    fn estimate_memory(&self) -> usize {
        std::mem::size_of::<ReleaseSavepointNode>()
            + self.savepoint.capacity()
            + self.col_names.iter().map(|s| s.capacity()).sum::<usize>()
            + self.output_var.as_ref().map(|s| s.capacity()).unwrap_or(0)
    }
}
