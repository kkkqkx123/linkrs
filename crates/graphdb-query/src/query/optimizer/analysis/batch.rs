//! Batch Plan Analyzer Module
//!
//! Performs all plan analysis in a single traversal, collecting:
//! - Reference counts (for materialization decisions)
//! - Structural fingerprints (for subplan identification)
//! - Expression summaries (aggregated from all nodes)
//!
//! This avoids multiple traversals of the same plan tree.

use std::collections::HashMap;

use crate::core::types::ContextualExpression;
use crate::query::optimizer::analysis::{
    ExpressionAnalysis, ExpressionAnalyzer, FingerprintCalculator, PlanFingerprint,
    ReferenceCountAnalysis, SubplanId, SubplanReferenceInfo,
};
use crate::query::planning::plan::core::nodes::PlanNodeEnum;

/// Combined plan analysis result from a single traversal
#[derive(Debug, Clone)]
pub struct BatchPlanAnalysis {
    /// Reference count analysis (for detecting repeated subplans)
    pub reference_count: ReferenceCountAnalysis,
    /// Aggregated expression analysis from all nodes
    pub expression_summary: AggregatedExpressionAnalysis,
    /// Node fingerprint mapping
    pub fingerprints: HashMap<i64, PlanFingerprint>,
}

/// Aggregated expression analysis across all nodes in the plan
#[derive(Debug, Clone, Default)]
pub struct AggregatedExpressionAnalysis {
    /// Whether all expressions in the plan are deterministic
    pub is_fully_deterministic: bool,
    /// Total complexity score across all expressions
    pub total_complexity: u32,
    /// All referenced properties (deduplicated)
    pub all_referenced_properties: Vec<String>,
    /// All referenced variables (deduplicated)
    pub all_referenced_variables: Vec<String>,
    /// All called functions (deduplicated)
    pub all_called_functions: Vec<String>,
    /// Whether any expression contains aggregate functions
    pub contains_aggregates: bool,
    /// Whether any expression contains subqueries
    pub contains_subqueries: bool,
    /// Total expression node count
    pub total_expression_nodes: u32,
}

/// Context shared during the single traversal
struct AnalysisContext {
    /// Fingerprint calculator
    fingerprint_calculator: FingerprintCalculator,
    /// Expression analyzer
    expression_analyzer: ExpressionAnalyzer,
    /// Reference count tracking
    fingerprint_map: HashMap<u64, SubplanReferenceInfo>,
    node_fingerprint_map: HashMap<i64, u64>,
    node_count_map: HashMap<i64, usize>,
    /// Collected expression analyses
    expression_analyses: Vec<ExpressionAnalysis>,
    /// Node ID to fingerprint mapping
    fingerprints: HashMap<i64, PlanFingerprint>,
}

impl AnalysisContext {
    fn new() -> Self {
        Self {
            fingerprint_calculator: FingerprintCalculator::new(),
            expression_analyzer: ExpressionAnalyzer::new(),
            fingerprint_map: HashMap::new(),
            node_fingerprint_map: HashMap::new(),
            node_count_map: HashMap::new(),
            expression_analyses: Vec::new(),
            fingerprints: HashMap::new(),
        }
    }

    /// Record a subplan reference
    fn record_reference(&mut self, fingerprint: u64, node_id: i64, parent_id: Option<i64>) {
        self.node_fingerprint_map.insert(node_id, fingerprint);

        let info = self
            .fingerprint_map
            .entry(fingerprint)
            .or_insert_with(|| SubplanReferenceInfo::new(SubplanId::new(fingerprint), node_id));

        if let Some(parent) = parent_id {
            info.add_reference(parent);
        } else {
            info.reference_count = info.reference_count.max(1);
        }
    }

    /// Record node count for a subplan
    fn record_node_count(&mut self, node_id: i64, count: usize) {
        self.node_count_map.insert(node_id, count);
    }

    /// Record fingerprint for a node
    fn record_fingerprint(&mut self, node_id: i64, fingerprint: PlanFingerprint) {
        self.fingerprints.insert(node_id, fingerprint);
    }

    /// Analyze an expression and collect results
    fn analyze_expression(&mut self, ctx_expr: &ContextualExpression) {
        let analysis = self.expression_analyzer.analyze(ctx_expr);
        self.expression_analyses.push(analysis);
    }

    /// Build reference count analysis result
    fn build_reference_analysis(&self) -> ReferenceCountAnalysis {
        let mut repeated_subplans = Vec::new();
        let mut node_reference_map = HashMap::new();

        for (fingerprint, info) in &self.fingerprint_map {
            if info.reference_count >= 2 {
                let mut info_clone = info.clone();
                if let Some(&count) = self.node_count_map.get(&info.root_node_id) {
                    info_clone.node_count = count;
                }

                for (node_id, fp) in &self.node_fingerprint_map {
                    if *fp == *fingerprint {
                        node_reference_map.insert(*node_id, info_clone.clone());
                    }
                }

                repeated_subplans.push(info_clone);
            }
        }

        ReferenceCountAnalysis {
            repeated_subplans,
            node_reference_map,
        }
    }

