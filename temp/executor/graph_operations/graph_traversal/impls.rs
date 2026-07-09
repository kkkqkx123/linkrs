use super::*;
use crate::query::executor::graph_operations::graph_traversal::algorithms::ShortestPathAlgorithmType;
use crate::query::executor::graph_operations::graph_traversal::expand::ExpandExecutor;
use crate::query::executor::graph_operations::graph_traversal::expand_all::ExpandAllExecutor;
use crate::query::executor::graph_operations::graph_traversal::shortest_path::ShortestPathExecutor;
use crate::query::executor::graph_operations::graph_traversal::traits::TraversalStats;
use crate::query::executor::graph_operations::graph_traversal::traverse::TraverseExecutor;

/// Macro definition: Implements general statistical information for executors that have access to node statistics.
macro_rules! impl_graph_traversal_executor_with_stats {
    ($executor:ty, $visited_nodes_field:ident) => {
        impl<S: crate::storage::StorageClient + Send + 'static> GraphTraversalExecutor<S>
            for $executor
        {
            fn set_edge_direction(
                &mut self,
                direction: crate::query::executor::base::EdgeDirection,
            ) {
                self.edge_direction = direction;
            }

            fn set_edge_types(&mut self, edge_types: Option<Vec<String>>) {
                self.edge_types = edge_types;
            }

            fn set_max_depth(&mut self, max_depth: Option<usize>) {
                self.max_depth = max_depth;
            }

            fn get_edge_direction(&self) -> crate::query::executor::base::EdgeDirection {
                self.edge_direction.clone()
            }

            fn get_edge_types(&self) -> Option<Vec<String>> {
                self.edge_types.clone()
            }

            fn get_max_depth(&self) -> Option<usize> {
                self.max_depth
            }

            fn validate_config(&self) -> Result<(), String> {
                if let Some(max_depth) = self.max_depth {
                    if max_depth == 0 {
                        return Err("Maximum depth cannot be 0".to_string());
                    }
                }
                Ok(())
            }

            fn get_stats(&self) -> TraversalStats {
                TraversalStats {
                    nodes_visited: self.$visited_nodes_field.len(),
                    edges_traversed: 0,
                    execution_time_ms: 0,
                    max_depth_reached: self.max_depth.unwrap_or(0),
                }
            }
        }
    };
}

// Implementing general features for executors with a `visited_nodes` field using macros with statistics
impl_graph_traversal_executor_with_stats!(ExpandExecutor<S>, visited_nodes);
impl_graph_traversal_executor_with_stats!(ExpandAllExecutor<S>, visited_nodes);
impl_graph_traversal_executor_with_stats!(TraverseExecutor<S>, visited_nodes);

/// Provide a special implementation for ShortestPathExecutor.
impl<S: crate::storage::StorageClient + Send + 'static> GraphTraversalExecutor<S>
    for ShortestPathExecutor<S>
{
    fn set_edge_direction(&mut self, direction: crate::query::executor::base::EdgeDirection) {
        self.edge_direction = direction;
    }

    fn set_edge_types(&mut self, edge_types: Option<Vec<String>>) {
        self.edge_types = edge_types;
    }

    fn set_max_depth(&mut self, max_depth: Option<usize>) {
        // The shortest path algorithm supports a maximum depth limit.
        // This can be used to limit the search scope and prevent infinite loops.
        self.max_depth = max_depth;
    }

    fn get_edge_direction(&self) -> crate::query::executor::base::EdgeDirection {
        self.edge_direction
    }

    fn get_edge_types(&self) -> Option<Vec<String>> {
        self.edge_types.clone()
    }

    fn get_max_depth(&self) -> Option<usize> {
        // Return the maximum depth actually set.
        self.max_depth
    }

    fn validate_config(&self) -> Result<(), String> {
        // Special validation of the shortest path
        if let Some(max_depth) = self.max_depth {
            if max_depth == 0 {
                return Err("Maximum depth of shortest path cannot be 0".to_string());
            }
        }

        // Verify whether the selection of the validation algorithm is effective.
        let algorithm = self.get_algorithm();
        match algorithm {
            ShortestPathAlgorithmType::BFS
            | ShortestPathAlgorithmType::Dijkstra
            | ShortestPathAlgorithmType::AStar => {
                // All enumerated variants are valid.
            }
        }

        // Verify the configuration of the starting and ending nodes.
        if self.get_start_vertex_ids().is_empty() {
            return Err(
                "Shortest path must be configured with at least one start node".to_string(),
            );
        }
        if self.get_end_vertex_ids().is_empty() {
            return Err("Shortest path must be configured with at least one end node".to_string());
        }

        Ok(())
    }

    fn get_stats(&self) -> TraversalStats {
        // Provide statistical information specific to the shortest path.
        TraversalStats {
            nodes_visited: self.nodes_visited,
            edges_traversed: self.edges_traversed,
            execution_time_ms: self.execution_time_ms,
            max_depth_reached: self.max_depth_reached,
        }
    }
}
