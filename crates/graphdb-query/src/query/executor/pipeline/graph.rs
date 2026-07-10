//! Pipeline graph types
//!
//! Defines Pipeline, PipelineGraph, and associated enums for
//! representing the pipeline DAG structure.

use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;

use super::breaker::PipelineBreakerKind;

/// How data enters a pipeline
#[derive(Debug, Clone)]
pub enum PipelineSource {
    /// Pipeline reads from a physical plan node (scan, argument, etc.)
    PlanNode(Box<PlanNodeEnum>),
    /// Pipeline receives data from an upstream pipeline via exchange
    Exchange,
}

/// How data leaves a pipeline
#[derive(Debug, Clone)]
pub enum PipelineSink {
    /// Final output to the query result
    Root,
    /// Pipeline sends data downstream via exchange
    Exchange,
    /// Pipeline materializes its output (breaker boundary)
    Breaker(PipelineBreakerKind),
}

/// A single pipeline segment — a chain of streaming operators
/// terminated by a breaker or the root.
///
/// Each pipeline holds a sub-tree of the original plan that can be
/// converted into a single StreamingExecutor chain for execution.
#[derive(Debug, Clone)]
pub struct Pipeline {
    /// Unique pipeline ID
    pub id: usize,
    /// Human-readable name for explain output
    pub name: String,
    /// The root node of this pipeline's sub-tree in the physical plan
    pub root_node: PlanNodeEnum,
    /// How data enters this pipeline
    pub source: PipelineSource,
    /// How data leaves this pipeline
    pub sink: PipelineSink,
    /// IDs of upstream pipelines this pipeline depends on
    pub upstream_ids: Vec<usize>,
    /// IDs of downstream pipelines that depend on this one
    pub downstream_ids: Vec<usize>,
}

impl Pipeline {
    pub fn new(
        id: usize,
        name: String,
        root_node: PlanNodeEnum,
        source: PipelineSource,
        sink: PipelineSink,
    ) -> Self {
        Self {
            id,
            name,
            root_node,
            source,
            sink,
            upstream_ids: Vec::new(),
            downstream_ids: Vec::new(),
        }
    }

    /// Check if this pipeline's sink matches the given sink kind.
    pub fn matches_sink(&self, other: &PipelineSink) -> bool {
        std::mem::discriminant(&self.sink) == std::mem::discriminant(other)
    }
}

/// The complete pipeline DAG for a query plan
#[derive(Debug, Clone)]
pub struct PipelineGraph {
    /// All pipelines in the graph
    pub pipelines: Vec<Pipeline>,
    /// ID of the pipeline that produces the final output
    pub root_pipeline_id: usize,
}

impl PipelineGraph {
    pub fn new(pipelines: Vec<Pipeline>, root_pipeline_id: usize) -> Self {
        Self {
            pipelines,
            root_pipeline_id,
        }
    }

    pub fn pipeline_count(&self) -> usize {
        self.pipelines.len()
    }

    /// Return an iterator over all pipelines in topological order
    /// (leaves first, root last).
    pub fn topological_order(&self) -> Vec<usize> {
        let mut visited = vec![false; self.pipelines.len()];
        let mut order = Vec::new();

        fn dfs(pipelines: &[Pipeline], id: usize, visited: &mut Vec<bool>, order: &mut Vec<usize>) {
            if visited[id] {
                return;
            }
            visited[id] = true;
            for &up_id in &pipelines[id].upstream_ids {
                dfs(pipelines, up_id, visited, order);
            }
            order.push(id);
        }

        for i in 0..self.pipelines.len() {
            dfs(&self.pipelines, i, &mut visited, &mut order);
        }
        order
    }

    /// Generate a human-readable explain string for this pipeline graph
    pub fn explain(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "PipelineGraph ({} pipelines, root={})\n",
            self.pipelines.len(),
            self.root_pipeline_id
        ));
        for p in &self.pipelines {
            s.push_str(&format!(
                "  Pipeline {} [{}]: ",
                p.id,
                p.name
            ));
            if p.upstream_ids.is_empty() {
                s.push_str("source");
            } else {
                s.push_str(&format!("inputs=[{}]", p.upstream_ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(",")));
            }
            s.push_str(&format!(" -> {}", match &p.sink {
                PipelineSink::Root => "root".to_string(),
                PipelineSink::Exchange => "exchange".to_string(),
                PipelineSink::Breaker(k) => format!("breaker({})", k.name()),
            }));
            s.push('\n');
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;

    #[test]
    fn test_single_pipeline_graph() {
        let root = PlanNodeEnum::Start(StartNode::new());
        let pipeline = Pipeline::new(
            0,
            "scan".to_string(),
            root,
            PipelineSource::PlanNode(Box::new(PlanNodeEnum::Start(StartNode::new()))),
            PipelineSink::Root,
        );
        let graph = PipelineGraph::new(vec![pipeline], 0);
        assert_eq!(graph.pipeline_count(), 1);
        assert_eq!(graph.topological_order(), vec![0]);
    }

    #[test]
    fn test_topological_order() {
        // Pipeline 1 depends on 0, Pipeline 2 depends on 1
        let pipeline0 = Pipeline::new(
            0,
            "scan".to_string(),
            PlanNodeEnum::Start(StartNode::new()),
            PipelineSource::PlanNode(Box::new(PlanNodeEnum::Start(StartNode::new()))),
            PipelineSink::Breaker(PipelineBreakerKind::Aggregate),
        );
        let mut pipeline1 = Pipeline::new(
            1,
            "agg".to_string(),
            PlanNodeEnum::Start(StartNode::new()),
            PipelineSource::Exchange,
            PipelineSink::Root,
        );
        pipeline1.upstream_ids = vec![0];

        let graph = PipelineGraph::new(vec![pipeline0, pipeline1], 1);
        let order = graph.topological_order();
        assert_eq!(order, vec![0, 1], "Expected leaves-first order");
    }
}
