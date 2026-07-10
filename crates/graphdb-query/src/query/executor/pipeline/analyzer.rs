//! Pipeline analyzer
//!
//! Walks a physical plan tree and produces a PipelineGraph by
//! identifying pipeline breaker boundaries.

use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};

use super::breaker::{classify_breaker, is_source, PipelineBreakerKind};
use super::graph::{Pipeline, PipelineGraph, PipelineSink, PipelineSource};

/// Result of analyzing a plan node and its sub-tree.
struct AnalysisResult {
    /// The pipeline ID that produces the output for this sub-tree
    pipeline_id: usize,
}

/// Analyzes a physical plan and produces a PipelineGraph.
pub struct PipelineAnalyzer;

impl PipelineAnalyzer {
    /// Analyze the given plan tree and produce a pipeline graph.
    pub fn analyze(root: &PlanNodeEnum) -> PipelineGraph {
        let mut next_id: usize = 0;
        let mut all_pipelines: Vec<Pipeline> = Vec::new();

        let result = Self::analyze_node(root, &mut next_id, &mut all_pipelines);

        let root_pipeline_id = result.pipeline_id;

        // Connect pipeline edges: populate upstream/downstream IDs
        // by walking pipelines and identifying exchange boundaries
        Self::connect_pipelines(&mut all_pipelines);

        PipelineGraph::new(all_pipelines, root_pipeline_id)
    }

    /// Recursively analyze a plan node and its sub-tree.
    fn analyze_node(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        if let Some(breaker_kind) = classify_breaker(node) {
            return Self::create_breaker_pipeline(node, breaker_kind, next_id, all_pipelines);
        }

        // If it's a source (leaf), create a single pipeline
        if is_source(node) {
            return Self::create_source_pipeline(node, next_id, all_pipelines);
        }

        // Check if the node has a single input (streaming operator like Filter, Project, Limit)
        // For single-input nodes, try to extend the input's pipeline
        if let Some(input) = Self::try_get_single_input(node) {
            return Self::handle_single_input_node(node, input, next_id, all_pipelines);
        }

        // Handle binary input nodes (joins)
        if Self::has_binary_input(node) {
            return Self::handle_binary_input_node(node, next_id, all_pipelines);
        }

        // Handle multiple input nodes (set ops, expand)
        if Self::has_multiple_input(node) {
            return Self::handle_multi_input_node(node, next_id, all_pipelines);
        }

        // Fallback: treat as source-like pipeline
        Self::create_source_pipeline(node, next_id, all_pipelines)
    }

    /// Create a pipeline for a source node (leaf).
    fn create_source_pipeline(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let id = *next_id;
        *next_id += 1;

        let name = node.type_name().to_string();
        let pipeline = Pipeline::new(
            id,
            name,
            node.clone(),
            PipelineSource::PlanNode(Box::new(node.clone())),
            PipelineSink::Root,
        );
        all_pipelines.push(pipeline);

        AnalysisResult { pipeline_id: id }
    }

    /// The breaker's inputs become separate pipeline chains.
    fn create_breaker_pipeline(
        node: &PlanNodeEnum,
        breaker_kind: PipelineBreakerKind,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let id = *next_id;
        *next_id += 1;

        let name = format!("{}[{}]", node.type_name(), breaker_kind.name());

        // Recursively analyze all inputs to this breaker
        for input in Self::get_inputs(node) {
            Self::analyze_node(&input, next_id, all_pipelines);
        }

        let pipeline = Pipeline::new(
            id,
            name,
            node.clone(),
            PipelineSource::Exchange,
            PipelineSink::Breaker(breaker_kind),
        );
        all_pipelines.push(pipeline);

        AnalysisResult { pipeline_id: id }
    }