    /// Build aggregated expression analysis
    fn build_expression_summary(&self) -> AggregatedExpressionAnalysis {
        let mut summary = AggregatedExpressionAnalysis {
            is_fully_deterministic: true,
            ..Default::default()
        };

        let mut properties_set = std::collections::HashSet::new();
        let mut variables_set = std::collections::HashSet::new();
        let mut functions_set = std::collections::HashSet::new();

        for analysis in &self.expression_analyses {
            if !analysis.is_deterministic {
                summary.is_fully_deterministic = false;
            }
            summary.total_complexity += analysis.complexity_score;
            summary.total_expression_nodes += analysis.node_count;
            summary.contains_aggregates |= analysis.contains_aggregate;
            summary.contains_subqueries |= analysis.contains_subquery;

            for prop in &analysis.referenced_properties {
                properties_set.insert(prop.clone());
            }
            for var in &analysis.referenced_variables {
                variables_set.insert(var.clone());
            }
            for func in &analysis.called_functions {
                functions_set.insert(func.clone());
            }
        }

        summary.all_referenced_properties = properties_set.into_iter().collect();
        summary.all_referenced_variables = variables_set.into_iter().collect();
        summary.all_called_functions = functions_set.into_iter().collect();

        summary
    }
}

/// Batch plan analyzer - performs all analysis in a single traversal
#[derive(Debug, Clone, Default)]
pub struct BatchPlanAnalyzer;

impl BatchPlanAnalyzer {
    /// Create a new batch analyzer
    pub fn new() -> Self {
        Self
    }

    /// Analyze the entire plan in a single traversal
    ///
    /// This method performs:
    /// 1. Reference counting (for materialization decisions)
    /// 2. Fingerprint calculation (for subplan identification)
    /// 3. Expression analysis aggregation (from all nodes)
    ///
    /// All operations happen in one post-order traversal of the plan tree.
    pub fn analyze(&self, root: &PlanNodeEnum) -> BatchPlanAnalysis {
        let mut context = AnalysisContext::new();
        self.analyze_recursive(root, &mut context, None);

        BatchPlanAnalysis {
            reference_count: context.build_reference_analysis(),
            expression_summary: context.build_expression_summary(),
            fingerprints: context.fingerprints,
        }
    }

    /// Recursive single-pass analysis
    ///
    /// Returns the number of nodes in the subtree
    fn analyze_recursive(
        &self,
        node: &PlanNodeEnum,
        context: &mut AnalysisContext,
        parent_id: Option<i64>,
    ) -> usize {
        // 1. Calculate fingerprint (shared computation)
        let fingerprint = context.fingerprint_calculator.calculate_fingerprint(node);
        let node_id = node.id();

        // Record fingerprint
        context.record_fingerprint(node_id, fingerprint);

        // 2. Record reference for reference counting
        context.record_reference(fingerprint.value(), node_id, parent_id);

        // 3. Analyze expressions in the current node
        self.analyze_node_expressions(node, context);

        // 4. Recursively analyze children
        let child_count = self.analyze_children(node, context, node_id);
        let total_count = child_count + 1;

        // 5. Record node count for this subtree
        context.record_node_count(node_id, total_count);

        total_count
    }

    /// Analyze expressions contained in a specific node
    fn analyze_node_expressions(&self, node: &PlanNodeEnum, context: &mut AnalysisContext) {
        use crate::query::planning::plan::core::nodes::*;

        match node {
            PlanNodeEnum::Filter(n) => {
                context.analyze_expression(n.condition());
            }
            PlanNodeEnum::Project(n) => {
                for col in n.columns() {
                    context.analyze_expression(&col.expression);
                }
            }
            PlanNodeEnum::Aggregate(n) => {
                // Aggregate node itself doesn't have expressions directly,
                // but we could analyze group keys if needed
                for key in n.group_keys() {
                    // Group keys are strings, not expressions
                    // Skip for now as they don't need expression analysis
                    let _ = key;
                }
            }
            PlanNodeEnum::Sort(n) => {
                // Sort items contain column names (strings), not expressions
                // Skip expression analysis for sort items
                for _item in n.sort_items() {
                    // Sort items are SortItem with column name (String) and direction
                    // No ContextualExpression to analyze here
                }
            }
            PlanNodeEnum::InnerJoin(n) => {
                for key in n.hash_keys() {
                    context.analyze_expression(key);
                }
                for key in n.probe_keys() {
                    context.analyze_expression(key);
                }
            }
            PlanNodeEnum::LeftJoin(n) => {
                for key in n.hash_keys() {
                    context.analyze_expression(key);
                }
                for key in n.probe_keys() {
                    context.analyze_expression(key);
                }
            }
            PlanNodeEnum::HashInnerJoin(n) => {
                for key in n.hash_keys() {
                    context.analyze_expression(key);
                }
                for key in n.probe_keys() {
                    context.analyze_expression(key);
                }
            }
            PlanNodeEnum::HashLeftJoin(n) => {
                for key in n.hash_keys() {
                    context.analyze_expression(key);
                }
                for key in n.probe_keys() {
                    context.analyze_expression(key);
                }
            }
            PlanNodeEnum::Loop(n) => {
                context.analyze_expression(n.condition());
            }
            PlanNodeEnum::Select(n) => {
                context.analyze_expression(n.condition());
            }
            _ => {}
        }
    }

