//! Test Scenario Module
//!
//! Provides a high-level API for writing integration tests with fluent interface

use crate::common::TestResult;
use graphdb_query::core::types::VertexId;
use graphdb_query::core::Value;
use graphdb_query::query::executor::base::ExecutionResult;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use graphdb_query::storage::{GraphStorage, StorageReader, StorageSchemaContextOps};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::common::TestStorage;

/// Test scenario builder for fluent test writing
pub struct TestScenario {
    storage: Arc<RwLock<GraphStorage>>,
    pipeline: QueryPipelineManager<GraphStorage>,
    last_result: Option<ExecutionResult>,
    last_error: Option<String>,
    current_space: Option<graphdb_query::core::types::SpaceInfo>,
}

impl TestScenario {
    /// Create a new test scenario
    pub fn new() -> TestResult<Self> {
        let test_storage = TestStorage::new()?;
        let storage = test_storage.storage();

        use graphdb_query::core::stats::StatsManager;
        use graphdb_query::query::optimizer::OptimizerEngine;
        use std::sync::Arc;

        let stats_manager = Arc::new(StatsManager::new());
        let optimizer = Arc::new(OptimizerEngine::default());
        let schema_manager = {
            let storage_guard = storage.write();
            storage_guard
                .get_schema_manager()
                .expect("Storage should provide a schema manager")
        };
        let pipeline =
            QueryPipelineManager::with_optimizer(storage.clone(), stats_manager, optimizer)
                .with_schema_manager(schema_manager);

        Ok(Self {
            storage,
            pipeline,
            last_result: None,
            last_error: None,
            current_space: None,
        })
    }

    // ==================== Execution Methods ====================

    /// Execute a DCL statement
    pub fn exec_dcl(mut self, query: &str) -> Self {
        match self
            .pipeline
            .execute_query_with_space(query, self.current_space.clone())
        {
            Ok(result) => match &result {
                ExecutionResult::Error(e) => {
                    self.last_error = Some(e.clone());
                    self.last_result = Some(result);
                }
                _ => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
            },
            Err(e) => {
                self.last_error = Some(format!("{:?}", e));
                self.last_result = None;
            }
        }
        self
    }

    /// Execute a DDL statement
    pub fn exec_ddl(self, query: &str) -> Self {
        self.exec_dcl(query)
    }

    /// Execute a DML statement
    pub fn exec_dml(self, query: &str) -> Self {
        self.exec_dcl(query)
    }

    // ==================== Query Execution (generic) ====================

    /// Execute a query
    pub fn query(mut self, query: &str) -> Self {
        match self
            .pipeline
            .execute_query_with_space(query, self.current_space.clone())
        {
            Ok(result) => match &result {
                ExecutionResult::Error(e) => {
                    self.last_error = Some(e.clone());
                    self.last_result = Some(result);
                }
                _ => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
            },
            Err(e) => {
                self.last_error = Some(format!("{:?}", e));
                self.last_result = None;
            }
        }
        self
    }

    // ==================== Setup Methods ====================

    /// Setup graph space
    pub fn setup_space(mut self, space_name: &str) -> Self {
        let query = format!("CREATE SPACE IF NOT EXISTS {}", space_name);

        match self.pipeline.execute_query_with_space(&query, None) {
            Ok(result) => match &result {
                ExecutionResult::Success | ExecutionResult::Empty => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
                ExecutionResult::Error(e) => {
                    self.last_error = Some(format!("CREATE SPACE failed: {}", e));
                    self.last_result = Some(result);
                    return self;
                }
                _ => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
            },
            Err(e) => {
                self.last_error = Some(format!("{:?}", e));
                self.last_result = None;
                return self;
            }
        }

        let space_result = {
            let storage_guard = self.storage.write();
            storage_guard.get_space(space_name)
        };

        match space_result {
            Ok(Some(space)) => {
                self.current_space = Some(space);
            }
            Ok(None) => {
                self.last_error = Some(format!(
                    "Space '{}' not found in storage after creation",
                    space_name
                ));
                return self;
            }
            Err(e) => {
                self.last_error = Some(format!("Failed to get space from storage: {}", e));
                return self;
            }
        }

        self
    }

    /// Setup schema with tags and edges
    pub fn setup_schema(mut self, ddls: Vec<&str>) -> Self {
        for ddl in ddls {
            match self
                .pipeline
                .execute_query_with_space(ddl, self.current_space.clone())
            {
                Ok(result) => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
                Err(e) => {
                    self.last_error = Some(format!("{:?}", e));
                    self.last_result = None;
                }
            }
        }
        self
    }

