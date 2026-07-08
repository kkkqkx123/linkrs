//! Graph Traversal Integrated Testing
//!
//! Test scope:
//! The executor for the shortest path algorithm is created and configured.
//! Multi-source shortest path algorithm
//! Subgraph query executor
//! Algorithm context and configuration
//! Path data structure

mod common;

use common::TestStorage;
use graphdb_query::core::types::VertexId;
use graphdb_query::core::vertex_edge_path::Tag;
use graphdb_query::core::{Edge, Path, Step, Value, Vertex};
use graphdb_query::query::executor::base::{EdgeDirection as ExecEdgeDirection, Executor};
use graphdb_query::query::executor::graph_operations::graph_traversal::algorithms::{
    AlgorithmContext, AlgorithmStats, MultiShortestPathExecutor, SubgraphConfig, SubgraphExecutor,
};
use graphdb_query::query::validator::context::ExpressionAnalysisContext;
use std::collections::HashMap;
use std::sync::Arc;

// ==================== Algorithm Context Testing ====================

#[test]
fn test_algorithm_context_creation() {
    // Creating a context for testing algorithms
    let context = AlgorithmContext::new()
        .with_max_depth(Some(10))
        .with_limit(100)
        .with_single_shortest(true)
        .with_cycle(true);

    assert_eq!(context.max_depth, Some(10));
    assert_eq!(context.limit, 100);
    assert!(context.single_shortest);
    assert!(context.with_cycle);
}

#[test]
fn test_algorithm_context_default() {
    let context = AlgorithmContext::new();

    assert_eq!(context.max_depth, None);
    assert_eq!(context.limit, usize::MAX);
    assert!(!context.single_shortest);
    assert!(!context.with_cycle);
}

#[test]
fn test_algorithm_stats() {
    let mut stats = AlgorithmStats::new();

    assert_eq!(stats.nodes_visited, 0);
    assert_eq!(stats.edges_traversed, 0);
    assert_eq!(stats.execution_time_ms, 0);

    stats.nodes_visited = 100;
    stats.edges_traversed = 200;
    stats.execution_time_ms = 50;

    assert_eq!(stats.nodes_visited, 100);
    assert_eq!(stats.edges_traversed, 200);
    assert_eq!(stats.execution_time_ms, 50);
}

// ==================== Multi-source Shortest Path Executor Test ====================

#[test]
fn test_multi_shortest_path_executor_creation() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let executor = MultiShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::MultiShortestPathConfig {
            start_vids: vec![VertexId::from_string("alice")],
            direction: ExecEdgeDirection::Out,
            edge_types: None,
            max_steps: 10,
            space_name: "test".to_string(),
        },
    );

    assert_eq!(executor.id(), 1);
    assert_eq!(executor.name(), "MultiShortestPathExecutor");
    assert!(executor.description().contains("shortest path"));
}

#[test]
fn test_multi_shortest_path_with_edge_filter() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let executor = MultiShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::MultiShortestPathConfig {
            start_vids: vec![VertexId::from_string("alice")],
            direction: ExecEdgeDirection::Out,
            edge_types: Some(vec!["KNOWS".to_string()]),
            max_steps: 10,
            space_name: "test".to_string(),
        },
    );

    assert_eq!(executor.id(), 1);
    // Verification that the executor was created successfully, with filtering by edge type.
}

#[test]
fn test_multi_shortest_path_bidirectional_direction() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let executor = MultiShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::MultiShortestPathConfig {
            start_vids: vec![VertexId::from_string("alice")],
            direction: ExecEdgeDirection::Both,
            edge_types: None,
            max_steps: 10,
            space_name: "test".to_string(),
        },
    );

    assert_eq!(executor.id(), 1);
    // The bidirectional edge direction setting was successfully verified.
}

// ==================== Subgraph Query Executor Test ====================

#[test]
fn test_subgraph_config_default() {
    let config = SubgraphConfig::default();

    assert_eq!(config.steps, 1);
    assert_eq!(config.edge_direction, ExecEdgeDirection::Out);
    assert!(config.edge_types.is_none());
    assert!(config.limit.is_none());
    assert!(config.with_properties);
}

