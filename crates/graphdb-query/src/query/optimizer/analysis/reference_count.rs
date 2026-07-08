//! Reference Count Analysis Module
//!
//! Identify the sub-plan nodes that are referenced multiple times in the execution plan, to provide data support for the selection of the materialization strategy.

use std::collections::HashMap;

use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

use super::fingerprint::FingerprintCalculator;

/// Unique identifier for the sub-plan
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SubplanId(pub u64);

impl SubplanId {
    /// Create a new sub-plan ID.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// Obtain the ID value
    pub fn value(&self) -> u64 {
        self.0
    }
}

/// Sub-plan reference information
#[derive(Debug, Clone)]
pub struct SubplanReferenceInfo {
    /// The unique identifier of the sub-plan
    pub subplan_id: SubplanId,
    /// Sub-plan root node ID
    pub root_node_id: i64,
    /// Number of citations
    pub reference_count: usize,
    /// Reference location (list of parent node IDs)
    pub reference_locations: Vec<i64>,
    /// Estimate the number of output lines.
    pub estimated_output_rows: u64,
    /// The number of nodes included in the sub-plan
    pub node_count: usize,
}

impl SubplanReferenceInfo {
    /// Create new sub-plan reference information.
    pub fn new(subplan_id: SubplanId, root_node_id: i64) -> Self {
        Self {
            subplan_id,
            root_node_id,
            reference_count: 0,
            reference_locations: Vec::new(),
            estimated_output_rows: 0,
            node_count: 0,
        }
    }

    /// Increase the reference count
    pub fn add_reference(&mut self, location: i64) {
        self.reference_count += 1;
        if !self.reference_locations.contains(&location) {
            self.reference_locations.push(location);
        }
    }
}

/// Results of reference count analysis
#[derive(Debug, Clone)]
pub struct ReferenceCountAnalysis {
    /// All sub-plans that have been referenced multiple times (number of references >= 2)
    pub repeated_subplans: Vec<SubplanReferenceInfo>,
    /// Mapping from node IDs to reference information
    pub node_reference_map: HashMap<i64, SubplanReferenceInfo>,
}

impl ReferenceCountAnalysis {
    /// Create an empty analysis result.
    pub fn new() -> Self {
        Self {
            repeated_subplans: Vec::new(),
            node_reference_map: HashMap::new(),
        }
    }

    /// Obtain the reference information for the specified node.
    pub fn get_node_info(&self, node_id: i64) -> Option<&SubplanReferenceInfo> {
        self.node_reference_map.get(&node_id)
    }

    /// Check whether the sub-plan is referenced multiple times.
    pub fn is_repeated(&self, node_id: i64) -> bool {
        self.node_reference_map
            .get(&node_id)
            .map(|info| info.reference_count >= 2)
            .unwrap_or(false)
    }

    /// Obtain the number of sub-plans that have been referenced multiple times.
    pub fn repeated_count(&self) -> usize {
        self.repeated_subplans.len()
    }
}

impl Default for ReferenceCountAnalysis {
    fn default() -> Self {
        Self::new()
    }
}

/// Analyze the context
struct AnalysisContext {
    /// Mapping from fingerprints to reference information
    fingerprint_map: HashMap<u64, SubplanReferenceInfo>,
    /// Mapping from node IDs to fingerprints
    node_fingerprint_map: HashMap<i64, u64>,
    /// Node count mapping (used for estimating the size of sub-plans)
    node_count_map: HashMap<i64, usize>,
}

impl AnalysisContext {
    /// Create a new analysis context.
    fn new() -> Self {
        Self {
            fingerprint_map: HashMap::new(),
            node_fingerprint_map: HashMap::new(),
            node_count_map: HashMap::new(),
        }
    }

