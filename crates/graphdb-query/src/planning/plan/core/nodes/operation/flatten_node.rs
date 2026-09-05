use crate::define_plan_node_with_deps;

define_plan_node_with_deps! {
    pub struct FlattenNode {
        group_pos: u32,
        group_columns: Vec<String>,
        expected_groups: Option<u32>,
        schema_snapshot: Option<String>,
    }
    enum: Flatten
    input: SingleInputNode
}

impl FlattenNode {
    pub fn new(
        input: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        group_pos: u32,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();
        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            group_pos,
            group_columns: Vec::new(),
            expected_groups: None,
            schema_snapshot: None,
            output_var: None,
            col_names,
            column_types: Vec::new(),
        })
    }

    pub fn group_pos(&self) -> u32 {
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

    pub fn schema_snapshot(&self) -> Option<&str> {
        self.schema_snapshot.as_deref()
    }

    pub fn set_schema_snapshot(&mut self, snapshot: String) {
        self.schema_snapshot = Some(snapshot);
    }
}
