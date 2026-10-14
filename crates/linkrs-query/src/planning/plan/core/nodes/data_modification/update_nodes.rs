//! Update Operation Plan Nodes
//!
//! Provides the plan node for UPDATE VERTEX and UPDATE EDGE operations.

use crate::define_plan_node;

use super::info::UpdateTargetType;

define_plan_node! {
    pub struct UpdateNode {
        info: UpdateTargetType,
    }
    enum: Update
    input: ZeroInputNode
}

impl UpdateNode {
    pub fn new(id: i64, info: UpdateTargetType) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: vec!["updated".to_string()],
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &UpdateTargetType {
        &self.info
    }
}