    /// Handle a single-input streaming operator
    /// if the input is itself a streaming pipeline, or creating a new pipeline.
    fn handle_single_input_node(
        node: &PlanNodeEnum,
        input: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let input_result = Self::analyze_node(input, next_id, all_pipelines);

        // Determine whether to extend or create new based on input's sink kind
        let input_sink_is_root = all_pipelines[input_result.pipeline_id]
            .matches_sink(&PipelineSink::Root);

        if input_sink_is_root {
            // Extend: wrap the input's root_node under this node
            let root = Self::wrap_node(node, &all_pipelines[input_result.pipeline_id].root_node);
            all_pipelines[input_result.pipeline_id].root_node = root;
            all_pipelines[input_result.pipeline_id].name = node.type_name().to_string();
            input_result
        } else {
            // Can't extend through a breaker/exchange - start new pipeline
            let id = *next_id;
            *next_id += 1;

            let pipeline = Pipeline::new(
                id,
                node.type_name().to_string(),
                Self::wrap_node(node, input),
                PipelineSource::Exchange,
                PipelineSink::Root,
            );
            all_pipelines.push(pipeline);

            AnalysisResult { pipeline_id: id }
        }
    }

    /// Handle a binary-input node (join-like).
    fn handle_binary_input_node(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let inputs = Self::get_inputs(node);

        for input in &inputs {
            Self::analyze_node(input, next_id, all_pipelines);
        }

        let id = *next_id;
        *next_id += 1;

        let pipeline = Pipeline::new(
            id,
            node.type_name().to_string(),
            node.clone(),
            PipelineSource::Exchange,
            PipelineSink::Root,
        );
        all_pipelines.push(pipeline);

        AnalysisResult { pipeline_id: id }
    }

    /// Handle a multi-input node (set ops, expand).
    fn handle_multi_input_node(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let inputs = Self::get_inputs(node);

        for input in &inputs {
            Self::analyze_node(input, next_id, all_pipelines);
        }

        let id = *next_id;
        *next_id += 1;

        let pipeline = Pipeline::new(
            id,
            node.type_name().to_string(),
            node.clone(),
            PipelineSource::Exchange,
            PipelineSink::Root,
        );
        all_pipelines.push(pipeline);

        AnalysisResult { pipeline_id: id }
    }

    // ── Helpers ──

