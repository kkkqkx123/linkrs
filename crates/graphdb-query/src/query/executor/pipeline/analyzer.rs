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
        // Track plan node id -> pipeline id for accurate dependency wiring
        let mut node_to_pipeline: std::collections::HashMap<i64, usize> = std::collections::HashMap::new();

        let result = Self::analyze_node_with_map(root, &mut next_id, &mut all_pipelines, &mut node_to_pipeline);

        let root_pipeline_id = result.pipeline_id;

        // Connect pipeline edges: populate upstream/downstream IDs
        // using the node-to-pipeline mapping for accurate wiring
        Self::connect_pipelines(&mut all_pipelines, &node_to_pipeline);

        PipelineGraph::new(all_pipelines, root_pipeline_id)
    }

    /// Internal analyze that also populates a node-to-pipeline mapping.
    fn analyze_node_with_map(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        if let Some(breaker_kind) = classify_breaker(node) {
            return Self::create_breaker_pipeline_with_map(node, breaker_kind, next_id, all_pipelines, node_to_pipeline);
        }

        if is_source(node) {
            return Self::create_source_pipeline_with_map(node, next_id, all_pipelines, node_to_pipeline);
        }

        if let Some(input) = Self::try_get_single_input(node) {
            return Self::handle_single_input_node_with_map(node, input, next_id, all_pipelines, node_to_pipeline);
        }

        if Self::has_binary_input(node) {
            return Self::handle_binary_input_node_with_map(node, next_id, all_pipelines, node_to_pipeline);
        }

        if Self::has_multiple_input(node) {
            return Self::handle_multi_input_node_with_map(node, next_id, all_pipelines, node_to_pipeline);
        }

        Self::create_source_pipeline_with_map(node, next_id, all_pipelines, node_to_pipeline)
    }

    fn create_source_pipeline_with_map(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        let result = Self::create_source_pipeline(node, next_id, all_pipelines);
        node_to_pipeline.insert(node.id(), result.pipeline_id);
        result
    }

    fn create_breaker_pipeline_with_map(
        node: &PlanNodeEnum,
        breaker_kind: PipelineBreakerKind,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        // Recursively analyze all inputs to this breaker
        for input in Self::get_inputs(node) {
            Self::analyze_node_with_map(&input, next_id, all_pipelines, node_to_pipeline);
        }
        let id = *next_id;
        *next_id += 1;
        let name = format!("{}[{}]", node.type_name(), breaker_kind.name());
        let pipeline = Pipeline::new(
            id,
            name,
            node.clone(),
            PipelineSource::Exchange,
            PipelineSink::Breaker(breaker_kind),
        );
        node_to_pipeline.insert(node.id(), id);
        all_pipelines.push(pipeline);
        AnalysisResult { pipeline_id: id }
    }

    fn handle_single_input_node_with_map(
        node: &PlanNodeEnum,
        input: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        let input_result = Self::analyze_node_with_map(input, next_id, all_pipelines, node_to_pipeline);
        let input_sink_is_root = all_pipelines[input_result.pipeline_id]
            .matches_sink(&PipelineSink::Root);

        if input_sink_is_root {
            let root = Self::wrap_node(node, &all_pipelines[input_result.pipeline_id].root_node);
            all_pipelines[input_result.pipeline_id].root_node = root;
            all_pipelines[input_result.pipeline_id].name = node.type_name().to_string();
            node_to_pipeline.insert(node.id(), input_result.pipeline_id);
            input_result
        } else {
            let id = *next_id;
            *next_id += 1;
            let pipeline = Pipeline::new(
                id,
                node.type_name().to_string(),
                Self::wrap_node(node, input),
                PipelineSource::Exchange,
                PipelineSink::Root,
            );
            node_to_pipeline.insert(node.id(), id);
            all_pipelines.push(pipeline);
            AnalysisResult { pipeline_id: id }
        }
    }

    fn handle_binary_input_node_with_map(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        let inputs = Self::get_inputs(node);
        for input in &inputs {
            Self::analyze_node_with_map(input, next_id, all_pipelines, node_to_pipeline);
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
        node_to_pipeline.insert(node.id(), id);
        all_pipelines.push(pipeline);
        AnalysisResult { pipeline_id: id }
    }

    fn handle_multi_input_node_with_map(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
        node_to_pipeline: &mut std::collections::HashMap<i64, usize>,
    ) -> AnalysisResult {
        let inputs = Self::get_inputs(node);
        for input in &inputs {
            Self::analyze_node_with_map(input, next_id, all_pipelines, node_to_pipeline);
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
        node_to_pipeline.insert(node.id(), id);
        all_pipelines.push(pipeline);
        AnalysisResult { pipeline_id: id }
    }

    /// Recursively analyze a plan node and its sub-tree (maintains backward compat).
    #[allow(dead_code)]
    fn analyze_node(
        node: &PlanNodeEnum,
        next_id: &mut usize,
        all_pipelines: &mut Vec<Pipeline>,
    ) -> AnalysisResult {
        let mut node_to_pipeline = std::collections::HashMap::new();
        Self::analyze_node_with_map(node, next_id, all_pipelines, &mut node_to_pipeline)
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
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorSearch(_) => vec![],
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorLookup(_) => vec![],
            #[cfg(feature = "qdrant")]
            PlanNodeEnum::VectorMatch(_) => vec![],
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

    /// Connect pipeline edges using the node-to-pipeline mapping.
    ///
    /// For each pipeline with `source == Exchange`, find the upstream
    /// pipeline(s) by looking up the plan node dependencies in
    /// `node_to_pipeline`.  This correctly handles binary-input nodes
    /// (joins) and multi-input nodes where the heuristic fails.
    fn connect_pipelines(
        pipelines: &mut [Pipeline],
        node_to_pipeline: &std::collections::HashMap<i64, usize>,
    ) {
        let count = pipelines.len();
        let mut feeds_into: Vec<Vec<usize>> = vec![Vec::new(); count];

        for (i, pipeline) in pipelines.iter().enumerate() {
            if matches!(pipeline.source, PipelineSource::Exchange) {
                let node = &pipeline.root_node;
                let inputs = Self::get_inputs(node);
                for input in &inputs {
                    if let Some(&upstream_id) = node_to_pipeline.get(&input.id()) {
                        if upstream_id != i {
                            feeds_into[upstream_id].push(pipeline.id);
                        }
                    }
                }
            }
        }

        // Collect pipeline IDs for lookup
        let pipeline_ids: Vec<usize> = pipelines.iter().map(|p| p.id).collect();

        // Write upstream_ids for each pipeline based on the reverse map
        for (i, pipeline) in pipelines.iter_mut().enumerate() {
            let mut upstream = Vec::new();
            for (j, deps) in feeds_into.iter().enumerate() {
                if deps.contains(&pipeline_ids[i]) {
                    upstream.push(pipeline_ids[j]);
                }
            }
            upstream.sort();
            upstream.dedup();
            pipeline.upstream_ids = upstream;
        }

        // Populate downstream_ids based on the forward map
        for (i, pipeline) in pipelines.iter_mut().enumerate() {
            pipeline.downstream_ids = feeds_into[i].clone();
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
