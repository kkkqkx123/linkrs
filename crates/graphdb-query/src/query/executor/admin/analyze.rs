//! AnalyzeExecutor – An analysis executor
//!
//! Responsible for collecting and updating statistical information in the database, which is used for query optimization.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::types::EdgeDirection;
use crate::core::Value;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::optimizer::stats::{EdgeTypeStatistics, StatisticsManager, TagStatistics};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::query::DataSet;
use crate::storage::StorageReader;

/// Analysis of the target type
#[derive(Debug, Clone)]
pub enum AnalyzeTarget {
    /// Analyze all objects.
    All,
    /// Analyze the specified tags
    Tag(String),
    /// Analyze the specified edge type
    EdgeType(String),
    /// Analyze the specified attribute
    Property {
        tag: Option<String>,
        property: String,
    },
}

/// Analysis of the executor
///
/// This executor is responsible for collecting statistical information from the database, which is used for the cost calculation of the query optimizer.
/// The execution is triggered by the ANALYZE command.
#[derive(Debug)]
pub struct AnalyzeExecutor<S: StorageReader> {
    base: BaseExecutor<S>,
    target: AnalyzeTarget,
    stats_manager: Arc<RwLock<StatisticsManager>>,
}

impl<S: StorageReader> AnalyzeExecutor<S> {
    /// Create a new AnalyzeExecutor.
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AnalyzeExecutor".to_string(), storage, expr_context),
            target: AnalyzeTarget::All,
            stats_manager: Arc::new(RwLock::new(StatisticsManager::new())),
        }
    }

    /// Create an AnalyzeExecutor with a specified goal
    pub fn with_target(
        id: i64,
        storage: Arc<RwLock<S>>,
        target: AnalyzeTarget,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AnalyzeExecutor".to_string(), storage, expr_context),
            target,
            stats_manager: Arc::new(RwLock::new(StatisticsManager::new())),
        }
    }

    /// Set analysis objectives
    pub fn set_target(&mut self, target: AnalyzeTarget) {
        self.target = target;
    }

    /// Statistics Information Manager
    pub fn stats_manager(&self) -> Arc<RwLock<StatisticsManager>> {
        self.stats_manager.clone()
    }

    /// Collecting tag statistics information
    fn collect_tag_stats(
        &self,
        storage: &S,
        space: &str,
        tag_name: &str,
    ) -> Result<TagStatistics, crate::core::StorageError> {
        let mut stats = TagStatistics::new(tag_name.to_string());

        // Scan all the vertices of that tag.
        let vertices = storage.scan_vertices_by_tag(space, tag_name)?;
        stats.vertex_count = vertices.len() as u64;

        if stats.vertex_count > 0 {
            // Calculate the average degree.
            let (avg_out, avg_in) = self.calculate_average_degrees(storage, space, &vertices)?;
            stats.avg_out_degree = avg_out;
            stats.avg_in_degree = avg_in;
        }

        Ok(stats)
    }

    /// Calculate the average outdegree and indegree of the vertices.
    fn calculate_average_degrees(
        &self,
        storage: &S,
        space: &str,
        vertices: &[crate::core::Vertex],
    ) -> Result<(f64, f64), crate::core::StorageError> {
        let mut total_out_degree: usize = 0;
        let mut total_in_degree: usize = 0;

        for vertex in vertices {
            // Get it out (of somewhere).
            let out_edges = storage.get_node_edges(space, vertex.vid(), EdgeDirection::Out)?;
            total_out_degree += out_edges.len();

            // Get the content inside.
            let in_edges = storage.get_node_edges(space, vertex.vid(), EdgeDirection::In)?;
            total_in_degree += in_edges.len();
        }

        let count = vertices.len();
        let avg_out = if count > 0 {
            total_out_degree as f64 / count as f64
        } else {
            0.0
        };
        let avg_in = if count > 0 {
            total_in_degree as f64 / count as f64
        } else {
            0.0
        };

        Ok((avg_out, avg_in))
    }

    /// Collecting statistical information on edge types
    fn collect_edge_stats(
        &self,
        storage: &S,
        space: &str,
        edge_type: &str,
    ) -> Result<EdgeTypeStatistics, crate::core::StorageError> {
        let mut stats = EdgeTypeStatistics::new(edge_type.to_string());

        // Scan all the edges of this type.
        let edges = storage.scan_edges_by_type(space, edge_type)?;
        stats.edge_count = edges.len() as u64;

        if stats.edge_count > 0 {
            // Calculate the number of unique source vertices and target vertices.
            let mut unique_src = std::collections::HashSet::new();
            let mut unique_dst = std::collections::HashSet::new();

            for edge in &edges {
                unique_src.insert(*edge.src());
                unique_dst.insert(*edge.dst());
            }

            stats.unique_src_vertices = unique_src.len() as u64;
            let unique_dst_count = unique_dst.len() as u64;

            // Calculate the average outdegree and indegree.
            stats.avg_out_degree = if stats.unique_src_vertices > 0 {
                stats.edge_count as f64 / stats.unique_src_vertices as f64
            } else {
                0.0
            };
            stats.avg_in_degree = if unique_dst_count > 0 {
                stats.edge_count as f64 / unique_dst_count as f64
            } else {
                0.0
            };
        }

        Ok(stats)
    }

    /// Perform the analysis and return the resulting dataset.
    fn execute_analysis(&self, space: &str) -> crate::query::executor::base::DBResult<DataSet> {
        let storage = self.get_storage();
        let storage_guard = storage.read();

        let mut rows = Vec::new();

        match &self.target {
            AnalyzeTarget::All => {
                // Retrieve all tags
                let tags = storage_guard.list_tags(space)?;
                for tag_info in &tags {
                    let stats =
                        self.collect_tag_stats(&*storage_guard, space, &tag_info.tag_name)?;

                    // Update the Statistics Information Manager
                    {
                        let manager = self.stats_manager.write();
                        manager.update_tag_stats(stats.clone());
                    }

                    rows.push(vec![
                        Value::String("TAG".to_string()),
                        Value::String(stats.tag_name.clone()),
                        Value::BigInt(stats.vertex_count as i64),
                        Value::Double(stats.avg_out_degree),
                        Value::Double(stats.avg_in_degree),
                    ]);
                }

                // Retrieve all edge types
                let edge_types = storage_guard.list_edge_types(space)?;
                for edge_type_info in &edge_types {
                    let stats = self.collect_edge_stats(
                        &*storage_guard,
                        space,
                        &edge_type_info.edge_type_name,
                    )?;

                    // Update the Statistics Information Manager
                    {
                        let manager = self.stats_manager.write();
                        manager.update_edge_stats(stats.clone());
                    }

                    rows.push(vec![
                        Value::String("EDGE".to_string()),
                        Value::String(stats.edge_type.clone()),
                        Value::BigInt(stats.edge_count as i64),
                        Value::Double(stats.avg_out_degree),
                        Value::Double(stats.avg_in_degree),
                    ]);
                }
            }
            AnalyzeTarget::Tag(tag_name) => {
                let stats = self.collect_tag_stats(&*storage_guard, space, tag_name)?;

                // Update the Statistics Information Manager
                {
                    let manager = self.stats_manager.write();
                    manager.update_tag_stats(stats.clone());
                }

                rows.push(vec![
                    Value::String("TAG".to_string()),
                    Value::String(stats.tag_name.clone()),
                    Value::BigInt(stats.vertex_count as i64),
                    Value::Double(stats.avg_out_degree),
                    Value::Double(stats.avg_in_degree),
                ]);
            }
            AnalyzeTarget::EdgeType(edge_type) => {
                let stats = self.collect_edge_stats(&*storage_guard, space, edge_type)?;

                // Update the Statistics Information Manager
                {
                    let manager = self.stats_manager.write();
                    manager.update_edge_stats(stats.clone());
                }

                rows.push(vec![
                    Value::String("EDGE".to_string()),
                    Value::String(stats.edge_type.clone()),
                    Value::BigInt(stats.edge_count as i64),
                    Value::Double(stats.avg_out_degree),
                    Value::Double(stats.avg_in_degree),
                ]);
            }
            AnalyzeTarget::Property { tag, property } => {
                // Collection of attribute statistics information
                // For the current simplified implementation, only basic information is returned.
                rows.push(vec![
                    Value::String("PROPERTY".to_string()),
                    Value::String(format!("{}.{}", tag.as_deref().unwrap_or("*"), property)),
                    Value::Int(0),
                    Value::Float(0.0),
                    Value::Float(0.0),
                ]);
            }
        }

        Ok(DataSet {
            col_names: vec![
                "Type".to_string(),
                "Name".to_string(),
                "Count".to_string(),
                "Avg Out Degree".to_string(),
                "Avg In Degree".to_string(),
            ],
            rows,
        })
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for AnalyzeExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        // Get the current space name from the context variable.
        let space = self
            .base
            .context
            .get_variable("current_space")
            .and_then(|v| match v {
                Value::String(s) => Some(s.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "default".to_string());

        match self.execute_analysis(&space) {
            Ok(dataset) => Ok(ExecutionResult::DataSet(dataset)),
            Err(e) => Ok(ExecutionResult::Error(format!("ANALYZE failed: {}", e))),
        }
    }

    fn open(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "AnalyzeExecutor"
    }

    fn description(&self) -> &str {
        "Collects database statistics for query optimization"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for AnalyzeExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_target_clone() {
        let target = AnalyzeTarget::Tag("Person".to_string());
        let cloned = target.clone();

        match cloned {
            AnalyzeTarget::Tag(name) => assert_eq!(name, "Person"),
            _ => panic!("Expected Tag target"),
        }
    }

    #[test]
    fn test_analyze_target_debug() {
        let target = AnalyzeTarget::All;
        let debug_str = format!("{:?}", target);
        assert!(debug_str.contains("All"));
    }
}
