//! Rework the definition of the context.
//!
//! Define the RewriteContext structure to manage the state during the rewriting process.
//! This is a simplified version that has been separated from the optimizer layer, focusing on the requirements for heuristic rewriting rules.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::query::planning::plan::PlanNodeEnum;
use crate::query::validator::context::ExpressionAnalysisContext;

/// Rewrite the context
///
/// Manage the status and node information during the rewriting process.
/// Compared to the OptContext of the optimizer, this is a lightweight version.
/// It does not include features specific to optimizers, such as caches for statistical information or cost calculations.
#[derive(Debug)]
pub struct RewriteContext {
    /// Node ID Counter – Generates unique node IDs
    node_id_counter: usize,
    /// Mapping of plan nodes to IDs
    plan_node_to_id: RefCell<HashMap<usize, usize>>,
    /// Mapping from IDs to planned nodes
    nodes_by_id: RefCell<HashMap<usize, Rc<RefCell<PlanNodeWrapper>>>>,
    /// Expression context
    expr_context: Arc<ExpressionAnalysisContext>,
}

/// Plan Node Wrapper
///
/// Wrap the PlanNodeEnum and add the necessary metadata for the override.
#[derive(Debug, Clone)]
pub struct PlanNodeWrapper {
    pub id: usize,
    pub plan_node: PlanNodeEnum,
    pub dependencies: Vec<usize>,
}

impl PlanNodeWrapper {
    pub fn new(id: usize, plan_node: PlanNodeEnum) -> Self {
        Self {
            id,
            plan_node,
            dependencies: Vec::new(),
        }
    }
}

impl Default for RewriteContext {
    fn default() -> Self {
        Self {
            node_id_counter: 0,
            plan_node_to_id: RefCell::new(HashMap::new()),
            nodes_by_id: RefCell::new(HashMap::new()),
            expr_context: Arc::new(ExpressionAnalysisContext::new()),
        }
    }
}

impl RewriteContext {
    /// Create a new rewriting context (using the default ExpressionContext).
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new rewriting context (using the specified ExpressionContext).
    pub fn with_expr_context(expr_context: Arc<ExpressionAnalysisContext>) -> Self {
        Self {
            node_id_counter: 0,
            plan_node_to_id: RefCell::new(HashMap::new()),
            nodes_by_id: RefCell::new(HashMap::new()),
            expr_context,
        }
    }

    /// Assign a new node ID
    pub fn allocate_node_id(&mut self) -> usize {
        let id = self.node_id_counter;
        self.node_id_counter += 1;
        id
    }

    /// Register a plan node
    pub fn register_node(
        &mut self,
        node_id: usize,
        plan_node: PlanNodeEnum,
    ) -> Rc<RefCell<PlanNodeWrapper>> {
        let wrapper = Rc::new(RefCell::new(PlanNodeWrapper::new(node_id, plan_node)));
        self.nodes_by_id
            .borrow_mut()
            .insert(node_id, wrapper.clone());
        wrapper
    }

    /// Find a node by its ID
    pub fn find_node_by_id(&self, id: usize) -> Option<Rc<RefCell<PlanNodeWrapper>>> {
        self.nodes_by_id.borrow().get(&id).cloned()
    }

    /// Add node mapping
    pub fn add_plan_node_mapping(&self, plan_node_id: usize, rewrite_node_id: usize) {
        self.plan_node_to_id
            .borrow_mut()
            .insert(plan_node_id, rewrite_node_id);
    }

    /// Find the rewrite node ID by using the planned node ID.
    pub fn find_rewrite_id_by_plan_id(&self, plan_node_id: usize) -> Option<usize> {
        self.plan_node_to_id.borrow().get(&plan_node_id).copied()
    }

    /// Get the current node count.
    pub fn node_count(&self) -> usize {
        self.node_id_counter
    }

    /// Obtain the context of the expression.
    pub fn expr_context(&self) -> Arc<ExpressionAnalysisContext> {
        self.expr_context.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode;

    #[test]
    fn test_context_creation() {
        let ctx = RewriteContext::new();
        assert_eq!(ctx.node_count(), 0);
    }

    #[test]
    fn test_allocate_node_id() {
        let mut ctx = RewriteContext::new();
        assert_eq!(ctx.allocate_node_id(), 0);
        assert_eq!(ctx.allocate_node_id(), 1);
        assert_eq!(ctx.allocate_node_id(), 2);
    }

    #[test]
    fn test_register_and_find_node() {
        let mut ctx = RewriteContext::new();
        let node_id = ctx.allocate_node_id();
        let plan_node = PlanNodeEnum::ScanVertices(ScanVerticesNode::new(1, "default"));

        let wrapper = ctx.register_node(node_id, plan_node);
        assert_eq!(wrapper.borrow().id, node_id);

        let found = ctx.find_node_by_id(node_id);
        assert!(found.is_some());
        assert_eq!(found.expect("Failed to find node").borrow().id, node_id);
    }
}
