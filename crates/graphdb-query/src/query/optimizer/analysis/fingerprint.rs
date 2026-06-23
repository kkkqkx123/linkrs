//! Plan Node Fingerprint Calculation Module
//!
//! Provide a function for calculating the structural fingerprint of plan nodes, which is used to identify equivalent sub-plans.
//! Sub-plans with the same structure will generate the same fingerprint values.
//!
//! ## Design Specifications
//!
//! The current implementation is a simplified version that only hashes the node types and the structure of the child nodes.
//! Used to identify duplicate sub-plans (such as in the optimization of materialized CTEs).
//! It does not include node configuration parameters or expression structures in order to improve performance.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

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
/// ## Simplified Design
///
/// Only the node type and the sub-node structure are hashed; the actual data is not hashed.
/// Node configuration parameters (such as Filter criteria, number of columns in the Project, etc.)
/// Expression syntax (such as variable names, literal values, etc.)
///
/// This design meets the current requirements (identifying duplicate sub-plans) and also improves performance.
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
    pub fn calculate_fingerprint(&self, node: &PlanNodeEnum) -> PlanFingerprint {
        let mut hasher = DefaultHasher::new();

        // Hash node type
        std::mem::discriminant(node).hash(&mut hasher);

        // Hash child node fingerprint
        self.hash_children(node, &mut hasher);

        PlanFingerprint::new(hasher.finish())
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
                // ExpandNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::ExpandAll(n) => {
                // ExpandAllNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::AppendVertices(n) => {
                // AppendVerticesNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                for dep in n.inputs() {
                    let fp = self.calculate_fingerprint(dep);
                    fp.hash(hasher);
                }
            }
            PlanNodeEnum::Argument(_) => {
                // The `ArgumentNode` is a node with zero inputs; therefore, it does not require any hash child nodes.
            }
            PlanNodeEnum::PassThrough(_) => {
                // The PassThroughNode is a node with zero inputs; it does not require any hash-related child nodes.
            }
            PlanNodeEnum::PatternApply(n) => {
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
            PlanNodeEnum::HashInnerJoin(n) => {
                self.hash_binary_input(n, hasher);
            }
            PlanNodeEnum::HashLeftJoin(n) => {
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

            // Add more nodes
            PlanNodeEnum::Select(n) => {
                // The SelectNode method uses the if_branch and else_branch methods.
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
                // The `body` of `LoopNode` returns an `Option<Box<PlanNodeEnum>>`.
                if let Some(ref body) = n.body() {
                    let body_fp = self.calculate_fingerprint(body);
                    body_fp.hash(hasher);
                }
            }

            // Zero-input nodes (leaf nodes)
            PlanNodeEnum::Start(_) => {
                // Leaf nodes do not require hashed child nodes.
            }
            PlanNodeEnum::GetVertices(_) => {
                // Leaf node
            }
            PlanNodeEnum::GetEdges(_) => {
                // Leaf node
            }
            PlanNodeEnum::GetNeighbors(_) => {
                // Leaf node
            }
            PlanNodeEnum::ScanVertices(_) => {
                // Leaf node
            }
            PlanNodeEnum::ScanEdges(_) => {
                // Leaf node
            }
            PlanNodeEnum::EdgeIndexScan(_) => {
                // Leaf node
            }
            PlanNodeEnum::IndexScan(_) => {
                // Leaf node
            }
            PlanNodeEnum::ShortestPath(_) => {
                // Leaf node
            }
            PlanNodeEnum::MultiShortestPath(_) => {
                // Leaf node
            }
            PlanNodeEnum::BFSShortest(_) => {
                // Leaf node
            }
            PlanNodeEnum::AllPaths(_) => {
                // Leaf node
            }

            // Management node (does not participate in optimization decisions)
            _ => {
                // The management node does not calculate fingerprints.
            }
        }
    }

    /// Child nodes of a hash single-input node
    fn hash_single_input<T: SingleInputNode>(&self, node: &T, hasher: &mut DefaultHasher) {
        let input_fp = self.calculate_fingerprint(node.input());
        input_fp.hash(hasher);
    }

    /// Child nodes of a hash dual-input node
    fn hash_binary_input<T: BinaryInputNode>(&self, node: &T, hasher: &mut DefaultHasher) {
        let left_fp = self.calculate_fingerprint(node.left_input());
        let right_fp = self.calculate_fingerprint(node.right_input());
        left_fp.hash(hasher);
        right_fp.hash(hasher);
    }
}
