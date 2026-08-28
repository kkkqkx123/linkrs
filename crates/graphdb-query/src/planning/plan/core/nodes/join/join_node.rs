//! Implementation of connection nodes
//!
//! It includes various types of join nodes, such as inner joins, left joins, etc.

use crate::define_binary_input_node;
use crate::define_join_node;
use graphdb_core::types::ContextualExpression;

define_join_node! {
    pub struct InnerJoinNode {
        // Whether constant folding replaced part of the join keys.
        has_folded_expressions: bool,
    }
    enum: InnerJoin
}

impl InnerJoinNode {
    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        // Merge column names, avoiding duplicates
        let mut col_names = left.col_names().to_vec();
        let right_col_names = right.col_names();

        // Add right column names, skipping duplicates
        for col in right_col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                // If duplicate, add a suffix to make it unique
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }

        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: -1,
            left: Box::new(left),
            right: Box::new(right),
            hash_keys,
            probe_keys,
            deps,
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn right(&self) -> &crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
        &self.right
    }

    /// Whether constant folding replaced part of the join keys.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the join keys.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

define_join_node! {
    pub struct LeftJoinNode {
        // Whether constant folding replaced part of the join keys.
        has_folded_expressions: bool,
    }
    enum: LeftJoin
}

impl LeftJoinNode {
    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        // Merge column names, avoiding duplicates
        let mut col_names = left.col_names().to_vec();
        let right_col_names = right.col_names();

        // Add right column names, skipping duplicates
        for col in right_col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                // If duplicate, add a suffix to make it unique
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }

        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: -1,
            left: Box::new(left),
            right: Box::new(right),
            hash_keys,
            probe_keys,
            deps,
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    /// Whether constant folding replaced part of the join keys.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the join keys.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

define_binary_input_node! {
    pub struct CrossJoinNode {
    }
    enum: CrossJoin
    input: BinaryInputNode
}

impl CrossJoinNode {
    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        use crate::planning::plan::core::node_id_generator::next_node_id;

        // Merge column names, avoiding duplicates
        let mut col_names = left.col_names().to_vec();
        let right_col_names = right.col_names();

        // Add right column names, skipping duplicates
        for col in right_col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                // If duplicate, add a suffix to make it unique
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }

        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: next_node_id(),
            left: Box::new(left),
            right: Box::new(right),
            deps,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }
}

define_join_node! {
    pub struct FullOuterJoinNode {
        // Whether constant folding replaced part of the join keys.
        has_folded_expressions: bool,
    }
    enum: FullOuterJoin
}

impl FullOuterJoinNode {
    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let mut col_names = left.col_names().to_vec();
        let right_col_names = right.col_names();

        for col in right_col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }

        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: -1,
            left: Box::new(left),
            right: Box::new(right),
            hash_keys,
            probe_keys,
            deps,
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    /// Whether constant folding replaced part of the join keys.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the join keys.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

define_join_node! {
    pub struct RightJoinNode {
        // Whether constant folding replaced part of the join keys.
        has_folded_expressions: bool,
    }
    enum: RightJoin
}

impl RightJoinNode {
    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let mut col_names = left.col_names().to_vec();
        let right_col_names = right.col_names();

        for col in right_col_names {
            if !col_names.contains(col) {
                col_names.push(col.clone());
            } else {
                let mut idx = 1;
                let mut new_col = format!("{}_{}", col, idx);
                while col_names.contains(&new_col) {
                    idx += 1;
                    new_col = format!("{}_{}", col, idx);
                }
                col_names.push(new_col);
            }
        }

        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: -1,
            left: Box::new(left),
            right: Box::new(right),
            hash_keys,
            probe_keys,
            deps,
            has_folded_expressions: false,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn to_left_join(&self) -> LeftJoinNode {
        LeftJoinNode::new(
            (*self.right).clone(),
            (*self.left).clone(),
            self.probe_keys.clone(),
            self.hash_keys.clone(),
        )
        .expect("Failed to convert RightJoin to LeftJoin")
    }