    /// Get all input sub-trees of a plan node.
    fn get_inputs(node: &PlanNodeEnum) -> Vec<PlanNodeEnum> {
        match node {
            PlanNodeEnum::Filter(n) => vec![n.input().clone()],
            PlanNodeEnum::Project(n) => vec![n.input().clone()],
            PlanNodeEnum::Limit(n) => vec![n.input().clone()],
            PlanNodeEnum::Sort(n) => vec![n.input().clone()],
            PlanNodeEnum::TopN(n) => vec![n.input().clone()],
            PlanNodeEnum::Sample(n) => vec![n.input().clone()],
            PlanNodeEnum::Aggregate(n) => vec![n.input().clone()],
            PlanNodeEnum::Dedup(n) => vec![n.input().clone()],
            PlanNodeEnum::Window(n) => vec![n.input().clone()],
            PlanNodeEnum::Traverse(n) => vec![n.input().clone()],

            // SingleInputNode with extra deps (union has input + union_input)
            PlanNodeEnum::Union(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Unwind(n) => n.dependencies().to_vec(),
            PlanNodeEnum::DataCollect(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Assign(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Minus(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Intersect(n) => n.dependencies().to_vec(),

            // MultipleInputNode variants (define_plan_node! with input: MultipleInputNode)
            PlanNodeEnum::Expand(n) => n.inputs().to_vec(),
            PlanNodeEnum::ExpandAll(n) => n.inputs().to_vec(),
            PlanNodeEnum::AppendVertices(n) => n.inputs().to_vec(),
            PlanNodeEnum::GetVertices(n) => n.inputs().to_vec(),
            PlanNodeEnum::GetNeighbors(n) => n.inputs().to_vec(),

            // Binary input nodes (from define_binary_input_node!)
            PlanNodeEnum::InnerJoin(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::LeftJoin(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::RightJoin(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::CrossJoin(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::HashInnerJoin(n) => {
                vec![n.left_input().clone(), n.right_input().clone()]
            }
            PlanNodeEnum::HashLeftJoin(n) => {
                vec![n.left_input().clone(), n.right_input().clone()]
            }
            PlanNodeEnum::FullOuterJoin(n) => {
                vec![n.left_input().clone(), n.right_input().clone()]
            }
            PlanNodeEnum::SemiJoin(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::BiExpand(n) => vec![n.left_input().clone(), n.right_input().clone()],
            PlanNodeEnum::BiTraverse(n) => vec![n.left_input().clone(), n.right_input().clone()],

            // Custom binary-input nodes (hand-rolled structs)
            PlanNodeEnum::RollUpApply(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Remove(n) => n.dependencies().to_vec(),
            PlanNodeEnum::PatternApply(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Materialize(n) => n.dependencies().to_vec(),
            PlanNodeEnum::Apply(n) => n.dependencies().to_vec(),

            // Sources and zero-input nodes
            PlanNodeEnum::Start(_)
            | PlanNodeEnum::ScanVertices(_)
            | PlanNodeEnum::ScanEdges(_)
            | PlanNodeEnum::EdgeIndexScan(_)
            | PlanNodeEnum::IndexScan(_)
            | PlanNodeEnum::Argument(_)
            | PlanNodeEnum::GetEdges(_)
            | PlanNodeEnum::ShortestPath(_)
            | PlanNodeEnum::BFSShortest(_)
            | PlanNodeEnum::AllPaths(_)
            | PlanNodeEnum::MultiShortestPath(_)
            | PlanNodeEnum::Loop(_)
            | PlanNodeEnum::PassThrough(_)
            | PlanNodeEnum::Select(_)
            | PlanNodeEnum::BeginTransaction(_)
            | PlanNodeEnum::Commit(_)
            | PlanNodeEnum::Rollback(_)
            | PlanNodeEnum::InsertVertices(_)
            | PlanNodeEnum::InsertEdges(_)
            | PlanNodeEnum::DeleteVertices(_)
            | PlanNodeEnum::DeleteEdges(_)
            | PlanNodeEnum::DeleteTags(_)
            | PlanNodeEnum::DeleteIndex(_)
            | PlanNodeEnum::PipeDeleteVertices(_)
            | PlanNodeEnum::PipeDeleteEdges(_)
            | PlanNodeEnum::Update(_)
            | PlanNodeEnum::UpdateVertices(_)
            | PlanNodeEnum::UpdateEdges(_)
            | PlanNodeEnum::SpaceManage(_)
            | PlanNodeEnum::TagManage(_)
            | PlanNodeEnum::EdgeManage(_)
            | PlanNodeEnum::IndexManage(_)
            | PlanNodeEnum::UserManage(_)
            | PlanNodeEnum::FulltextManage(_)
            | PlanNodeEnum::VectorManage(_)
            | PlanNodeEnum::ShowStats(_)
            | PlanNodeEnum::FulltextSearch(_)
            | PlanNodeEnum::FulltextLookup(_)
            | PlanNodeEnum::MatchFulltext(_) => vec![],
        }
    }

    /// Try to get the single input of a node (if it has exactly one input).
    fn try_get_single_input(node: &PlanNodeEnum) -> Option<&PlanNodeEnum> {
        match node {
            PlanNodeEnum::Filter(n) => Some(n.input()),
            PlanNodeEnum::Project(n) => Some(n.input()),
            PlanNodeEnum::Limit(n) => Some(n.input()),
            PlanNodeEnum::Sort(n) => Some(n.input()),
            PlanNodeEnum::TopN(n) => Some(n.input()),
            PlanNodeEnum::Sample(n) => Some(n.input()),
            PlanNodeEnum::Aggregate(n) => Some(n.input()),
            PlanNodeEnum::Dedup(n) => Some(n.input()),
            PlanNodeEnum::Window(n) => Some(n.input()),
            PlanNodeEnum::Traverse(n) => Some(n.input()),
            _ => {
                let inputs = Self::get_inputs(node);
                if inputs.len() == 1 {
                    // We can't return a reference to a local, so only use
                    // for nodes we match above
                    None
                } else {
                    None
                }
            }
        }
    }

    fn has_binary_input(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::InnerJoin(_)
                | PlanNodeEnum::LeftJoin(_)
                | PlanNodeEnum::RightJoin(_)
                | PlanNodeEnum::CrossJoin(_)
                | PlanNodeEnum::HashInnerJoin(_)
                | PlanNodeEnum::HashLeftJoin(_)
                | PlanNodeEnum::FullOuterJoin(_)
                | PlanNodeEnum::SemiJoin(_)
                | PlanNodeEnum::BiExpand(_)
                | PlanNodeEnum::BiTraverse(_)
        )
    }

    fn has_multiple_input(node: &PlanNodeEnum) -> bool {
        matches!(
            node,
            PlanNodeEnum::Union(_)
                | PlanNodeEnum::Minus(_)
                | PlanNodeEnum::Intersect(_)
                | PlanNodeEnum::Expand(_)
                | PlanNodeEnum::ExpandAll(_)
                | PlanNodeEnum::AppendVertices(_)
                | PlanNodeEnum::DataCollect(_)
                | PlanNodeEnum::Remove(_)
                | PlanNodeEnum::PatternApply(_)
                | PlanNodeEnum::RollUpApply(_)
                | PlanNodeEnum::Unwind(_)
                | PlanNodeEnum::Materialize(_)
                | PlanNodeEnum::Assign(_)
                | PlanNodeEnum::Apply(_)
        )
    }

    /// Wrap an input node under a parent node, creating a new sub-tree.
    fn wrap_node(parent: &PlanNodeEnum, _child: &PlanNodeEnum) -> PlanNodeEnum {
        // To keep things simple, for the pipeline graph representation
        // we use parent.clone() but it won't have correct inputs.
        // This is used for structural analysis only.
        parent.clone()
    }

    /// Connect pipeline edges by analyzing source/sink relationships.
    ///
    /// A Pipeline with `source == Exchange` depends on the pipeline(s)
    /// whose `sink` produces its input.  We match them by analyzing the
    /// logical plan structure.
    ///
    /// Phase 6a: simple heuristic — every pipeline whose source is
    /// `Exchange` finds its upstream as the *last non-Exchange-source*
    /// pipeline created before it.  This is correct for linear chains
    /// and covers the most common query shapes.
    fn connect_pipelines(pipelines: &mut [Pipeline]) {
        let count = pipelines.len();

        // Forward map: pipeline index -> indices of pipelines it feeds into
        let mut feeds_into: Vec<Vec<usize>> = vec![Vec::new(); count];

        for i in 0..count {
            if matches!(pipelines[i].source, PipelineSource::Exchange) {
                // Find the most recently created pipeline that is NOT an exchange source.
                // In a well-formed graph built by the recursive analyzer, this will be
                // the pipeline that produces the input for this one.
                for j in (0..i).rev() {
                    if !matches!(pipelines[j].source, PipelineSource::Exchange) {
                        feeds_into[j].push(pipelines[i].id);
                        break;
                    }
                }
            }
        }

        // Write upstream_ids for each pipeline based on the reverse map
        for i in 0..count {
            let mut upstream = Vec::new();
            for (j, deps) in feeds_into.iter().enumerate() {
                if deps.contains(&pipelines[i].id) {
                    upstream.push(pipelines[j].id);
                }
            }
            upstream.sort();
            upstream.dedup();
            pipelines[i].upstream_ids = upstream;
        }

        // Populate downstream_ids based on the forward map
        for i in 0..count {
            pipelines[i].downstream_ids = feeds_into[i].clone();
        }
    }


}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_analyze_single_source() {
        let root = PlanNodeEnum::Start(StartNode::new());
        let graph = PipelineAnalyzer::analyze(&root);
        assert_eq!(graph.pipeline_count(), 1);
        assert_eq!(graph.root_pipeline_id, 0);
    }
}
