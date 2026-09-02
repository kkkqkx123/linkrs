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