    /// Whether constant folding replaced part of the join keys.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the join keys.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

define_join_node! {
    pub struct SemiJoinNode {
        anti: bool,
        // Whether constant folding replaced part of the join keys.
        has_folded_expressions: bool,
        // Optional residual condition for non-equi correlation (Mark-Join
        // form): evaluated over the combined left+right row, keeping the
        // left row when any right row satisfies it.
        join_condition: Option<ContextualExpression>,
    }
    enum: SemiJoin
}

impl SemiJoinNode {
    pub fn new_semi(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        Self::new(left, right, hash_keys, probe_keys, false)
    }

    pub fn new_anti(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        Self::new(left, right, hash_keys, probe_keys, true)
    }

    pub fn new(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
        anti: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let col_names = left.col_names().to_vec();
        let deps = vec![left.clone(), right.clone()];

        Ok(Self {
            id: -1,
            left: Box::new(left),
            right: Box::new(right),
            hash_keys,
            probe_keys,
            deps,
            anti,
            has_folded_expressions: false,
            join_condition: None,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    /// Create a Mark-Join style semi join carrying a residual non-equi
    /// condition evaluated over the combined left+right row.
    pub fn new_with_condition(
        left: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        right: crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        hash_keys: Vec<ContextualExpression>,
        probe_keys: Vec<ContextualExpression>,
        join_condition: ContextualExpression,
        anti: bool,
    ) -> Result<Self, crate::planning::planner::PlannerError> {
        let mut node = Self::new(left, right, hash_keys, probe_keys, anti)?;
        node.join_condition = Some(join_condition);
        Ok(node)
    }

    pub fn is_anti(&self) -> bool {
        self.anti
    }

    /// The residual non-equi join condition, if any.
    pub fn join_condition(&self) -> Option<&ContextualExpression> {
        self.join_condition.as_ref()
    }

    /// Whether constant folding replaced part of the join keys.
    pub fn has_folded_expressions(&self) -> bool {
        self.has_folded_expressions
    }

    /// Mark whether constant folding replaced part of the join keys.
    pub fn set_has_folded_expressions(&mut self, val: bool) {
        self.has_folded_expressions = val;
    }
}

pub type AntiJoinNode = SemiJoinNode;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::planning::plan::core::nodes::control_flow::start_node::StartNode;

    fn create_test_start_node(_id: i64) -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn test_inner_join_node_creation() {
        let left = create_test_start_node(1);
        let right = create_test_start_node(2);
        let hash_keys = vec![];
        let probe_keys = vec![];

        let node = InnerJoinNode::new(left, right, hash_keys, probe_keys);
        assert!(node.is_ok());
        let node = node.expect("InnerJoinNode creation should succeed!");
        assert_eq!(node.type_name(), "InnerJoinNode");
        assert_eq!(node.id(), -1);
    }

    #[test]
    fn test_left_join_node_creation() {
        let left = create_test_start_node(1);
        let right = create_test_start_node(2);
        let hash_keys = vec![];
        let probe_keys = vec![];

        let node = LeftJoinNode::new(left, right, hash_keys, probe_keys);
        assert!(node.is_ok());
        let node = node.expect("LeftJoinNode creation should succeed!");
        assert_eq!(node.type_name(), "LeftJoinNode");
        assert_eq!(node.id(), -1);
    }

    #[test]
    fn test_cross_join_node_creation() {
        let left = create_test_start_node(1);
        let right = create_test_start_node(2);

        let node = CrossJoinNode::new(left, right);
        assert!(node.is_ok());
        let node = node.expect("CrossJoinNode creation should succeed!");
        assert_eq!(node.type_name(), "CrossJoinNode");
        assert!(node.id() > 0); // Dynamic ID generated by next_node_id()
    }
}
