//! Implementation of the starting node
//!
//! StartNode is used to represent the starting point of the execution plan.

use crate::define_plan_node;

define_plan_node! {
    pub struct StartNode {
    }
    enum: Start
    input: ZeroInputNode
}

impl Default for StartNode {
    fn default() -> Self {
        Self::new()
    }
}

impl StartNode {
    pub fn new() -> Self {
        Self {
            id: -1,
            output_var: None,
            col_names: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_node_creation() {
        let start_node = StartNode::new();

        assert_eq!(start_node.type_name(), "StartNode");
        assert_eq!(start_node.col_names().len(), 0);
    }

    #[test]
    fn test_start_node_mutable() {
        let mut start_node = StartNode::new();

        start_node.set_col_names(vec!["test".to_string()]);
        assert_eq!(start_node.col_names().len(), 1);
        assert_eq!(start_node.col_names()[0], "test");
    }

    #[test]
    fn test_start_node_traits() {
        let start_node = StartNode::new();

        assert_eq!(start_node.id(), -1);
        assert!(start_node.output_var().is_none());
    }

    #[test]
    fn test_start_node_clone() {
        let mut start_node = StartNode::new();
        start_node.set_col_names(vec!["col1".to_string(), "col2".to_string()]);

        let cloned = start_node.clone_plan_node();
        assert_ne!(cloned.id(), -1);
        assert_eq!(cloned.col_names().len(), 2);

        let cloned_with_id = start_node.clone_with_new_id(100);
        assert_eq!(cloned_with_id.id(), 100);
    }
}