    /// Load test data
    pub fn load_data(mut self, dmls: Vec<&str>) -> Self {
        for dml in dmls {
            match self.pipeline.execute_query(dml) {
                Ok(result) => {
                    self.last_result = Some(result);
                    self.last_error = None;
                }
                Err(e) => {
                    self.last_error = Some(format!("{:?}", e));
                    self.last_result = None;
                }
            }
        }
        self
    }

    // ==================== Assertion Methods ====================

    /// Assert that the last operation succeeded
    pub fn assert_success(self) -> Self {
        assert!(
            self.last_error.is_none(),
            "Expected success but got error: {:?}",
            self.last_error
        );
        self
    }

    /// Assert that the last operation failed
    pub fn assert_error(self) -> Self {
        assert!(
            self.last_error.is_some(),
            "Expected error but operation succeeded"
        );
        self
    }

    /// Print the last result for debugging (only in test mode)
    #[cfg(test)]
    pub fn debug_print_result(self) -> Self {
        eprintln!("\n=== Debug: Last Query Result ===");
        if let Some(ref result) = self.last_result {
            match result {
                ExecutionResult::DataSet(ds) => {
                    eprintln!("Columns: {:?}", ds.col_names);
                    eprintln!("Rows ({}):", ds.rows.len());
                    for (i, row) in ds.rows.iter().enumerate() {
                        eprintln!("  Row {}: {:?}", i, row);
                    }
                }
                _ => {
                    eprintln!("Result: {:?}", result);
                }
            }
        } else {
            eprintln!("No result available");
        }
        eprintln!("================================\n");
        self
    }

    /// Assert result count
    pub fn assert_result_count(self, expected: usize) -> Self {
        let actual = self.last_result.as_ref().map(|r| r.count()).unwrap_or(0);
        assert_eq!(
            actual, expected,
            "Expected {} results but got {}",
            expected, actual
        );
        self
    }

    /// Assert result is empty
    pub fn assert_result_empty(self) -> Self {
        self.assert_result_count(0)
    }

    /// Print current space info for debugging
    #[cfg(test)]
    pub fn debug_print_space(self) -> Self {
        eprintln!("\n=== Debug: Current Space ===");
        if let Some(ref space) = self.current_space {
            eprintln!("Space ID: {}", space.space_id);
            eprintln!("Space Name: {}", space.space_name);
        } else {
            eprintln!("No space selected");
        }
        eprintln!("================================\n");
        self
    }

    /// Assert result columns
    pub fn assert_result_columns(self, expected: &[&str]) -> Self {
        if let Some(ref result) = self.last_result {
            let col_names: Vec<String> = match result {
                ExecutionResult::DataSet(ds) => ds.col_names.clone(),
                _ => vec![],
            };

            let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
            assert_eq!(
                col_names, expected,
                "Column names don't match. Expected {:?}, got {:?}",
                expected, col_names
            );
        } else {
            panic!("No result to check columns");
        }
        self
    }

    /// Assert result contains specific values
    pub fn assert_result_contains(self, expected: Vec<Value>) -> Self {
        if let Some(ref result) = self.last_result {
            let rows: Vec<Vec<Value>> = match result {
                ExecutionResult::DataSet(ds) => ds.rows.clone(),
                _ => vec![],
            };

            // Check if any row contains all expected values (subset match)
            let found = rows
                .iter()
                .any(|row| expected.iter().all(|exp_val| row.contains(exp_val)));
            assert!(
                found,
                "Expected to find row containing {:?} in results, actual rows: {:?}",
                expected, rows
            );
        } else {
            panic!("No result to check");
        }
        self
    }

    /// Assert vertex or edge in result has a specific property value
    pub fn assert_vertex_or_edge_has_property(
        self,
        prop_name: &str,
        expected_value: Value,
    ) -> Self {
        if let Some(ref result) = self.last_result {
            let rows: Vec<Vec<Value>> = match result {
                ExecutionResult::DataSet(ds) => ds.rows.clone(),
                _ => vec![],
            };

            let found = rows.iter().any(|row| {
                row.iter().any(|val| match val {
                    Value::Vertex(v) => v
                        .tags
                        .iter()
                        .any(|tag| tag.properties.get(prop_name) == Some(&expected_value)),
                    Value::Edge(e) => e.props.get(prop_name) == Some(&expected_value),
                    _ => false,
                })
            });

            assert!(
                found,
                "Expected to find vertex/edge with property '{}' = {:?} in results, actual rows: {:?}",
                prop_name, expected_value, rows
            );
        } else {
            panic!("No result to check");
        }
        self
    }