    /// Record citations
    fn record_reference(&mut self, fingerprint: u64, node_id: i64, parent_id: Option<i64>) {
        // Record the mapping from nodes to fingerprints.
        self.node_fingerprint_map.insert(node_id, fingerprint);

        // Obtain or create citation information.
        let info = self
            .fingerprint_map
            .entry(fingerprint)
            .or_insert_with(|| SubplanReferenceInfo::new(SubplanId::new(fingerprint), node_id));

        // Increase the reference count
        if let Some(parent) = parent_id {
            info.add_reference(parent);
        } else {
            // Root node: The reference count is at least 1.
            info.reference_count = info.reference_count.max(1);
        }
    }

    /// Record the number of nodes.
    fn record_node_count(&mut self, node_id: i64, count: usize) {
        self.node_count_map.insert(node_id, count);
    }

    /// Analysis results:
    fn into_analysis_result(self) -> ReferenceCountAnalysis {
        let mut repeated_subplans = Vec::new();
        let mut node_reference_map = HashMap::new();

        for (fingerprint, mut info) in self.fingerprint_map {
            // Retain only the sub-plans that are referenced multiple times.
            if info.reference_count >= 2 {
                // Update the number of nodes.
                if let Some(&count) = self.node_count_map.get(&info.root_node_id) {
                    info.node_count = count;
                }

                // Add reference information for all nodes that have the same fingerprint.
                for (node_id, fp) in &self.node_fingerprint_map {
                    if *fp == fingerprint {
                        node_reference_map.insert(*node_id, info.clone());
                    }
                }

                repeated_subplans.push(info);
            }
        }

        ReferenceCountAnalysis {
            repeated_subplans,
            node_reference_map,
        }
    }
}

/// Reference Count Analyzer
///
/// Analyze the execution plan to identify the sub-plan nodes that are referenced multiple times.
#[derive(Debug, Clone)]
pub struct ReferenceCountAnalyzer {
    /// Fingerprint calculator
    fingerprint_calculator: FingerprintCalculator,
}

impl ReferenceCountAnalyzer {
    /// Create a new reference counting analyzer
    pub fn new() -> Self {
        Self {
            fingerprint_calculator: FingerprintCalculator::new(),
        }
    }

    /// Analyze the reference count of the planning plan.
    ///
    /// # Parameters
    /// The root node of the execution plan to be analyzed.
    ///
    /// # Return
    /// Citation Count Analysis Results
    ///
    /// # Algorithms
    /// Post-order traversal of a plan tree
    /// 2. Calculate a structural fingerprint for each node.
    /// 3. Count the number of occurrences of each fingerprint.
    /// 4. Retrieve information about the sub-plans that have been referenced multiple times.
    pub fn analyze(&self, plan: &PlanNodeEnum) -> ReferenceCountAnalysis {
        let mut context = AnalysisContext::new();
        self.analyze_recursive(plan, &mut context, None);
        context.into_analysis_result()
    }

