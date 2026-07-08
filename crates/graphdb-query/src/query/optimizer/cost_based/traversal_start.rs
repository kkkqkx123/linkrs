//! Traverse the starting point selector module
//!
//! Used to select the optimal starting point for graph traversal.

use std::collections::HashMap;
use std::sync::Arc;

use crate::core::types::BinaryOperator;
use crate::core::types::Expression;
use crate::query::optimizer::cost::{CostCalculator, SelectivityEstimator};
use crate::query::parser::ast::pattern::{
    EdgePattern, NodePattern, PathElement, PathPattern, Pattern, VariablePattern,
};

/// Traverse the starting point selector
#[derive(Debug)]
pub struct TraversalStartSelector {
    cost_calculator: Arc<CostCalculator>,
    selectivity_estimator: Arc<SelectivityEstimator>,
    /// The variable binding context is used to parse variable patterns.
    variable_context: HashMap<String, NodePattern>,
}

/// Candidate starting point information
#[derive(Debug, Clone)]
pub struct CandidateStart {
    /// Node mode
    pub node_pattern: NodePattern,
    /// Estimate the number of starting nodes
    pub estimated_start_nodes: u64,
    /// Estimated cost
    pub estimated_cost: f64,
    /// Reason for the choice
    pub reason: SelectionReason,
}

/// select a reason
#[derive(Debug, Clone)]
pub enum SelectionReason {
    /// Explicit VID specification
    ExplicitVid,
    /// Highly selective index
    HighSelectivityIndex {
        /// Selective values
        selectivity: f64,
    },
    /// Tag Index
    TagIndex {
        /// Number of vertices
        vertex_count: u64,
    },
    /// Full table scan
    FullScan {
        /// number of vertices
        vertex_count: u64,
    },
    /// Variable binding
    VariableBinding {
        /// Variable name
        variable_name: String,
    },
}

impl TraversalStartSelector {
    /// Create a new selector for choosing the starting point of the traversal.
    pub fn new(
        cost_calculator: Arc<CostCalculator>,
        selectivity_estimator: Arc<SelectivityEstimator>,
    ) -> Self {
        Self {
            cost_calculator,
            selectivity_estimator,
            variable_context: HashMap::new(),
        }
    }