    /// Assert plan contains specific operator
    /// This is useful for testing EXPLAIN output
    pub fn assert_plan_contains(self, operator: &str) -> Self {
        if let Some(ref result) = self.last_result {
            let plan_str = match result {
                ExecutionResult::DataSet(ds) => format!("{:?}", ds),
                _ => String::new(),
            };

            assert!(
                plan_str.contains(operator),
                "Expected '{}' in plan, but got: {}",
                operator,
                plan_str
            );
        } else {
            panic!("No result to check plan");
        }
        self
    }

    /// Assert plan contains any of the specified operators
    pub fn assert_plan_contains_any(self, operators: &[&str]) -> Self {
        if let Some(ref result) = self.last_result {
            let plan_str = match result {
                ExecutionResult::DataSet(ds) => format!("{:?}", ds),
                _ => String::new(),
            };

            let found = operators.iter().any(|op| plan_str.contains(op));
            assert!(
                found,
                "Expected one of {:?} in plan, but got: {}",
                operators, plan_str
            );
        } else {
            panic!("No result to check plan");
        }
        self
    }

    /// Get the plan string for custom assertions
    pub fn get_plan_string(&self) -> Option<String> {
        self.last_result.as_ref().map(|result| match result {
            ExecutionResult::DataSet(ds) => format!("{:?}", ds),
            _ => String::new(),
        })
    }

    // ==================== Data Validation Methods ====================

    /// Assert vertex exists
    pub fn assert_vertex_exists(mut self, vid: i64, tag: &str) -> Self {
        let query = format!("FETCH PROP ON {} {}", tag, vid);
        match self
            .pipeline
            .execute_query_with_space(&query, self.current_space.clone())
        {
            Ok(result) => {
                assert!(
                    result.count() > 0,
                    "Expected vertex {} with tag {} to exist",
                    vid,
                    tag
                );
            }
            Err(e) => {
                panic!("Failed to check vertex existence: {:?}", e);
            }
        }
        self
    }

    /// Assert vertex does not exist
    pub fn assert_vertex_not_exists(mut self, vid: i64, tag: &str) -> Self {
        let query = format!("FETCH PROP ON {} {}", tag, vid);
        match self
            .pipeline
            .execute_query_with_space(&query, self.current_space.clone())
        {
            Ok(result) => {
                assert!(
                    result.count() == 0,
                    "Expected vertex {} with tag {} to not exist",
                    vid,
                    tag
                );
            }
            Err(_e) => {
                // Error might mean vertex doesn't exist, which is what we want
            }
        }
        self
    }

    /// Assert vertex has specific properties
    pub fn assert_vertex_props(
        mut self,
        vid: i64,
        tag: &str,
        expected: HashMap<&str, Value>,
    ) -> Self {
        let query = format!("FETCH PROP ON {} {}", tag, vid);
        match self
            .pipeline
            .execute_query_with_space(&query, self.current_space.clone())
        {
            Ok(result) => {
                let props = self.extract_props(&result);
                for (key, value) in expected {
                    assert_eq!(
                        props.get(key),
                        Some(&value),
                        "Property {} mismatch for vertex {}. Expected {:?}, got {:?}",
                        key,
                        vid,
                        value,
                        props.get(key)
                    );
                }
            }
            Err(e) => {
                panic!("Failed to get vertex properties: {:?}", e);
            }
        }
        self
    }

    /// Assert edge exists
    pub fn assert_edge_exists(self, src: i64, dst: i64, edge_type: &str) -> Self {
        let space_name = self
            .current_space
            .as_ref()
            .map(|s| s.space_name.clone())
            .unwrap_or_default();
        let src_vid = VertexId::from_int64(src);
        let dst_vid = VertexId::from_int64(dst);
        let found = {
            let storage_guard = self.storage.write();
            let edges = storage_guard
                .scan_edges_by_type(&space_name, edge_type)
                .unwrap_or_default();
            edges
                .iter()
                .any(|e| *e.src() == src_vid && *e.dst() == dst_vid && e.edge_type == edge_type)
        };
        assert!(
            found,
            "Expected edge {} -> {} with type {} to exist",
            src, dst, edge_type
        );
        self
    }

