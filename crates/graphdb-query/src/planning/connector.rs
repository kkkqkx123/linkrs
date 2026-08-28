//! Connector module
//!
//! Provide the functionality to connect the planned nodes, including inner joins, left joins, and cross joins.

use crate::planning::plan::{PlanNodeEnum, SubPlan};
use crate::planning::planner::PlannerError;
use crate::QueryContext;
use std::collections::HashSet;

/// Plan Connector
///
/// Used to connect two sub-plans, similar to the SegmentsConnector implementation in C++.
pub struct SegmentsConnector;

impl SegmentsConnector {
    /// Create an inner join
    ///
    /// Perform an inner join on the two plans, using the specified join key.
    pub fn inner_join(
        _qctx: &QueryContext,
        left: SubPlan,
        right: SubPlan,
        _inter_aliases: HashSet<&str>,
    ) -> Result<SubPlan, PlannerError> {
        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        let _col_names = left_root.col_names().to_vec();
        let join_node = PlanNodeEnum::InnerJoin(
            crate::planning::plan::core::nodes::InnerJoinNode::new(
                left_root.clone(),
                right_root.clone(),
                vec![],
                vec![],
            )
            .map_err(|e| {
                PlannerError::JoinFailed(format!("Inner join node creation failed: {}", e))
            })?,
        );

        Ok(SubPlan {
            root: Some(join_node),
            tail: left.tail.or(right.tail),
            logical_root: None,
        })
    }

    /// Create a left join
    ///
    /// Perform a left join on the two plans, for use in scenarios such as an optional MATCH operation.
    pub fn left_join(
        _qctx: &QueryContext,
        left: SubPlan,
        right: SubPlan,
        _inter_aliases: HashSet<&str>,
    ) -> Result<SubPlan, PlannerError> {
        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        let join_node = PlanNodeEnum::LeftJoin(
            crate::planning::plan::core::nodes::LeftJoinNode::new(
                left_root.clone(),
                right_root.clone(),
                vec![],
                vec![],
            )
            .map_err(|e| {
                PlannerError::JoinFailed(format!("Left join node creation failed: {}", e))
            })?,
        );

        Ok(SubPlan {
            root: Some(join_node),
            tail: left.tail.or(right.tail),
            logical_root: None,
        })
    }

    /// Create a cross-link
    ///
    /// Connect the two plans using the Cartesian product.
    pub fn cross_join(left: SubPlan, right: SubPlan) -> Result<SubPlan, PlannerError> {
        let left_root = match left.root {
            Some(ref r) => r,
            None => return Ok(right),
        };

        let right_root = match right.root {
            Some(ref r) => r,
            None => return Ok(left),
        };

        let join_node = PlanNodeEnum::CrossJoin(
            crate::planning::plan::core::nodes::CrossJoinNode::new(
                left_root.clone(),
                right_root.clone(),
            )
            .map_err(|e| {
                PlannerError::JoinFailed(format!("Cross join node creation failed: {}", e))
            })?,
        );

        Ok(SubPlan {
            root: Some(join_node),
            tail: left.tail.or(right.tail),
            logical_root: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::QueryRequestContext;
    use std::sync::Arc;

    fn create_test_query_context() -> QueryContext {
        let rctx = Arc::new(QueryRequestContext::new("TEST".to_string()));
        QueryContext::new(rctx)
    }

    #[test]
    fn test_inner_join() {
        let left = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));
        let right = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));

        let result = SegmentsConnector::inner_join(
            &create_test_query_context(),
            left,
            right,
            HashSet::new(),
        );
        assert!(result.is_ok());
        assert!(result
            .expect("Expected planner result to exist")
            .root
            .is_some());
    }

    #[test]
    fn test_left_join() {
        let left = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));
        let right = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));

        let result =
            SegmentsConnector::left_join(&create_test_query_context(), left, right, HashSet::new());
        assert!(result.is_ok());
        assert!(result
            .expect("Expected planner result to exist")
            .root
            .is_some());
    }

    #[test]
    fn test_cross_join() {
        let left = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));
        let right = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));

        let result = SegmentsConnector::cross_join(left, right);
        assert!(result.is_ok());
        assert!(result
            .expect("Expected planner result to exist")
            .root
            .is_some());
    }

    #[test]
    fn test_inner_join_with_empty_left() {
        let left = SubPlan::new(None, None);
        let right = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));

        let result = SegmentsConnector::inner_join(
            &create_test_query_context(),
            left,
            right,
            HashSet::new(),
        );
        assert!(result.is_ok());
        assert!(result
            .expect("Expected planner result to exist")
            .root
            .is_some());
    }

    #[test]
    fn test_cross_join_with_empty_right() {
        let left = SubPlan::from_single_node(PlanNodeEnum::Start(
            crate::planning::plan::core::nodes::StartNode::new(),
        ));
        let right = SubPlan::new(None, None);

        let result = SegmentsConnector::cross_join(left, right);
        assert!(result.is_ok());
        assert!(result
            .expect("Expected planner result to exist")
            .root
            .is_some());
    }
}