    /// Select the optimal starting point for the traversal from the available patterns.
    pub fn select_start_node(&self, pattern: &Pattern) -> Option<CandidateStart> {
        let candidates = self.evaluate_pattern(pattern);

        if candidates.is_empty() {
            return None;
        }

        // Choose the starting point that incurs the lowest cost.
        candidates.into_iter().min_by(|a, b| {
            a.estimated_cost
                .partial_cmp(&b.estimated_cost)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// All candidate nodes in the evaluation mode
    fn evaluate_pattern(&self, pattern: &Pattern) -> Vec<CandidateStart> {
        let mut candidates = Vec::new();

        match pattern {
            Pattern::Node(node) => {
                if let Some(candidate) = self.evaluate_node(node) {
                    candidates.push(candidate);
                }
            }
            Pattern::Path(path) => {
                candidates.extend(self.evaluate_path(path));
            }
            Pattern::Edge(edge) => {
                // The edge mode can be converted into the node mode as a starting point.
                // Obtain the source or target node of the edge as a candidate.
                candidates.extend(self.evaluate_edge_as_start(edge));
            }
            Pattern::Variable(var) => {
                // The variable pattern attempts to parse the value from the context.
                if let Some(candidate) = self.evaluate_variable(var) {
                    candidates.push(candidate);
                }
            }
        }

        candidates
    }

    /// Evaluate all nodes in the path pattern.
    fn evaluate_path(&self, path: &PathPattern) -> Vec<CandidateStart> {
        let mut candidates = Vec::new();

        for element in &path.elements {
            match element {
                PathElement::Node(node) => {
                    if let Some(candidate) = self.evaluate_node(node) {
                        candidates.push(candidate);
                    }
                }
                PathElement::Edge(edge) => {
                    // The edge mode can be converted into the node mode as a starting point.
                    candidates.extend(self.evaluate_edge_as_start(edge));
                }
                PathElement::Alternative(patterns) => {
                    // Evaluate each of the alternative models.
                    for pattern in patterns {
                        candidates.extend(self.evaluate_pattern(pattern));
                    }
                }
                PathElement::Optional(inner) => match inner.as_ref() {
                    PathElement::Node(node) => {
                        if let Some(candidate) = self.evaluate_node(node) {
                            candidates.push(candidate);
                        }
                    }
                    PathElement::Edge(edge) => {
                        candidates.extend(self.evaluate_edge_as_start(edge));
                    }
                    _ => {}
                },
                PathElement::Repeated(inner, _) => match inner.as_ref() {
                    PathElement::Node(node) => {
                        if let Some(candidate) = self.evaluate_node(node) {
                            candidates.push(candidate);
                        }
                    }
                    PathElement::Edge(edge) => {
                        candidates.extend(self.evaluate_edge_as_start(edge));
                    }
                    _ => {}
                },
            }
        }

        candidates
    }

    /// Evaluate the edge mode as a starting candidate.
    ///
    /// The border mode itself cannot be used directly as a starting point for traversal, but it can be converted in the following way:
    /// 1. If there is a type of edge, the number of edges can be estimated as a reference.
    /// 2. Return a virtual node representation indicating that the process can start from either end of the edge.
    fn evaluate_edge_as_start(&self, edge: &EdgePattern) -> Vec<CandidateStart> {
        let mut candidates = Vec::new();

        // Edge modes cannot be used directly as a starting point, but we can create a virtual node that represents the endpoints of the edges.
        // This can be used in the actual planning of queries to determine the direction of the traversal.

        // If there is type information associated with the edges, we can create candidates based on the statistics derived from those edges.
        if let Some(edge_type) = edge.edge_types.first() {
            let edge_stats = self
                .cost_calculator
                .statistics_manager()
                .get_edge_stats(edge_type);

            if let Some(stats) = edge_stats {
                // Create a virtual node pattern to represent the source endpoint of an edge.
                let virtual_node = NodePattern {
                    span: edge.span,
                    variable: edge.variable.clone(),
                    labels: Vec::new(), // The edges do not have labels, but they may have a type.
                    properties: edge.properties.clone(),
                    predicates: edge.predicates.clone(),
                };

                // Cost estimation based on edge statistics
                let estimated_cost = stats.estimate_expand_cost(1);

                candidates.push(CandidateStart {
                    node_pattern: virtual_node,
                    estimated_start_nodes: stats.unique_src_vertices,
                    estimated_cost,
                    reason: SelectionReason::TagIndex {
                        vertex_count: stats.unique_src_vertices,
                    },
                });
            }
        }

        candidates
    }

    /// Evaluating variable patterns
    ///
    /// Find the node pattern corresponding to the variable from the variable context.
    fn evaluate_variable(&self, var: &VariablePattern) -> Option<CandidateStart> {
        // Search for a variable in the context of the variables.
        if let Some(node) = self.variable_context.get(&var.name) {
            return self.evaluate_node(node);
        }

        // If the variable is not bound, create a placeholder candidate.
        // This can happen in the early stages of query planning.
        let placeholder_node = NodePattern {
            span: var.span,
            variable: Some(var.name.clone()),
            labels: Vec::new(),
            properties: None,
            predicates: Vec::new(),
        };

        Some(CandidateStart {
            node_pattern: placeholder_node,
            estimated_start_nodes: 1000, // Default estimate
            estimated_cost: 1000.0,      // A high cost indicates uncertainty.
            reason: SelectionReason::VariableBinding {
                variable_name: var.name.clone(),
            },
        })
    }

    /// Evaluating a single node
    fn evaluate_node(&self, node: &NodePattern) -> Option<CandidateStart> {
        // Check whether there is an explicit VID (either through an attribute or a predicate).
        if let Some(vid_selectivity) = self.check_explicit_vid(node) {
            return Some(CandidateStart {
                node_pattern: node.clone(),
                estimated_start_nodes: 1,
                estimated_cost: 1.0 * vid_selectivity,
                reason: SelectionReason::ExplicitVid,
            });
        }

        // Obtain tag information
        let tag_name = node.labels.first()?;

        // Calculating selectivity
        let selectivity = self.calculate_node_selectivity(node, tag_name);

        // Obtain the number of vertices
        let vertex_count = self
            .cost_calculator
            .statistics_manager()
            .get_vertex_count(tag_name);

        // Calculate the estimated number of starting nodes.
        let estimated_start_nodes = ((vertex_count as f64 * selectivity) as u64).max(1);

        // Calculating the scanning cost
        let estimated_cost = if selectivity < 0.1 {
            // Use index scanning
            self.cost_calculator
                .calculate_index_scan_cost(tag_name, "", selectivity)
        } else {
            // Full table scan
            self.cost_calculator.calculate_scan_vertices_cost(tag_name)
        };

        let reason = if selectivity < 0.1 {
            SelectionReason::HighSelectivityIndex { selectivity }
        } else if vertex_count > 0 {
            SelectionReason::TagIndex { vertex_count }
        } else {
            SelectionReason::FullScan { vertex_count }
        };

        Some(CandidateStart {
            node_pattern: node.clone(),
            estimated_start_nodes,
            estimated_cost,
            reason,
        })
    }

    /// Check whether there is an explicit VID (Video Identifier).
    ///
    /// Check whether the attributes and predicates in the node mode contain explicit VID conditions, for example:
    /// - id(v) == "xxx"
    /// - v.id == "xxx"
    /// - {id: "xxx"}
    fn check_explicit_vid(&self, node: &NodePattern) -> Option<f64> {
        // Check whether there is a VID condition in the properties.
        if let Some(props) = &node.properties {
            // Check whether the attribute expression contains the id field.
            if self.has_vid_condition_ctx(props) {
                return Some(0.01); // The VID conditions are highly selective.
            }
        }

        // Check whether there are VID conditions in the predicate.
        for predicate in &node.predicates {
            if self.has_vid_condition_ctx(predicate) {
                return Some(0.01);
            }
        }

        None
    }

    /// Check whether the expression contains a VID condition.
    ///
    /// Identify the following pattern:
    /// - id(v) == value
    /// - v.id == value
    /// - {id: value}
    fn has_vid_condition_ctx(&self, expr: &crate::core::types::expr::ContextualExpression) -> bool {
        match expr.expression() {
            Some(meta) => self.has_vid_condition(meta.inner()),
            None => false,
        }
    }

    /// Check whether the expression contains VID conditions
    ///
    /// Identify the following patterns:
    /// - id(v) == value
    /// - v.id == value
    /// - {id: value}
    fn has_vid_condition(&self, expr: &Expression) -> bool {
        match expr {
            // 检查是否包含 id() 函数调用
            Expression::Function { name, args } => {
                let name_upper = name.to_uppercase();
                if name_upper == "ID" && !args.is_empty() {
                    return true;
                }
                // Recursive check of parameters
                args.iter().any(|arg| self.has_vid_condition(arg))
            }
            // Check whether the property access is of the .id type.
            Expression::Property { property, .. } => {
                if property.eq_ignore_ascii_case("id") {
                    return true;
                }
                false
            }
            // Check whether the binary operation contains a VID condition.
            Expression::Binary { left, right, op } => {
                // Check whether it is an equivalent comparison and whether it contains the VID.
                if matches!(op, BinaryOperator::Equal | BinaryOperator::NotEqual)
                    && (self.is_vid_expression(left) || self.is_vid_expression(right))
                {
                    return true;
                }
                // Recursive checking
                self.has_vid_condition(left) || self.has_vid_condition(right)
            }
            // Check whether the Map contains an id field.
            Expression::Map(pairs) => pairs.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("id") || self.has_vid_condition(value)
            }),
            // For other types of expressions, recursively check the subexpressions.
            _ => expr
                .children()
                .iter()
                .any(|child| self.has_vid_condition(child)),
        }
    }

    /// Determine whether the expression is a VID expression.
    ///
    /// 识别 id() 函数调用或 .id 属性访问
    fn is_vid_expression(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Function { name, args } => {
                let name_upper = name.to_uppercase();
                name_upper == "ID" && !args.is_empty()
            }
            Expression::Property { property, .. } => property.eq_ignore_ascii_case("id"),
            _ => false,
        }
    }

