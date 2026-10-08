use super::*;

use super::super::config::ESTIMATED_ROW_WIDTH_BYTES;

#[test]
fn test_calculate_scan_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    // When no statistical information is available, a value of 0 should be returned.
    let cost = calculator.calculate_scan_vertices_cost("test", "NonExistent");
    assert_eq!(cost, 0.0);
}

#[test]
fn scan_cost_includes_io_and_point_lookup_is_cheaper() {
    use crate::optimizer::stats::TagStatistics;
    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 1000;
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);

    let scan = calculator.calculate_scan_vertices_cost("test", "person");
    // CPU alone would be 10.0; cache-aware I/O must add a positive term.
    assert!(scan > 10.0, "scan={scan} should include I/O");
    let point = calculator.calculate_get_vertices_cost(1);
    assert!(point < scan, "point={point} scan={scan}");
    assert_eq!(calculator.calculate_get_vertices_cost(0), 0.0);
}

#[test]
fn scan_cost_grows_with_allocated_slots() {
    use crate::optimizer::stats::TagStatistics;
    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 1000;
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);
    let no_holes = calculator.calculate_scan_vertices_cost("test", "person");

    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 1000;
    tag_stats.allocated_slots = Some(5000);
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);
    let with_holes = calculator.calculate_scan_vertices_cost("test", "person");
    assert!(
        with_holes > no_holes,
        "with_holes={with_holes} no_holes={no_holes}"
    );

    // Inconsistent snapshots (allocated below live) fall back to live.
    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 1000;
    tag_stats.allocated_slots = Some(10);
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);
    assert_eq!(
        calculator.calculate_scan_vertices_cost("test", "person"),
        no_holes
    );
}

#[test]
fn index_scan_heap_cost_grows_with_hole_rate() {
    use crate::optimizer::stats::TagStatistics;
    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 10_000;
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);
    let no_holes = calculator.calculate_index_scan_cost("test", "person", "name", 0.1);

    let stats_manager = Arc::new(StatisticsManager::new());
    let mut tag_stats = TagStatistics::new("person".to_string());
    tag_stats.vertex_count = 10_000;
    tag_stats.allocated_slots = Some(20_000);
    stats_manager.update_tag_stats("test", tag_stats);
    let calculator = CostCalculator::new(stats_manager);
    let with_holes = calculator.calculate_index_scan_cost("test", "person", "name", 0.1);
    // Hole rate 0.5 inflates only the heap revisit term: total must
    // grow but stay below a full doubling.
    assert!(
        with_holes > no_holes,
        "with_holes={with_holes} no_holes={no_holes}"
    );
    assert!(
        with_holes < 2.0 * no_holes,
        "with_holes={with_holes} no_holes={no_holes}"
    );
}

#[test]
fn tag_and_edge_selectivity_share_space_census() {
    use crate::optimizer::stats::{EdgeTypeStatistics, TagStatistics};
    let stats_manager = Arc::new(StatisticsManager::new());
    let mut a = TagStatistics::new("a".to_string());
    a.vertex_count = 750;
    stats_manager.update_tag_stats("s", a);
    let mut b = TagStatistics::new("b".to_string());
    b.vertex_count = 250;
    stats_manager.update_tag_stats("s", b);
    let calculator = CostCalculator::new(stats_manager);
    let sel = calculator.estimate_tag_selectivity("s", "a");
    assert!((sel - 0.75).abs() < 1e-9, "selectivity={sel}");
    assert_eq!(calculator.estimate_tag_selectivity("s", "missing"), 1.0);

    let stats_manager = Arc::new(StatisticsManager::new());
    let mut e = EdgeTypeStatistics::new("knows".to_string());
    e.edge_count = 900;
    stats_manager.update_edge_stats("s", e);
    let mut f = EdgeTypeStatistics::new("likes".to_string());
    f.edge_count = 100;
    stats_manager.update_edge_stats("s", f);
    let calculator = CostCalculator::new(stats_manager);
    let sel = calculator.estimate_edge_selectivity("s", "knows");
    assert!((sel - 0.9).abs() < 1e-9, "selectivity={sel}");
}

#[test]
fn test_calculate_filter_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let cost = calculator.calculate_filter_cost(1000, 3);
    assert!(cost > 0.0);
    // 1000 * 3 * 0.0025 = 7.5
    assert_eq!(cost, 7.5);
}

#[test]
fn test_calculate_hash_join_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let cost = calculator.calculate_hash_join_cost(100, 200);
    assert!(cost > 0.0);
    // (100 + 200) * 0.01 + 100 * 0.1 * 0.0025 = 3.0 + 0.025 = 3.025
    assert_eq!(cost, 3.025);
}

