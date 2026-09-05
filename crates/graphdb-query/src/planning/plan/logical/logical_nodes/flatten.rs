use crate::planning::plan::core::node_id_generator::next_node_id;
use crate::planning::plan::factorization::FGroupPos;
use crate::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
use crate::planning::plan::logical::logical_node_traits::{LogicalNode, LogicalSingleInputNode};

/// Logical flatten node: expands an unflat factorized group into flat rows.
///
/// Corresponds to Ladybug `LogicalFlatten` in `ref/ladybug/src/planner/operator/logical_flatten.cpp`.
#[derive(Debug, Clone)]
pub struct LogicalFlattenNode {
    pub id: i64,
    pub group_pos: FGroupPos,
    /// Alias names of the flattened group snapshotted at rewrite time.
    ///
    /// Traceability metadata only (EXPLAIN-visible): the schema alias
    /// namespace and the runtime column namespace are not identical, so
    /// the executor never validates these names. Empty means the mapping
    /// is unknown (anonymous group), not that the group is empty.
    pub group_columns: Vec<String>,
    /// Group count of the child schema at rewrite time.
    ///
    /// Lets the executor reject a stale `group_pos` loudly (`group_pos`
    /// must be below this count). `None` means unknown (hand-built or
    /// legacy plan) and skips the check honestly.
    pub expected_groups: Option<u32>,
    pub input: Option<Box<LogicalNodeEnum>>,
    pub deps: Vec<LogicalNodeEnum>,
    pub output_var: Option<String>,
    pub col_names: Vec<String>,
    pub column_types: Vec<graphdb_core::DataType>,
}

impl LogicalFlattenNode {
    pub fn new(group_pos: FGroupPos, input: LogicalNodeEnum) -> Self {
        Self {
            id: next_node_id(),
            group_pos,
            group_columns: Vec::new(),
            expected_groups: None,
            input: Some(Box::new(input)),
            deps: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: Vec::new(),
        }
    }

    pub fn with_id(id: i64, group_pos: FGroupPos, input: LogicalNodeEnum) -> Self {
        Self {
            id,
            group_pos,
            group_columns: Vec::new(),
            expected_groups: None,
            input: Some(Box::new(input)),
            deps: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
            column_types: Vec::new(),
        }
    }

    pub fn group_pos(&self) -> FGroupPos {
        self.group_pos
    }

    pub fn group_columns(&self) -> &[String] {
        &self.group_columns
    }

    pub fn set_group_columns(&mut self, columns: Vec<String>) {
        self.group_columns = columns;
    }

    pub fn expected_groups(&self) -> Option<u32> {
        self.expected_groups
    }

    pub fn set_expected_groups(&mut self, count: u32) {
        self.expected_groups = Some(count);
    }

    pub fn id(&self) -> i64 {
        self.id
    }

    pub fn col_names(&self) -> &[String] {
        &self.col_names
    }

    pub fn type_name(&self) -> &'static str {
        "LogicalFlatten"
    }

    pub fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    pub fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    pub fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }
}

impl LogicalNode for LogicalFlattenNode {
    fn id(&self) -> i64 {
        self.id
    }

    fn name(&self) -> &'static str {
        "LogicalFlatten"
    }

    fn output_var(&self) -> Option<&str> {
        self.output_var.as_deref()
    }

    fn col_names(&self) -> &[String] {
        &self.col_names
    }

    fn set_output_var(&mut self, var: String) {
        self.output_var = Some(var);
    }

    fn set_col_names(&mut self, names: Vec<String>) {
        self.col_names = names;
    }

    fn into_enum(self) -> LogicalNodeEnum {
        LogicalNodeEnum::Flatten(self)
    }
}

impl LogicalSingleInputNode for LogicalFlattenNode {
    fn input(&self) -> &LogicalNodeEnum {
        self.input.as_ref().expect("flatten input missing")
    }

    fn input_mut(&mut self) -> &mut LogicalNodeEnum {
        self.input.as_mut().expect("flatten input missing")
    }

    fn set_input(&mut self, input: LogicalNodeEnum) {
        self.input = Some(Box::new(input));
        self.deps.clear();
    }
}