    /// The selectivity of computing nodes
    fn calculate_node_selectivity(&self, node: &NodePattern, tag_name: &str) -> f64 {
        let mut selectivity = 1.0;

        // Estimating selectivity from dependent conditional probabilities
        if let Some(props) = &node.properties {
            if let Some(expr) = props.get_expression() {
                let prop_selectivity = self
                    .selectivity_estimator
                    .estimate_from_expression(&expr, Some(tag_name));
                selectivity *= prop_selectivity;
            }
        }

        // Estimating selectivity from predicate conditions
        for predicate in &node.predicates {
            if let Some(expr) = predicate.get_expression() {
                let pred_selectivity = self
                    .selectivity_estimator
                    .estimate_from_expression(&expr, Some(tag_name));
                selectivity *= pred_selectivity;
            }
        }

        selectivity
    }

    /// Add variable bindings to the context.
    ///
    /// Used to establish a mapping between variables and node patterns during the query planning process.
    pub fn bind_variable(&mut self, var_name: String, node: NodePattern) {
        self.variable_context.insert(var_name, node);
    }

    /// Clear the variable context
    pub fn clear_context(&mut self) {
        self.variable_context.clear();
    }
}

impl Clone for TraversalStartSelector {
    fn clone(&self) -> Self {
        Self {
            cost_calculator: self.cost_calculator.clone(),
            selectivity_estimator: self.selectivity_estimator.clone(),
            variable_context: self.variable_context.clone(),
        }
    }
}