#[test]
fn test_calculate_sort_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    // Test standard sorting (with no limit)
    let cost = calculator.calculate_sort_cost(1000, 2, None);
    assert!(cost > 0.0);

    // An empty input should return 0.
    let zero_cost = calculator.calculate_sort_cost(0, 2, None);
    assert_eq!(zero_cost, 0.0);

    // Testing the Top-N optimization (data volume > limit * 10)
    let topn_cost = calculator.calculate_sort_cost(1000, 2, Some(50));
    // The Top-N algorithm should be cheaper than the full sorting algorithm.
    assert!(topn_cost < cost);

    // Testing the small limit (which does not trigger the Top-N result).
    let small_limit_cost = calculator.calculate_sort_cost(1000, 2, Some(200));
    // The "small limit" should be sorted using the standard sorting method.
    assert!(small_limit_cost >= cost * 0.99 && small_limit_cost <= cost * 1.01);
}

#[test]
fn test_calculate_topn_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let cost = calculator.calculate_topn_cost(10000, 10);
    assert!(cost > 0.0);
    // 10000 * log2(10) * 0.0025 ≈ 83.05
    assert!(cost > 80.0 && cost < 85.0);
}

#[test]
fn test_with_config() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let config = CostModelConfig::for_ssd();
    let calculator = CostCalculator::with_config(stats_manager, config);

    assert_eq!(calculator.config().random_page_cost, 1.1);
}

#[test]
fn test_memory_aware_cost() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    // Test normal memory usage (below threshold)
    let base_cost = 100.0;
    let normal_memory = 1024 * 1024; // 1MB
    let normal_cost = calculator.calculate_memory_aware_cost(base_cost, normal_memory);
    // Memory cost for 1MB is: 1,048,576 * 0.0001 = 104.8576
    // So total should be around 204.86
    assert!(normal_cost > base_cost); // Should add memory cost

    // Test high memory usage (above threshold)
    let high_memory = 200 * 1024 * 1024; // 200MB (above 100MB threshold)
    let high_cost = calculator.calculate_memory_aware_cost(base_cost, high_memory);
    assert!(high_cost > normal_cost); // Should have penalty
}

#[test]
fn test_estimate_aggregate_memory() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    // Test with no group by keys
    let mem_no_groups = calculator.estimate_aggregate_memory(1000, 0);
    assert!(mem_no_groups > 0);

    // Test with multiple group by keys
    let mem_with_groups = calculator.estimate_aggregate_memory(10000, 3);
    assert!(mem_with_groups > 0);
}

#[test]
fn test_estimate_sort_memory() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let memory = calculator.estimate_sort_memory(1000, 2);
    assert_eq!(memory, 1000 * ESTIMATED_ROW_WIDTH_BYTES);
}

#[test]
fn test_estimate_hash_join_memory() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let memory = calculator.estimate_hash_join_memory(500);
    assert_eq!(memory, 500 * ESTIMATED_ROW_WIDTH_BYTES);
}

#[test]
fn test_calculate_aggregate_cost_enhanced() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let (cost, memory) = calculator.calculate_aggregate_cost_enhanced(1000, 2, 1);
    assert!(cost > 0.0);
    assert!(memory > 0);
}

#[test]
fn test_calculate_sort_cost_enhanced() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let (cost, memory) = calculator.calculate_sort_cost_enhanced(1000, 2, None);
    assert!(cost > 0.0);
    assert_eq!(memory, 1000 * ESTIMATED_ROW_WIDTH_BYTES);
}

#[test]
fn test_calculate_hash_join_cost_enhanced() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    let (cost, memory) = calculator.calculate_hash_join_cost_enhanced(100, 200);
    assert!(cost > 0.0);
    assert_eq!(memory, 100 * ESTIMATED_ROW_WIDTH_BYTES);
}

#[test]
fn test_get_type_cost_factor() {
    let stats_manager = Arc::new(StatisticsManager::new());
    let calculator = CostCalculator::new(stats_manager);

    // Fixed-size types
    assert_eq!(
        calculator.get_type_cost_factor(&Value::Int(42)),
        calculator.config.fixed_type_cost_factor
    );
    assert_eq!(
        calculator.get_type_cost_factor(&Value::Bool(true)),
        calculator.config.fixed_type_cost_factor
    );

    // Variable-length types
    assert_eq!(
        calculator.get_type_cost_factor(&Value::string("test")),
        calculator.config.variable_type_cost_factor
    );

    // Complex types
    assert_eq!(
        calculator.get_type_cost_factor(&Value::List(Box::<linkrs_core::value::List>::default())),
        calculator.config.complex_type_cost_factor
    );

    // Graph types
    use linkrs_core::vertex_edge_path::Vertex;
    let vertex = Vertex::new(
        linkrs_core::types::VertexId::try_from_int64(1).expect("valid vertex id"),
        linkrs_core::vertex_edge_path::Tag::new(String::new(), std::collections::HashMap::new()),
    );
    assert_eq!(
        calculator.get_type_cost_factor(&Value::Vertex(Box::new(vertex))),
        calculator.config.graph_type_cost_factor
    );
}
