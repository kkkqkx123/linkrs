//! Plan Node Fingerprint Calculation Module
//!
//! Provide a function for calculating the structural fingerprint of plan nodes, which is used to identify equivalent sub-plans.
//! Sub-plans with the same structure will generate the same fingerprint values.
//!
//! ## Design Specifications
//!
//! The fingerprint hashes the node type, the structure of the child nodes,
//! and the node configuration (filter condition, projection columns, sort
//! items, scan tag / projected properties).  Two sub-plans collide only when
//! both their structure and their configuration match, so the fingerprint is
//! a faithful equality proxy for both duplicate-sub-plan detection
//! (materialized CTEs) and batch cycle/convergence detection.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::core::types::expr::ContextualExpression;
use crate::query::planning::plan::core::nodes::{BinaryInputNode, PlanNodeEnum, SingleInputNode};

/// Plan node fingerprint
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlanFingerprint(pub u64);

impl PlanFingerprint {
    /// Create a new fingerprint.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Obtaining fingerprint values
    pub fn value(&self) -> u64 {
        self.0
    }
}

/// Fingerprint calculator
///
/// Use a stable hashing algorithm to calculate the structural fingerprint of the planned nodes.
/// Sub-plans with the same structure will generate the same fingerprint values.
///
/// ## Hashed Content
///
/// - Node type (enumeration discriminator) and child structure.
/// - Node configuration: Filter condition, Project columns, Sort items,
///   Scan tag / edge type / projected properties, plus limit amounts.
///
/// Node configuration is hashed so that plans which differ only in their
/// configuration (e.g. two Filter predicates) do not collide; this keeps
/// cycle/oscillation detection in the batch optimizer truthful and prevents
/// duplicate-sub-plan detection from merging distinct operators.
#[derive(Debug, Clone)]
pub struct FingerprintCalculator;

impl Default for FingerprintCalculator {
    fn default() -> Self {
        Self
    }
}

impl FingerprintCalculator {
    /// Create a new fingerprint calculator
    pub fn new() -> Self {
        Self
    }

    /// Calculate the structural fingerprint of the planning node.
    ///
    /// # Parameters
    /// `node`: The planned execution node.
    ///
    /// # Return
    /// The structural fingerprint of a node
    ///
    /// # Algorithms
    /// Hash node type (determined using an enumeration discriminator)
    /// 2. Recursive Hashing of Subnode Fingerprints
    /// 3. Hash the node configuration (condition/columns/sort items/scans)
    pub fn calculate_fingerprint(&self, node: &PlanNodeEnum) -> PlanFingerprint {
        let mut hasher = DefaultHasher::new();

        // Hash node type
        std::mem::discriminant(node).hash(&mut hasher);

        // Hash child node fingerprint
        self.hash_children(node, &mut hasher);

        // Hash node configuration
        self.hash_node_config(node, &mut hasher);

        PlanFingerprint::new(hasher.finish())
    }

    /// Hash the configuration of a node (operator parameters) into the
    /// fingerprint.  Two nodes that differ only in their configuration must
    /// produce different fingerprints, otherwise the cycle/convergence
    /// detection in the batch optimizer can mistake a configuration change
    /// for a no-op, and duplicate-sub-plan detection can merge unequal plans.
    fn hash_node_config(&self, node: &PlanNodeEnum, hasher: &mut DefaultHasher) {
        use crate::query::planning::plan::core::nodes::*;

        match node {
            PlanNodeEnum::Filter(n) => self.hash_expression(n.condition(), hasher),
            PlanNodeEnum::Project(n) => {
                for col in n.columns() {
                    col.alias.hash(hasher);
                    self.hash_expression(&col.expression, hasher);
                }
            }
            PlanNodeEnum::Sort(n) => {
                for item in n.sort_items() {
                    item.expression.to_expression_string().hash(hasher);
                    item.direction.hash(hasher);
                }
                n.limit().hash(hasher);
            }
            PlanNodeEnum::TopN(n) => {
                for item in n.sort_items() {
                    item.expression.to_expression_string().hash(hasher);
                    item.direction.hash(hasher);
                }
                n.limit().hash(hasher);
            }
            PlanNodeEnum::Limit(n) => {
                n.offset().hash(hasher);
                n.count().hash(hasher);
            }
            PlanNodeEnum::Sample(n) => {
                n.count().hash(hasher);
            }
            PlanNodeEnum::Aggregate(n) => {
                for key in n.group_keys() {
                    key.hash(hasher);
                }
            }
            PlanNodeEnum::ScanVertices(n) => {
                n.tag().hash(hasher);
                n.projected_properties().hash(hasher);
                if let Some(filter) = n.vertex_filter() {
                    self.hash_expression(filter, hasher);
                }
                n.limit().hash(hasher);
            }
            PlanNodeEnum::ScanEdges(n) => {
                n.edge_type().hash(hasher);
                n.projected_properties().hash(hasher);
                if let Some(filter) = n.filter() {
                    self.hash_expression(filter, hasher);
                }
                n.limit().hash(hasher);
            }
            PlanNodeEnum::IndexScan(n) => {
                n.schema_name().hash(hasher);
                n.index_name().hash(hasher);
                if let Some(filter) = n.filter() {
                    self.hash_expression(filter, hasher);
                }
                n.limit().hash(hasher);
            }
            _ => {}
        }
    }

