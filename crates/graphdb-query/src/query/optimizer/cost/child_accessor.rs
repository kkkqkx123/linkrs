//! Child node accessor
//!
//! Provide a unified way to access the subnodes of various plan nodes.

use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::PlanNodeEnum;

/// Child Node Accessor Trait
///
/// Provide a unified sub-node access interface for different types of plan nodes.
pub trait ChildAccessor {
    /// Get the number of child nodes
    fn child_count(&self) -> usize;

    /// Obtain a reference to the variable child node at the specified index.
    fn get_child_mut(&mut self, index: usize) -> Option<&mut PlanNodeEnum>;
}

impl ChildAccessor for PlanNodeEnum {
    fn child_count(&self) -> usize {
        self.children().len()
    }

    fn get_child_mut(&mut self, index: usize) -> Option<&mut PlanNodeEnum> {
        match self {
            // ==================== Dual-Input Nodes ====================
            PlanNodeEnum::InnerJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },
            PlanNodeEnum::LeftJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },
            PlanNodeEnum::CrossJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },
            PlanNodeEnum::HashInnerJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },
            PlanNodeEnum::HashLeftJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },
            PlanNodeEnum::FullOuterJoin(n) => match index {
                0 => Some(n.left_input_mut()),
                1 => Some(n.right_input_mut()),
                _ => None,
            },

            // ==================== Single Input Node ====================
            PlanNodeEnum::Project(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Filter(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Sort(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Limit(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::TopN(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Sample(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Dedup(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::DataCollect(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Aggregate(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Unwind(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Assign(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::PatternApply(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::RollUpApply(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Traverse(n) => {
                if index == 0 {
                    Some(n.input_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Union(n) => n.dependencies_mut().get_mut(index),
            PlanNodeEnum::Minus(n) => n.dependencies_mut().get_mut(index),
            PlanNodeEnum::Intersect(n) => n.dependencies_mut().get_mut(index),

            // ==================== Multiple input nodes ====================
            PlanNodeEnum::Expand(n) => n.inputs_mut().get_mut(index),
            PlanNodeEnum::ExpandAll(n) => n.inputs_mut().get_mut(index),
            PlanNodeEnum::AppendVertices(n) => n.inputs_mut().get_mut(index),
            PlanNodeEnum::GetVertices(n) => n.inputs_mut().get_mut(index),
            PlanNodeEnum::GetNeighbors(n) => n.inputs_mut().get_mut(index),

            // ==================== Control Flow Nodes ====================
            PlanNodeEnum::Loop(n) => {
                if index == 0 {
                    n.body_mut().as_mut().map(|b| b.as_mut())
                } else {
                    None
                }
            }
            PlanNodeEnum::Select(n) => match index {
                0 => n.if_branch_mut().as_mut().map(|b| b.as_mut()),
                1 => n.else_branch_mut().as_mut().map(|b| b.as_mut()),
                _ => None,
            },

            // ==================== No input nodes ====================
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    #[test]
    fn test_scan_vertices_child_count() {
        let scan = ScanVerticesNode::new(1, "test_space");
        let node = PlanNodeEnum::ScanVertices(scan);
        assert_eq!(node.child_count(), 0);
    }

    #[test]
    fn test_scan_vertices_get_child() {
        let scan = ScanVerticesNode::new(1, "test_space");
        let mut node = PlanNodeEnum::ScanVertices(scan);
        assert!(node.get_child_mut(0).is_none());
    }
}