#[test]
fn test_subgraph_config_builder() {
    let config = SubgraphConfig::new(3)
        .with_direction(ExecEdgeDirection::Both)
        .with_edge_types(vec!["KNOWS".to_string(), "FRIEND".to_string()])
        .with_limit(100);

    assert_eq!(config.steps, 3);
    assert_eq!(config.edge_direction, ExecEdgeDirection::Both);
    assert_eq!(
        config.edge_types,
        Some(vec!["KNOWS".to_string(), "FRIEND".to_string()])
    );
    assert_eq!(config.limit, Some(100));
}

#[test]
fn test_subgraph_executor_creation() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let config = SubgraphConfig::new(2);

    let executor = SubgraphExecutor::new(
        1,
        storage.clone(),
        vec![VertexId::from_string("alice")],
        config,
        Arc::new(ExpressionAnalysisContext::new()),
    );

    assert_eq!(executor.id(), 1);
    assert_eq!(executor.name(), "SubgraphExecutor");
    assert!(executor.description().contains("subgraph"));
}

#[test]
fn test_subgraph_executor_multiple_start_vids() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let config = SubgraphConfig::new(2);

    let executor = SubgraphExecutor::new(
        1,
        storage.clone(),
        vec![
            VertexId::from_string("alice"),
            VertexId::from_string("bob"),
            VertexId::from_string("charlie"),
        ],
        config,
        Arc::new(ExpressionAnalysisContext::new()),
    );

    assert_eq!(executor.id(), 1);
    // Verification that the creation of multiple starting points was successful.
}

// ==================== Testing of Path Data Structures ====================

#[test]
fn test_path_creation() {
    let vertex = Vertex::with_vid(VertexId::from_string("A"));
    let path = Path::new(vertex.clone());

    assert_eq!(path.src.vid, VertexId::from_string("A"));
    assert!(path.steps.is_empty());
}

#[test]
fn test_path_with_steps() {
    let src = Vertex::with_vid(VertexId::from_string("A"));
    let dst = Vertex::with_vid(VertexId::from_string("B"));

    let mut path = Path::new(src);
    path.steps
        .push(Step::new(dst, "KNOWS".to_string(), "KNOWS".to_string(), 0));

    assert_eq!(path.steps.len(), 1);
    assert_eq!(path.steps[0].edge.edge_type, "KNOWS");
}

#[test]
fn test_vertex_with_vid() {
    let vertex = Vertex::with_vid(VertexId::from_string("test_id"));

    assert_eq!(vertex.vid, VertexId::from_string("test_id"));
    assert!(vertex.tags.is_empty());
    assert!(vertex.properties.is_empty());
}

#[test]
fn test_vertex_with_tags() {
    let tag = Tag::new(
        "Person".to_string(),
        [("name".to_string(), Value::from("Alice"))]
            .iter()
            .cloned()
            .collect(),
    );

    let vertex = Vertex::new(VertexId::from_string("alice"), vec![tag]);

    assert_eq!(vertex.vid, VertexId::from_string("alice"));
    assert_eq!(vertex.tags.len(), 1);
    assert_eq!(vertex.tags[0].name, "Person");
}

// ==================== Testing of Edge Data Structures ====================

#[test]
fn test_edge_creation() {
    let edge = Edge::new(
        VertexId::from_string("A"),
        VertexId::from_string("B"),
        "KNOWS".to_string(),
        0,
        HashMap::new(),
    );

    assert_eq!(edge.src, VertexId::from_string("A"));
    assert_eq!(edge.dst, VertexId::from_string("B"));
    assert_eq!(edge.edge_type, "KNOWS");
    assert_eq!(edge.ranking, 0);
}

#[test]
fn test_edge_with_properties() {
    let mut props = HashMap::new();
    props.insert("since".to_string(), Value::from("2020-01-01"));

    let edge = Edge::new(
        VertexId::from_string("A"),
        VertexId::from_string("B"),
        "KNOWS".to_string(),
        1,
        props,
    );

    assert_eq!(edge.ranking, 1);
    assert!(edge.props.contains_key("since"));
}

// ==================== Boundary Condition Testing ====================

#[test]
fn test_multi_shortest_path_empty_start() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let executor = MultiShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::MultiShortestPathConfig {
            start_vids: vec![],
            direction: ExecEdgeDirection::Out,
            edge_types: None,
            max_steps: 10,
            space_name: "test".to_string(),
        },
    );

    assert_eq!(executor.id(), 1);
    // Verify that the actuator can still be created even when the starting point is empty.
}