    /// Hash a contextual expression by its string form, so that equivalent
    /// expressions (regardless of their registered IDs) collide.
    fn hash_expression(&self, expr: &ContextualExpression, hasher: &mut DefaultHasher) {
        match expr.expression() {
            Some(meta) => meta.to_expression_string().hash(hasher),
            None => expr.id().hash(hasher),
        }
    }

    /// Hash child node
    fn hash_children(&self, node: &PlanNodeEnum, hasher: &mut DefaultHasher) {
        use crate::query::planning::plan::core::nodes::*;

        match node {
            // Single input node
            PlanNodeEnum::Filter(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Project(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Sort(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Limit(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::TopN(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Sample(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Aggregate(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Dedup(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Unwind(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::DataCollect(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Traverse(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Expand(n) => {
                // ExpandNode uses MultipleInputNode, accessing children through inputs()
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::ExpandAll(n) => {
                // ExpandAllNode uses MultipleInputNode, accessing children through inputs()
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::AppendVertices(n) => {
                // AppendVerticesNode uses MultipleInputNode, accessing children through inputs()
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::Argument(_) => {
                // The ArgumentNode has zero inputs; no child nodes to hash.
            }
            PlanNodeEnum::PassThrough(_) => {
                // The PassThroughNode has zero inputs; no child nodes to hash.
            }
            PlanNodeEnum::PatternApply(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::CorrelatedApply(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::RollUpApply(n) => {
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Assign(n) => {
                self.hash_single_input(n, hasher);
            }

            // Dual-input node
            PlanNodeEnum::InnerJoin(n) => {
                self.hash_binary_input(n, hasher);
            }
            PlanNodeEnum::LeftJoin(n) => {
                self.hash_binary_input(n, hasher);
            }
            PlanNodeEnum::CrossJoin(n) => {
                self.hash_binary_input(n, hasher);
            }
            PlanNodeEnum::FullOuterJoin(n) => {
                self.hash_binary_input(n, hasher);
            }
            PlanNodeEnum::Union(n) => {
                // UnionNode is a single-input node.
                self.hash_single_input(n, hasher);
            }
            PlanNodeEnum::Minus(n) => {
                // MinusNode uses a custom method to access the input data.
                let left_fp = self.calculate_fingerprint(n.input());
                let right_fp = self.calculate_fingerprint(n.minus_input());
                left_fp.hash(hasher);
                right_fp.hash(hasher);
            }
            PlanNodeEnum::Intersect(n) => {
                // IntersectNode uses a custom method to access the input data.
                let left_fp = self.calculate_fingerprint(n.input());
                let right_fp = self.calculate_fingerprint(n.intersect_input());
                left_fp.hash(hasher);
                right_fp.hash(hasher);
            }

            // More nodes
            PlanNodeEnum::Select(n) => {
                if let Some(ref branch) = n.if_branch() {
                    let fp = self.calculate_fingerprint(branch);
                    fp.hash(hasher);
                }
                if let Some(ref branch) = n.else_branch() {
                    let fp = self.calculate_fingerprint(branch);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::Loop(n) => {
                if let Some(ref body) = n.body() {
                    let body_fp = self.calculate_fingerprint(body);
                    body_fp.hash(hasher);
                }
            }

            // Zero-input nodes (leaf nodes)
            PlanNodeEnum::Start(_) => {
                // Leaf
            }
            PlanNodeEnum::GetVertices(_) => {
                // Leaf
            }
            PlanNodeEnum::GetEdges(_) => {
                // Leaf
            }
            PlanNodeEnum::GetNeighbors(_) => {
                // Leaf
            }
            PlanNodeEnum::ScanVertices(_) => {
                // Leaf
            }
            PlanNodeEnum::ScanEdges(_) => {
                // Leaf
            }
            PlanNodeEnum::IndexScan(_) => {
                // Leaf
            }
            PlanNodeEnum::ShortestPath(_) => {
                // Leaf
            }
            PlanNodeEnum::MultiShortestPath(_) => {
                // Leaf
            }
            PlanNodeEnum::BFSShortest(_) => {
                // Leaf
            }
            PlanNodeEnum::AllPaths(_) => {
                // Leaf
            }

            // Management nodes (not involved in optimization decisions)
            _ => {
                // No fingerprints for management nodes.
            }
        }
    }

    /// Hash child nodes of a single-input node
    fn hash_single_input<T: SingleInputNode>(&self, node: &T, hasher: &mut DefaultHasher) {
        let input_fp = self.calculate_fingerprint(node.input());
        input_fp.hash(hasher);
    }

    /// Hash child nodes of a dual-input node
    fn hash_binary_input<T: BinaryInputNode>(&self, node: &T, hasher: &mut DefaultHasher) {
        let left_fp = self.calculate_fingerprint(node.left_input());
        let right_fp = self.calculate_fingerprint(node.right_input());
        left_fp.hash(hasher);
        right_fp.hash(hasher);
    }
}
