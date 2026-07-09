#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use crate::core::types::VertexId;
    use crate::core::{Edge, Value, Vertex};
    use crate::query::executor::base::EdgeDirection;
    use crate::query::executor::base::Executor;
    use crate::query::executor::graph_operations::graph_traversal::algorithms::{
        EdgeWeightConfig, HeuristicFunction, ShortestPathAlgorithmType,
    };
    use crate::query::executor::graph_operations::graph_traversal::expand::ExpandExecutorParams;
    use crate::query::executor::graph_operations::graph_traversal::expand_all::ExpandAllExecutorParams;
    use crate::query::executor::graph_operations::graph_traversal::factory::GraphTraversalExecutorFactory;
    use crate::query::executor::graph_operations::graph_traversal::traits::GraphTraversalExecutor;
    use crate::query::executor::graph_operations::graph_traversal::traverse::TraverseExecutorParams;
    use crate::query::validator::context::ExpressionAnalysisContext;
    use crate::storage::{MockStorage, StorageWriter};
    use parking_lot::RwLock;
    use std::sync::Arc;

    fn create_test_graph(_test_name: &str) -> Arc<RwLock<MockStorage>> {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create MockStorage"),
        ));
        let space = "default";

        // Create test diagrams: A -> B -> C, A -> D
        {
            let mut storage_lock = storage.write();

            // Create vertices
            let vertex_a = Vertex::new(VertexId::from_string("A"), vec![]);
            let vertex_b = Vertex::new(VertexId::from_string("B"), vec![]);
            let vertex_c = Vertex::new(VertexId::from_string("C"), vec![]);
            let vertex_d = Vertex::new(VertexId::from_string("D"), vec![]);

            let id_a = storage_lock
                .insert_vertex(space, vertex_a)
                .expect("Failed to insert test vertex A");
            let id_b = storage_lock
                .insert_vertex(space, vertex_b)
                .expect("Failed to insert test vertex B");
            let id_c = storage_lock
                .insert_vertex(space, vertex_c)
                .expect("Failed to insert test vertex C");
            let id_d = storage_lock
                .insert_vertex(space, vertex_d)
                .expect("Failed to insert test vertex D");

            // Create an edge.
            let edge_ab = Edge::new(
                id_a,
                id_b,
                "connect".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            let edge_bc = Edge::new(
                id_b,
                id_c,
                "connect".to_string(),
                0,
                std::collections::HashMap::new(),
            );
            let edge_ad = Edge::new(
                id_a,
                id_d,
                "connect".to_string(),
                0,
                std::collections::HashMap::new(),
            );

            storage_lock
                .insert_edge(space, edge_ab)
                .expect("Failed to insert test edge AB");
            storage_lock
                .insert_edge(space, edge_bc)
                .expect("Failed to insert test edge BC");
            storage_lock
                .insert_edge(space, edge_ad)
                .expect("Failed to insert test edge AD");
        }

        storage
    }

    #[test]
    fn test_expand_executor() {
        let storage = create_test_graph("expand");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor =
            GraphTraversalExecutorFactory::create_expand_executor(ExpandExecutorParams {
                id: 1,
                storage,
                edge_direction: EdgeDirection::Out,
                edge_types: Some(vec!["connect".to_string()]),
                max_depth: Some(1),
                expr_context,
            });

        // Testing the basic functions
        assert_eq!(executor.name(), "ExpandExecutor");
        assert_eq!(executor.id(), 1);
        assert!(matches!(executor.get_edge_direction(), EdgeDirection::Out));
        assert!(executor.get_edge_types().is_some());
        assert_eq!(executor.get_max_depth(), Some(1));
    }

    #[test]
    fn test_expand_all_executor() {
        let storage = create_test_graph("expand_all");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor =
            GraphTraversalExecutorFactory::create_expand_all_executor(ExpandAllExecutorParams {
                id: 2,
                storage,
                edge_direction: EdgeDirection::Both,
                edge_types: None,
                any_edge_type: false,
                max_depth: Some(2),
                expr_context,
                space_id: 1,
                space_name: "default".to_string(),
            });

        assert_eq!(executor.name(), "ExpandAllExecutor");
        assert_eq!(executor.id(), 2);
        assert!(matches!(executor.get_edge_direction(), EdgeDirection::Both));
        assert!(executor.get_edge_types().is_none());
        assert_eq!(executor.get_max_depth(), Some(2));
    }

    #[test]
    fn test_traverse_executor() {
        let storage = create_test_graph("traverse");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());
        let executor =
            GraphTraversalExecutorFactory::create_traverse_executor(TraverseExecutorParams {
                id: 3,
                storage,
                edge_direction: EdgeDirection::Out,
                edge_types: Some(vec!["connect".to_string()]),
                max_depth: Some(3),
                conditions: Some("true".to_string()),
                expr_context,
            });

        assert_eq!(executor.name(), "TraverseExecutor");
        assert_eq!(executor.id(), 3);
        assert!(matches!(executor.get_edge_direction(), EdgeDirection::Out));
        assert!(executor.get_edge_types().is_some());
        assert_eq!(executor.get_max_depth(), Some(3));
    }

    #[test]
    fn test_shortest_path_executor() {
        let storage = create_test_graph("shortest_path");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            4,
            storage,
            expr_context,
            config,
            ShortestPathAlgorithmType::BFS,
        );

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 4);
        assert!(matches!(executor.get_edge_direction(), EdgeDirection::Out));
        assert!(executor.get_edge_types().is_none());
    }

    /// Create a test image with weights applied
    /// Graph structure: A --(weight: 1)--> B --(weight: 2)--> C
    ///         \--(weight: 5)--> D --(weight: 1)--> C
    /// 最短路径(按权重): A->B->C (总权重: 3)
    /// 最短路径(按步数): A->B->C 或 A->D->C (都是2步)
    fn create_weighted_test_graph(_test_name: &str) -> Arc<RwLock<MockStorage>> {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let space = "default";

        {
            let mut storage_lock = storage.write();

            // Create vertices
            let vertex_a = Vertex::new(VertexId::from_string("A"), vec![]);
            let vertex_b = Vertex::new(VertexId::from_string("B"), vec![]);
            let vertex_c = Vertex::new(VertexId::from_string("C"), vec![]);
            let vertex_d = Vertex::new(VertexId::from_string("D"), vec![]);

            let id_a = storage_lock
                .insert_vertex(space, vertex_a)
                .expect("Failed to insert test vertex A");
            let id_b = storage_lock
                .insert_vertex(space, vertex_b)
                .expect("Failed to insert test vertex B");
            let id_c = storage_lock
                .insert_vertex(space, vertex_c)
                .expect("Failed to insert test vertex C");
            let id_d = storage_lock
                .insert_vertex(space, vertex_d)
                .expect("Failed to insert test vertex D");

            // Create edges with weights
            let mut props_ab = std::collections::HashMap::new();
            props_ab.insert("weight".to_string(), Value::Int(1));
            let edge_ab = Edge::new(
                id_a,
                id_b,
                "connect".to_string(),
                1, // ranking also set to 1 for testing
                props_ab,
            );

            let mut props_bc = std::collections::HashMap::new();
            props_bc.insert("weight".to_string(), Value::Int(2));
            let edge_bc = Edge::new(id_b, id_c, "connect".to_string(), 2, props_bc);

            let mut props_ad = std::collections::HashMap::new();
            props_ad.insert("weight".to_string(), Value::Int(5));
            let edge_ad = Edge::new(id_a, id_d, "connect".to_string(), 5, props_ad);

            let mut props_dc = std::collections::HashMap::new();
            props_dc.insert("weight".to_string(), Value::Int(1));
            let edge_dc = Edge::new(id_d, id_c, "connect".to_string(), 1, props_dc);

            storage_lock
                .insert_edge(space, edge_ab)
                .expect("Failed to insert test edge AB");
            storage_lock
                .insert_edge(space, edge_bc)
                .expect("Failed to insert test edge BC");
            storage_lock
                .insert_edge(space, edge_ad)
                .expect("Failed to insert test edge AD");
            storage_lock
                .insert_edge(space, edge_dc)
                .expect("Failed to insert test edge DC");
        }

        storage
    }

    #[test]
    fn test_weighted_shortest_path_with_property() {
        let storage = create_weighted_test_graph("weighted_shortest_path_prop");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Create an executor using attribute weights.
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            5,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::Dijkstra,
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()));

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 5);
    }

    #[test]
    fn test_weighted_shortest_path_with_ranking() {
        let storage = create_weighted_test_graph("weighted_shortest_path_ranking");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Create an executor using “ranking” as a weight.
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            6,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::Dijkstra,
        )
        .with_weight_config(EdgeWeightConfig::Ranking);

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 6);
    }

    #[test]
    fn test_unweighted_shortest_path() {
        let storage = create_weighted_test_graph("unweighted_shortest_path");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        // Create an executor using the configuration from an unauthorized graph.
        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            7,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::BFS,
        )
        .with_weight_config(EdgeWeightConfig::Unweighted);

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 7);
    }

    // Create a test image with coordinate attributes for testing the A* algorithm.
    fn create_spatial_test_graph(_test_name: &str) -> Arc<RwLock<MockStorage>> {
        let storage = Arc::new(RwLock::new(
            MockStorage::new().expect("Failed to create Mock store"),
        ));
        let space = "default";

        // 创建空间测试图：A(0,0) -> B(3,4) -> C(6,8), A -> D(1,1) -> C
        // Use the Euclidean distance as an heuristic.
        {
            let mut storage_lock = storage.write();

            // Create vertices with coordinate attributes
            let mut props_a = std::collections::HashMap::new();
            props_a.insert("lat".to_string(), Value::Float(0.0));
            props_a.insert("lon".to_string(), Value::Float(0.0));
            let vertex_a = Vertex::new_with_properties(VertexId::from_string("A"), vec![], props_a);

            let mut props_b = std::collections::HashMap::new();
            props_b.insert("lat".to_string(), Value::Float(3.0));
            props_b.insert("lon".to_string(), Value::Float(4.0));
            let vertex_b = Vertex::new_with_properties(VertexId::from_string("B"), vec![], props_b);

            let mut props_c = std::collections::HashMap::new();
            props_c.insert("lat".to_string(), Value::Float(6.0));
            props_c.insert("lon".to_string(), Value::Float(8.0));
            let vertex_c = Vertex::new_with_properties(VertexId::from_string("C"), vec![], props_c);

            let mut props_d = std::collections::HashMap::new();
            props_d.insert("lat".to_string(), Value::Float(1.0));
            props_d.insert("lon".to_string(), Value::Float(1.0));
            let vertex_d = Vertex::new_with_properties(VertexId::from_string("D"), vec![], props_d);

            let id_a = storage_lock
                .insert_vertex(space, vertex_a)
                .expect("Failed to insert test vertex A");
            let id_b = storage_lock
                .insert_vertex(space, vertex_b)
                .expect("Failed to insert test vertex B");
            let id_c = storage_lock
                .insert_vertex(space, vertex_c)
                .expect("Failed to insert test vertex C");
            let id_d = storage_lock
                .insert_vertex(space, vertex_d)
                .expect("Failed to insert test vertex D");

            // Create edges with weights
            let mut props_ab = std::collections::HashMap::new();
            props_ab.insert("weight".to_string(), Value::Int(5));
            let edge_ab = Edge::new(id_a, id_b, "connect".to_string(), 5, props_ab);

            let mut props_bc = std::collections::HashMap::new();
            props_bc.insert("weight".to_string(), Value::Int(5));
            let edge_bc = Edge::new(id_b, id_c, "connect".to_string(), 5, props_bc);

            let mut props_ad = std::collections::HashMap::new();
            props_ad.insert("weight".to_string(), Value::Int(2));
            let edge_ad = Edge::new(id_a, id_d, "connect".to_string(), 2, props_ad);

            let mut props_dc = std::collections::HashMap::new();
            props_dc.insert("weight".to_string(), Value::Int(8));
            let edge_dc = Edge::new(id_d, id_c, "connect".to_string(), 8, props_dc);

            storage_lock
                .insert_edge(space, edge_ab)
                .expect("Failed to insert test edge AB");
            storage_lock
                .insert_edge(space, edge_bc)
                .expect("Failed to insert test edge BC");
            storage_lock
                .insert_edge(space, edge_ad)
                .expect("Failed to insert test edge AD");
            storage_lock
                .insert_edge(space, edge_dc)
                .expect("Failed to insert test edge DC");
        }

        storage
    }

    #[test]
    fn test_astar_with_spatial_heuristic() {
        let storage = create_spatial_test_graph("astar_spatial");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Using the A* algorithm, with a space-heuristic approach
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            8,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::AStar,
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()))
        .with_heuristic_config(HeuristicFunction::PropertyDistance(
            "lat".to_string(),
            "lon".to_string(),
        ));

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 8);
    }

    #[test]
    fn test_astar_without_heuristic() {
        let storage = create_spatial_test_graph("astar_no_heuristic");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Use the A* algorithm, but without any heuristic methods (which reduces it to the Dijkstra algorithm).
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            9,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::AStar,
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()))
        .with_heuristic_config(HeuristicFunction::Zero);

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 9);
    }

    #[test]
    fn test_astar_with_scale_heuristic() {
        let storage = create_spatial_test_graph("astar_scale");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Using the A* algorithm with a fixed scaling factor as a heuristic approach
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            10,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::AStar,
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()))
        .with_heuristic_config(HeuristicFunction::ScaleFactor(0.5));

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");
        assert_eq!(executor.id(), 10);
    }

    #[test]
    fn test_weighted_path_query_integration() {
        // Test the complete process for querying weighted paths.
        let storage = create_weighted_test_graph("weighted_integration");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Testing the Dijkstra algorithm using attribute weights
        let mut dijkstra_executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            11,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::Dijkstra,
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()));

        dijkstra_executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        dijkstra_executor.max_depth = Some(10);

        assert_eq!(dijkstra_executor.name(), "ShortestPathExecutor");
        assert_eq!(dijkstra_executor.id(), 11);

        // Verify the type of the algorithm.
        assert!(matches!(
            dijkstra_executor.get_algorithm(),
            ShortestPathAlgorithmType::Dijkstra
        ));
    }

    #[test]
    fn test_algorithm_auto_selection_weighted() {
        let storage = create_weighted_test_graph("auto_select_weighted");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Create an executor with weights, and verify the automatic selection of the Dijkstra algorithm.
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            12,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::Dijkstra, // Explicitly specifying Dijkstra
        )
        .with_weight_config(EdgeWeightConfig::Property("weight".to_string()));

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");

        // For weighted graphs, the Dijkstra or A* algorithm should be used.
        let algorithm = executor.get_algorithm();
        assert!(
            matches!(
                algorithm,
                ShortestPathAlgorithmType::Dijkstra | ShortestPathAlgorithmType::AStar
            ),
            "For weighted graphs, the Dijkstra or A* algorithm should be used."
        );
    }

    #[test]
    fn test_algorithm_auto_selection_unweighted() {
        let storage = create_test_graph("auto_select_unweighted");
        let expr_context = Arc::new(ExpressionAnalysisContext::new());

        let config = crate::query::executor::base::ShortestPathConfig {
            start_vertex_ids: vec![VertexId::from_string("A")],
            direction: EdgeDirection::Out,
            edge_types: None,
            space_name: String::new(),
        };

        // Create an executor without any weights, and verify its functionality using the BFS (Breadth-First Search) algorithm.
        let mut executor = GraphTraversalExecutorFactory::create_shortest_path_executor(
            13,
            storage.clone(),
            expr_context,
            config,
            ShortestPathAlgorithmType::BFS,
        )
        .with_weight_config(EdgeWeightConfig::Unweighted);

        executor.set_end_vertex_ids(vec![VertexId::from_string("C")]);
        executor.max_depth = Some(10);

        assert_eq!(executor.name(), "ShortestPathExecutor");

        // The “No Permission Graph” should use the BFS (Breadth-First Search) algorithm.
        assert!(matches!(
            executor.get_algorithm(),
            ShortestPathAlgorithmType::BFS
        ));
    }
}