#[test]
fn test_multi_shortest_path_empty_end() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let executor = MultiShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::MultiShortestPathConfig {
            start_vids: vec![VertexId::from_string("alice")],
            direction: ExecEdgeDirection::Out,
            edge_types: None,
            max_steps: 10,
            space_name: "test".to_string(),
        },
    );

    assert_eq!(executor.id(), 1);
    // Verify that the actuator can still be created even when the destination point is empty.
}

#[test]
fn test_subgraph_empty_start() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let config = SubgraphConfig::new(2);

    let executor = SubgraphExecutor::new(
        1,
        storage.clone(),
        vec![], // Empty starting point
        config,
        Arc::new(ExpressionAnalysisContext::new()),
    );

    assert_eq!(executor.id(), 1);
    // Verify that the actuator can still be created even when the starting point is empty.
}

#[test]
fn test_subgraph_zero_steps() {
    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    let config = SubgraphConfig::new(0); // 0 steps

    let executor = SubgraphExecutor::new(
        1,
        storage.clone(),
        vec![VertexId::from_string("alice")],
        config,
        Arc::new(ExpressionAnalysisContext::new()),
    );

    assert_eq!(executor.id(), 1);
    // Verify that the executor can still be created even with the configuration in step 0 set to its default values.
}

#[test]
fn test_algorithm_context_with_zero_limit() {
    let context = AlgorithmContext::new().with_limit(0);

    assert_eq!(context.limit, 0);
}

#[test]
fn test_algorithm_context_with_max_depth_zero() {
    let context = AlgorithmContext::new().with_max_depth(Some(0));

    assert_eq!(context.max_depth, Some(0));
}

// ==================== Testing of the “with_loop” option =====================

#[test]
fn test_algorithm_context_with_loop() {
    // By default, the value of `with_loop` is `false` during testing.
    let context_default = AlgorithmContext::new();
    assert!(!context_default.with_loop);

    // The test setting has with_loop set to true.
    let context_with_loop = AlgorithmContext::new().with_loop(true);
    assert!(context_with_loop.with_loop);

    // The test setting has with_loop set to false.
    let context_no_loop = AlgorithmContext::new().with_loop(false);
    assert!(!context_no_loop.with_loop);
}

#[test]
fn test_algorithm_context_with_loop_and_other_options() {
    // Testing the combination of `with_loop` with other options
    let context = AlgorithmContext::new()
        .with_max_depth(Some(10))
        .with_limit(100)
        .with_single_shortest(true)
        .with_cycle(true)
        .with_loop(true);

    assert_eq!(context.max_depth, Some(10));
    assert_eq!(context.limit, 100);
    assert!(context.single_shortest);
    assert!(context.with_cycle);
    assert!(context.with_loop);
}

#[test]
fn test_expand_executor_with_loop() {
    use graphdb_query::query::executor::graph_operations::graph_traversal::ExpandExecutor;

    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    // Create a default executor (with_loop = false)
    let executor_default = ExpandExecutor::new(
        1,
        storage.clone(),
        ExecEdgeDirection::Out,
        None,
        Some(3),
        Arc::new(ExpressionAnalysisContext::new()),
    );
    assert!(!executor_default.with_loop);

    // Create an executor that allows the execution of self-looping edges.
    let executor_with_loop = ExpandExecutor::new(
        2,
        storage.clone(),
        ExecEdgeDirection::Out,
        None,
        Some(3),
        Arc::new(ExpressionAnalysisContext::new()),
    )
    .with_loop(true);
    assert!(executor_with_loop.with_loop);
}

#[test]
fn test_all_paths_executor_with_loop() {
    use graphdb_query::query::executor::graph_operations::graph_traversal::AllPathsExecutor;

    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    // Create the default executor (with_loop = false)
    let executor_default = AllPathsExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            1,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::AllPathsConfig {
            left_start_ids: vec![VertexId::from_string("A")],
            right_start_ids: vec![VertexId::from_string("B")],
            max_hops: 5,
            edge_types: None,
            direction: ExecEdgeDirection::Both,
            space_name: "test".to_string(),
        },
    );
    assert!(!executor_default.with_loop);

    // Create an executor that allows the execution of self-looping edges.
    let executor_with_loop = AllPathsExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            2,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::AllPathsConfig {
            left_start_ids: vec![VertexId::from_string("A")],
            right_start_ids: vec![VertexId::from_string("B")],
            max_hops: 5,
            edge_types: None,
            direction: ExecEdgeDirection::Both,
            space_name: "test".to_string(),
        },
    )
    .with_loop(true);
    assert!(executor_with_loop.with_loop);
}