    /// Assert edge does not exist
    pub fn assert_edge_not_exists(self, src: i64, dst: i64, edge_type: &str) -> Self {
        let space_name = self
            .current_space
            .as_ref()
            .map(|s| s.space_name.clone())
            .unwrap_or_default();
        let src_vid = VertexId::from_int64(src);
        let dst_vid = VertexId::from_int64(dst);
        let found = {
            let storage_guard = self.storage.write();
            let edges = storage_guard
                .scan_edges_by_type(&space_name, edge_type)
                .unwrap_or_default();
            edges
                .iter()
                .any(|e| *e.src() == src_vid && *e.dst() == dst_vid && e.edge_type == edge_type)
        };
        assert!(
            !found,
            "Expected edge {} -> {} with type {} to not exist",
            src, dst, edge_type
        );
        self
    }

    /// Assert tag exists
    pub fn assert_tag_exists(mut self, tag: &str) -> Self {
        let query = format!("DESC TAG {}", tag);
        match self
            .pipeline
            .execute_query_with_space(&query, self.current_space.clone())
        {
            Ok(result) => {
                assert!(result.count() > 0, "Expected tag {} to exist", tag);
            }
            Err(e) => {
                panic!("Failed to check tag existence: {:?}", e);
            }
        }
        self
    }

    /// Assert tag does not exist
    pub fn assert_tag_not_exists(mut self, tag: &str) -> Self {
        let query = format!("DESC TAG {}", tag);
        match self
            .pipeline
            .execute_query_with_space(&query, self.current_space.clone())
        {
            Ok(result) => {
                assert!(result.count() == 0, "Expected tag {} to not exist", tag);
            }
            Err(_e) => {
                // Error might mean tag doesn't exist, which is what we want
            }
        }
        self
    }

    /// Assert vertex count
    pub fn assert_vertex_count(self, tag: &str, expected: usize) -> Self {
        // Directly query storage to count vertices with the given tag
        let space_name = self
            .current_space
            .as_ref()
            .map(|s| s.space_name.clone())
            .unwrap_or_default();
        let actual = {
            let storage_guard = self.storage.write();
            storage_guard
                .scan_vertices_by_tag(&space_name, tag)
                .map(|vertices| vertices.len())
                .unwrap_or(0)
        };

        assert_eq!(
            actual, expected,
            "Expected {} vertices with tag {}, got {}",
            expected, tag, actual
        );
        self
    }

    /// Assert edge count
    pub fn assert_edge_count(self, edge_type: &str, expected: usize) -> Self {
        // Directly query storage to count edges with the given type
        let space_name = self
            .current_space
            .as_ref()
            .map(|s| s.space_name.clone())
            .unwrap_or_default();
        let actual = {
            let storage_guard = self.storage.write();
            storage_guard
                .scan_edges_by_type(&space_name, edge_type)
                .map(|edges| edges.len())
                .unwrap_or(0)
        };

        assert_eq!(
            actual, expected,
            "Expected {} edges with type {}, got {}",
            expected, edge_type, actual
        );
        self
    }

    // ==================== Helper Methods ====================

    fn extract_props(&self, result: &ExecutionResult) -> HashMap<String, Value> {
        let mut props = HashMap::new();

        if let ExecutionResult::DataSet(ds) = result {
            if let Some(row) = ds.rows.first() {
                for (i, col_name) in ds.col_names.iter().enumerate() {
                    if let Some(value) = row.get(i) {
                        if let Value::Vertex(vertex) = value {
                            for tag in vertex.tags() {
                                for (prop_name, prop_value) in &tag.properties {
                                    props.insert(prop_name.clone(), prop_value.clone());
                                }
                            }
                            for (prop_name, prop_value) in &vertex.properties {
                                props.insert(prop_name.clone(), prop_value.clone());
                            }
                        } else if let Value::Edge(edge) = value {
                            for (prop_name, prop_value) in &edge.props {
                                props.insert(prop_name.clone(), prop_value.clone());
                            }
                        } else {
                            props.insert(col_name.clone(), value.clone());
                        }
                    }
                }
            }
        }

        props
    }
}

impl Default for TestScenario {
    fn default() -> Self {
        Self::new().expect("Failed to create default TestScenario")
    }
}
