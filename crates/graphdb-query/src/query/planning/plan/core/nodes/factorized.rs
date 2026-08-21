//! Factorized execution plan nodes.
//!
//! Logical counterparts to the physical `FactorizedSpec` operators:
//! - `SemiMaskerNode` - semi-join mask pushed into Expand (SEMI_MASKER)
//! - `MultiplicityReducerNode` - factorized dedup (MULTIPLICITY_REDUCER)
//! - `NodeLabelFilterNode` - factorized label pruning (NODE_LABEL_FILTER)
//!
//! These nodes are logical only; the physical planner lowers them to
//! `FactorizedSpec` variants.  They reuse the existing `Operation` category
//! so that `supports_parallelism()` and `is_leaf()` semantics stay correct.

use crate::define_plan_node_with_deps;

// ── SemiMaskerNode ──────────────────────────────────────────────────────────

define_plan_node_with_deps! {
    pub struct SemiMaskerNode {
        key_column: String,
        mask_keys: Vec<String>,
        keep_match: bool,
    }
    enum: SemiMasker
    input: SingleInputNode
}

impl SemiMaskerNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        key_column: String,
        mask_keys: Vec<String>,
        keep_match: bool,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();
        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            key_column,
            mask_keys,
            keep_match,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn key_column(&self) -> &str {
        &self.key_column
    }

    pub fn mask_keys(&self) -> &[String] {
        &self.mask_keys
    }

    pub fn keep_match(&self) -> bool {
        self.keep_match
    }
}

// ── MultiplicityReducerNode ───────────────────────────────────────────────

define_plan_node_with_deps! {
    pub struct MultiplicityReducerNode {
        group_key_columns: Vec<String>,
    }
    enum: MultiplicityReducer
    input: SingleInputNode
}

impl MultiplicityReducerNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        group_key_columns: Vec<String>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();
        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            group_key_columns,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn group_key_columns(&self) -> &[String] {
        &self.group_key_columns
    }
}

// ── NodeLabelFilterNode ───────────────────────────────────────────────────

define_plan_node_with_deps! {
    pub struct NodeLabelFilterNode {
        label_column: String,
        allowed_labels: Vec<String>,
    }
    enum: NodeLabelFilter
    input: SingleInputNode
}

impl NodeLabelFilterNode {
    pub fn new(
        input: crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum,
        label_column: String,
        allowed_labels: Vec<String>,
    ) -> Result<Self, crate::query::planning::planner::PlannerError> {
        let col_names = input.col_names().to_vec();
        Ok(Self {
            id: -1,
            input: Some(Box::new(input.clone())),
            deps: vec![input],
            label_column,
            allowed_labels,
            output_var: None,
            col_names,
            column_types: vec![],
        })
    }

    pub fn label_column(&self) -> &str {
        &self.label_column
    }

    pub fn allowed_labels(&self) -> &[String] {
        &self.allowed_labels
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    fn start_node() -> PlanNodeEnum {
        PlanNodeEnum::Start(StartNode::new())
    }

    #[test]
    fn semi_masker_creation() {
        let node = SemiMaskerNode::new(start_node(), "id".to_string(), vec!["a".to_string()], true)
            .expect("should build");
        assert_eq!(node.key_column(), "id");
        assert_eq!(node.mask_keys(), &["a".to_string()]);
        assert!(node.keep_match());
    }

    #[test]
    fn multiplicity_reducer_creation() {
        let node = MultiplicityReducerNode::new(start_node(), vec!["id".to_string()])
            .expect("should build");
        assert_eq!(node.group_key_columns(), &["id".to_string()]);
    }

    #[test]
    fn node_label_filter_creation() {
        let node = NodeLabelFilterNode::new(
            start_node(),
            "label".to_string(),
            vec!["Person".to_string()],
        )
        .expect("should build");
        assert_eq!(node.label_column(), "label");
    }
}