// ==================== Integrated Testing of the Shortest Path with Weighting =====================

#[test]
fn test_weighted_shortest_path_executor_creation() {
    use graphdb_query::query::executor::graph_operations::graph_traversal::algorithms::{
        EdgeWeightConfig, ShortestPathAlgorithmType,
    };
    use graphdb_query::query::executor::graph_operations::graph_traversal::ShortestPathExecutor;

    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    // Create an executor for finding the shortest path with weights
    let executor = ShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            100,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: ExecEdgeDirection::Out,
            edge_types: Some(vec!["connect".to_string()]),
            space_name: "test".to_string(),
        },
        ShortestPathAlgorithmType::Dijkstra,
    )
    .with_weight_config(EdgeWeightConfig::Property("weight".to_string()));

    assert_eq!(executor.id(), 100);
    assert_eq!(executor.name(), "ShortestPathExecutor");
}

#[test]
fn test_weighted_shortest_path_with_ranking() {
    use graphdb_query::query::executor::graph_operations::graph_traversal::algorithms::{
        EdgeWeightConfig, ShortestPathAlgorithmType,
    };
    use graphdb_query::query::executor::graph_operations::graph_traversal::ShortestPathExecutor;

    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    // Use “ranking” as the weight.
    let executor = ShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            101,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: ExecEdgeDirection::Out,
            edge_types: None,
            space_name: "test".to_string(),
        },
        ShortestPathAlgorithmType::Dijkstra,
    )
    .with_weight_config(EdgeWeightConfig::Ranking);

    assert_eq!(executor.id(), 101);
}

#[test]
fn test_weighted_shortest_path_astar() {
    use graphdb_query::query::executor::graph_operations::graph_traversal::algorithms::{
        EdgeWeightConfig, HeuristicFunction, ShortestPathAlgorithmType,
    };
    use graphdb_query::query::executor::graph_operations::graph_traversal::ShortestPathExecutor;

    let test_storage = TestStorage::new().expect("创建测试存储失败");
    let storage = test_storage.storage();

    // Using the A* algorithm, with an heuristic function
    let executor = ShortestPathExecutor::new(
        graphdb_query::query::executor::base::ExecutorConfig::new(
            102,
            storage.clone(),
            Arc::new(ExpressionAnalysisContext::new()),
        ),
        graphdb_query::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: ExecEdgeDirection::Out,
            edge_types: None,
            space_name: "test".to_string(),
        },
        ShortestPathAlgorithmType::AStar,
    )
    .with_weight_config(EdgeWeightConfig::Property("weight".to_string()))
    .with_heuristic_config(HeuristicFunction::Zero);

    assert_eq!(executor.id(), 102);
}

#[test]
fn test_weighted_path_query_parser_integration() {
    use graphdb_query::query::parser::Parser;

    // Testing the parsing of authorized path query statements
    let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect WEIGHT weight";
    let mut parser = Parser::new(query);
    let result = parser.parse();

    assert!(
        result.is_ok(),
        "带权path查询: should succeed: {:?}",
        result.err()
    );

    let stmt = result.expect(": should succeed");
    assert_eq!(stmt.ast.stmt.kind(), "FIND PATH");
}

#[test]
fn test_weighted_path_query_with_ranking_parser() {
    use graphdb_query::query::parser::Parser;

    // Test the parsing of query statements that use “ranking” as a weighting factor.
    let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect WEIGHT ranking";
    let mut parser = Parser::new(query);
    let result = parser.parse();

    assert!(
        result.is_ok(),
        "使用ranking权重的path查询: should succeed: {:?}",
        result.err()
    );
}

#[test]
fn test_unweighted_path_query_parser() {
    use graphdb_query::query::parser::Parser;

    // Testing the parsing of query statements for unauthorized path access.
    let query = "FIND SHORTEST PATH FROM 1 TO 2 OVER connect";
    let mut parser = Parser::new(query);
    let result = parser.parse();

    assert!(
        result.is_ok(),
        "无权path查询: should succeed: {:?}",
        result.err()
    );
}