    /// Recursive analysis of the plan tree
    ///
    /// # Parameters
    /// `node`: The current node
    /// “Context” refers to the analysis of the surrounding circumstances, background, or information that is relevant to a particular situation or topic. In translation, understanding the context is crucial in order to provide an accurate and meaningful translation that captures the intended meaning of the original text. This may involve considering the context of the language, the cultural context, the historical context, or any other relevant factors that may affect the interpretation of the text.
    /// `parent_id`: ID of the parent node (used to record the reference location)
    ///
    /// # Back
    /// Number of nodes
    fn analyze_recursive(
        &self,
        node: &PlanNodeEnum,
        context: &mut AnalysisContext,
        parent_id: Option<i64>,
    ) -> usize {
        // Calculate the fingerprint of the current node.
        let fingerprint = self.fingerprint_calculator.calculate_fingerprint(node);
        let node_id = node.id();

        // Record the citation.
        context.record_reference(fingerprint.value(), node_id, parent_id);

        // Recursively analyze the child nodes and calculate the total number of nodes.
        let child_count = match node {
            // 单输入节点 - 使用 dependencies() 访问子节点
            PlanNodeEnum::Filter(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Project(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Sort(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Limit(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::TopN(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Sample(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Aggregate(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Dedup(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Unwind(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::DataCollect(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Traverse(n) => {
                // TraverseNode 使用 SingleInputNode trait，input() 方法在 input 为 None 时会 panic
                // Here, we directly access the `deps` field to traverse the child nodes.
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Expand(n) => {
                // ExpandNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                let mut total = 1;
                for dep in n.inputs() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::ExpandAll(n) => {
                // ExpandAllNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                let mut total = 1;
                for dep in n.inputs() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::AppendVertices(n) => {
                // AppendVerticesNode 使用 MultipleInputNode，通过 inputs() 访问子节点
                let mut total = 1;
                for dep in n.inputs() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            // ArgumentNode and PassThroughNode are nodes that accept no input.
            PlanNodeEnum::Argument(_) => 1,
            PlanNodeEnum::PassThrough(_) => 1,
            PlanNodeEnum::PatternApply(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::RollUpApply(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::Assign(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Minus(n) => {
                let main_count = self.analyze_recursive(n.input(), context, Some(node_id));
                let minus_count = self.analyze_recursive(n.minus_input(), context, Some(node_id));
                1 + main_count + minus_count
            }
            PlanNodeEnum::Intersect(n) => {
                let main_count = self.analyze_recursive(n.input(), context, Some(node_id));
                let intersect_count =
                    self.analyze_recursive(n.intersect_input(), context, Some(node_id));
                1 + main_count + intersect_count
            }

            // Dual-input node
            PlanNodeEnum::InnerJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::LeftJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::CrossJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::HashInnerJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::HashLeftJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::FullOuterJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                1 + left_count + right_count
            }
            PlanNodeEnum::Union(n) => {
                let mut total = 1;
                for dep in n.dependencies() {
                    let count = self.analyze_recursive(dep, context, Some(node_id));
                    total += count;
                }
                total
            }

            // Add more nodes.
            PlanNodeEnum::Select(n) => {
                let mut total = 1; // Current node
                if let Some(ref branch) = n.if_branch() {
                    let count = self.analyze_recursive(branch, context, Some(node_id));
                    total += count;
                }
                if let Some(ref branch) = n.else_branch() {
                    let count = self.analyze_recursive(branch, context, Some(node_id));
                    total += count;
                }
                total
            }
            PlanNodeEnum::Loop(n) => {
                let mut total = 1; // Current node
                if let Some(ref body) = n.body() {
                    let count = self.analyze_recursive(body, context, Some(node_id));
                    total += count;
                }
                total
            }

            // Zero-input nodes (leaf nodes)
            _ => 1, // Only the current node.
        };

        // Record the number of nodes.
        context.record_node_count(node_id, child_count);

        child_count
    }
}

impl Default for ReferenceCountAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reference_count_analyzer_new() {
        let _analyzer = ReferenceCountAnalyzer::new();
        // Verification of successful creation.
    }

    #[test]
    fn test_subplan_id() {
        let id = SubplanId::new(12345);
        assert_eq!(id.value(), 12345);
    }

    #[test]
    fn test_subplan_reference_info() {
        let mut info = SubplanReferenceInfo::new(SubplanId::new(1), 100);
        assert_eq!(info.reference_count, 0);

        info.add_reference(200);
        assert_eq!(info.reference_count, 1);
        assert!(info.reference_locations.contains(&200));

        info.add_reference(200); // Duplicate the addition.
        assert_eq!(info.reference_count, 2);
        assert_eq!(info.reference_locations.len(), 1); // Positions must not be duplicated.
    }

    #[test]
    fn test_reference_count_analysis() {
        let analysis = ReferenceCountAnalysis::new();
        assert_eq!(analysis.repeated_count(), 0);
        assert!(!analysis.is_repeated(1));
    }
}