    /// Analyze child nodes and return total count
    fn analyze_children(
        &self,
        node: &PlanNodeEnum,
        context: &mut AnalysisContext,
        node_id: i64,
    ) -> usize {
        use crate::query::planning::plan::core::nodes::*;

        match node {
            // Zero-input nodes
            PlanNodeEnum::Start(_)
            | PlanNodeEnum::GetVertices(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::GetNeighbors(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::EdgeIndexScan(_)
            | PlanNodeEnum::IndexScan(_)
            | PlanNodeEnum::Argument(_) => 0,

            // Single-input nodes
            PlanNodeEnum::Filter(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Project(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Sort(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Limit(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::TopN(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Sample(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Aggregate(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Dedup(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::PassThrough(_) => {
                // PassThroughNode is a ZeroInputNode, no children to analyze
                0
            }
            PlanNodeEnum::DataCollect(n) => {
                self.analyze_recursive(n.input(), context, Some(node_id))
            }
            PlanNodeEnum::Unwind(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Remove(n) => self.analyze_recursive(n.input(), context, Some(node_id)),
            PlanNodeEnum::Materialize(n) => {
                self.analyze_recursive(n.input(), context, Some(node_id))
            }

            // Two-input nodes (joins)
            PlanNodeEnum::InnerJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::LeftJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::CrossJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::HashInnerJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::HashLeftJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::FullOuterJoin(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }

            // Multi-input nodes using dependencies()
            PlanNodeEnum::Union(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::Minus(n) => {
                let main_count = self.analyze_recursive(n.input(), context, Some(node_id));
                let minus_count = self.analyze_recursive(n.minus_input(), context, Some(node_id));
                main_count + minus_count
            }
            PlanNodeEnum::Intersect(n) => {
                let main_count = self.analyze_recursive(n.input(), context, Some(node_id));
                let intersect_count =
                    self.analyze_recursive(n.intersect_input(), context, Some(node_id));
                main_count + intersect_count
            }

            // Traversal nodes
            PlanNodeEnum::Expand(n) => n
                .inputs()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::ExpandAll(n) => n
                .inputs()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::Traverse(n) => {
                // TraverseNode uses SingleInputNode trait, input() method panics when input is None
                // Here, we directly access the deps field to traverse the child nodes.
                n.dependencies()
                    .iter()
                    .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                    .sum()
            }
            PlanNodeEnum::AppendVertices(n) => n
                .inputs()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),

            // Control flow nodes
            PlanNodeEnum::Loop(n) => {
                let body_count = n
                    .body()
                    .as_ref()
                    .map(|body| self.analyze_recursive(body.as_ref(), context, Some(node_id)))
                    .unwrap_or(0);
                body_count
            }
            PlanNodeEnum::Select(n) => {
                let if_count = n
                    .if_branch()
                    .as_ref()
                    .map(|branch| self.analyze_recursive(branch.as_ref(), context, Some(node_id)))
                    .unwrap_or(0);
                let else_count = n
                    .else_branch()
                    .as_ref()
                    .map(|branch| self.analyze_recursive(branch.as_ref(), context, Some(node_id)))
                    .unwrap_or(0);
                if_count + else_count
            }

            // Pattern apply nodes
            PlanNodeEnum::PatternApply(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }
            PlanNodeEnum::RollUpApply(n) => {
                let left_count = self.analyze_recursive(n.left_input(), context, Some(node_id));
                let right_count = self.analyze_recursive(n.right_input(), context, Some(node_id));
                left_count + right_count
            }

            // Algorithm nodes
            PlanNodeEnum::ShortestPath(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::MultiShortestPath(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::BFSShortest(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),
            PlanNodeEnum::AllPaths(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),

            // Data processing nodes
            PlanNodeEnum::Assign(n) => n
                .dependencies()
                .iter()
                .map(|dep| self.analyze_recursive(dep, context, Some(node_id)))
                .sum(),

            // Management nodes (typically leaf nodes)
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::StartNode;

    #[test]
    fn test_batch_analyzer_creation() {
        let _analyzer = BatchPlanAnalyzer::new();
        let _default_analyzer = BatchPlanAnalyzer;
    }

    #[test]
    fn test_analyze_simple_plan() {
        let analyzer = BatchPlanAnalyzer::new();
        let root = PlanNodeEnum::Start(StartNode::new());

        let result = analyzer.analyze(&root);

        assert_eq!(result.reference_count.repeated_count(), 0);
        assert!(result.expression_summary.is_fully_deterministic);
        assert_eq!(result.expression_summary.total_expression_nodes, 0);
    }
}
