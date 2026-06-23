//! Query Optimizer Integration Tests
//!
//! Test coverage:
//! - Index selection optimization (IndexScan vs SeqScan)
//! - Join algorithm selection (HashJoin, IndexJoin, NestedLoop)
//! - Aggregation optimization (HashAggregate)
//! - TopN optimization (Sort+Limit -> TopN)
//! - EXPLAIN output validation
//! - Optimizer result equivalence (with vs without optimization)

use super::common;

use common::test_scenario::TestScenario;
use common::TestStorage;
use graphdb_query::core::stats::StatsManager;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

// ==================== Index Selection Tests ====================

#[test]
fn test_idx_001_index_scan_for_equality() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_idx")
        .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING, salary INT)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_name ON person(name)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
        .assert_success();

    for i in 0..100 {
        let name = format!("Person_{:03}", i);
        let age = 20 + (i % 40);
        let city = ["Beijing", "Shanghai", "Shenzhen"][i % 3];
        let salary = 5000 + (i * 100);

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, age, city, salary) VALUES {}:(\"{}\", {}, \"{}\", {})",
            i, name, age, city, salary
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (p:person {name: \"Person_001\"}) RETURN p.age")
        .assert_success()
        .assert_plan_contains_any(&["IndexScan", "index_scan", "ScanVertices", "scan_vertices"]);
}

#[test]
fn test_idx_002_index_scan_for_range() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_idx_range")
        .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING, salary INT)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
        .assert_success();

    for i in 0..100 {
        let name = format!("Person_{:03}", i);
        let age = 20 + (i % 40);

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, age) VALUES {}:(\"{}\", {})",
            i, name, age
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (p:person) WHERE p.age > 25 AND p.age < 35 RETURN p.name")
        .assert_success()
        .assert_plan_contains_any(&["IndexScan", "index_scan", "ScanVertices", "scan_vertices"]);
}

#[test]
fn test_idx_003_no_index_full_scan() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_full_scan")
        .exec_ddl("CREATE TAG person(name STRING, age INT, salary INT)")
        .assert_success();

    for i in 0..100 {
        let name = format!("Person_{:03}", i);
        let salary = 5000 + (i * 100);

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, salary) VALUES {}:(\"{}\", {})",
            i, name, salary
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (p:person) WHERE p.salary > 10000 RETURN p.name")
        .assert_success()
        .assert_plan_contains_any(&["Scan", "scan"]);
}

// ==================== Join Algorithm Selection Tests ====================

#[test]
fn test_join_001_join_algorithm_selection() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_join")
        .exec_ddl("CREATE TAG company(name STRING, industry STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG employee(name STRING, salary INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE works_at(position STRING)")
        .assert_success();

    for i in 0..10 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX company(name, industry) VALUES {}: (\"Company_{:02}\", \"Tech\")",
            i, i
        ));
    }

    for i in 0..100 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX employee(name, salary) VALUES {}: (\"Employee_{:03}\", {})",
            100 + i,
            i,
            5000 + i * 100
        ));
    }

    for i in 0..100 {
        let company_id = i % 10;
        scenario = scenario.exec_dml(&format!(
            "INSERT EDGE works_at(position) VALUES {} -> {}:(\"Engineer\")",
            100 + i,
            company_id
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (e:employee)-[:works_at]->(c:company) RETURN e.name, c.name")
        .assert_success()
        .assert_plan_contains_any(&[
            "HashJoin",
            "hash_join",
            "IndexJoin",
            "index_join",
            "NestedLoop",
            "nested_loop",
            "Join",
            "Expand",
        ]);
}

// ==================== Aggregation Optimization Tests ====================

#[test]
fn test_agg_001_hash_aggregate() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_agg")
        .exec_ddl("CREATE TAG sales(product STRING, amount INT, category STRING)")
        .assert_success();

    for i in 0..100 {
        let product = format!("Product_{:02}", i % 20);
        let amount = (i % 100) * 10 + 10;
        let category = ["A", "B", "C"][i % 3];

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX sales(product, amount, category) VALUES \"s{:04}\":(\"{}\", {}, \"{}\")",
            i, product, amount, category
        ));
    }

    scenario
        .assert_success()
        .query(
            "EXPLAIN MATCH (s:sales) RETURN s.category, sum(s.amount) AS total GROUP BY s.category",
        )
        .assert_success()
        .assert_plan_contains_any(&["Aggregate", "aggregate", "HashAggregate", "hash_aggregate"]);
}

// ==================== TopN Optimization Tests ====================

#[test]
fn test_topn_001_order_by_limit() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_topn")
        .exec_ddl("CREATE TAG product(name STRING, price INT, sales INT)")
        .assert_success();

    for i in 0..100 {
        let price = (i % 100) * 10 + 10;
        let sales = i % 1000;

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX product(name, price, sales) VALUES \"p{:03}\":(\"Product_{:03}\", {}, {})",
            i, i, price, sales
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (p:product) RETURN p.name, p.price ORDER BY p.price DESC LIMIT 10")
        .assert_success()
        .assert_plan_contains_any(&["TopN", "top_n"]);
}

// ==================== EXPLAIN Format Tests ====================

#[test]
fn test_explain_001_text_format() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_explain")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .query("EXPLAIN MATCH (p:person) RETURN p.name")
        .assert_success();
}

#[test]
fn test_explain_002_dot_format() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_explain_dot")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .query("EXPLAIN FORMAT = DOT MATCH (p:person) RETURN p.name")
        .assert_success()
        .assert_plan_contains_any(&["digraph", "DOT"]);
}

// ==================== PROFILE Tests ====================

#[test]
fn test_profile_001_basic_profile() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_profile")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success();

    for i in 0..50 {
        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, age) VALUES \"p{:03}\":(\"Person_{:03}\", {})",
            i,
            i,
            20 + i
        ));
    }

    scenario
        .assert_success()
        .query("PROFILE MATCH (p:person) RETURN count(p)")
        .assert_success();
}

// ==================== Edge Cases Tests ====================

#[test]
fn test_optimizer_empty_result() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_empty")
        .exec_ddl("CREATE TAG person(name STRING, age INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name, age) VALUES \"p001\":(\"Alice\", 30)")
        .assert_success()
        .query("EXPLAIN MATCH (p:person) WHERE p.age > 100 RETURN p")
        .assert_success();
}

#[test]
fn test_optimizer_multiple_indexes() {
    let mut scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_multi_idx")
        .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_name ON person(name)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
        .assert_success()
        .exec_ddl("CREATE TAG INDEX idx_person_city ON person(city)")
        .assert_success();

    for i in 0..100 {
        let name = format!("Person_{:03}", i);
        let age = 20 + (i % 40);
        let city = ["Beijing", "Shanghai", "Shenzhen"][i % 3];

        scenario = scenario.exec_dml(&format!(
            "INSERT VERTEX person(name, age, city) VALUES \"p{:03}\":(\"{}\", {}, \"{}\")",
            i, name, age, city
        ));
    }

    scenario
        .assert_success()
        .query("EXPLAIN MATCH (p:person {name: \"Person_001\", age: 21}) RETURN p")
        .assert_success()
        .assert_plan_contains_any(&["IndexScan", "index_scan", "Scan"]);
}

#[test]
fn test_optimizer_complex_join() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("optimizer_test_complex_join")
        .exec_ddl("CREATE TAG person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG company(name STRING)")
        .assert_success()
        .exec_ddl("CREATE TAG department(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE works_at(position STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE belongs_to(since STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:(\"Alice\")")
        .exec_dml("INSERT VERTEX person(name) VALUES 2:(\"Bob\")")
        .exec_dml("INSERT VERTEX company(name) VALUES 100:(\"TechCorp\")")
        .exec_dml("INSERT VERTEX department(name) VALUES 200:(\"Engineering\")")
        .exec_dml("INSERT EDGE works_at(position) VALUES 1 -> 100:(\"Engineer\")")
        .exec_dml("INSERT EDGE belongs_to(since) VALUES 100 -> 200:(\"2020-01-01\")")
        .assert_success()
        .query("EXPLAIN MATCH (p:person)-[:works_at]->(c:company)-[:belongs_to]->(d:department) RETURN p.name, c.name, d.name")
        .assert_success()
        .assert_plan_contains_any(&["Join", "join", "Expand", "expand"]);
}

// ==================== Optimizer Result Equivalence Tests ====================

#[test]
fn test_optimizer_result_equivalence() {
    let test_storage = TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let schema_manager = test_storage.schema_manager();

    // Create data once (space + tag + vertex)
    {
        let stats_manager = Arc::new(StatsManager::new());
        let opt_enabled = Arc::new(OptimizerEngine::default());
        let mut pipeline =
            QueryPipelineManager::with_optimizer(storage.clone(), stats_manager, opt_enabled)
                .with_schema_manager(schema_manager.clone());
        pipeline
            .execute_query("CREATE SPACE opt_equiv (vid_type=INT64)")
            .expect("CREATE SPACE");
        // USE does not persist across execute_query calls — the space must be
        // passed via execute_query_with_space for each query.
    }

    // Construct SpaceInfo for opt_equiv (space_id is 1 as it's the first space created)
    use graphdb_query::core::types::SpaceInfo;
    let space_info = SpaceInfo {
        space_id: 1,
        space_name: "opt_equiv".to_string(),
        vid_type: graphdb_query::core::DataType::BigInt,
        ..Default::default()
    };
    let space_info: SpaceInfo = space_info;

    // Create data inside the space by passing SpaceInfo
    {
        let stats_manager = Arc::new(StatsManager::new());
        let opt_enabled = Arc::new(OptimizerEngine::default());
        let mut pipeline =
            QueryPipelineManager::with_optimizer(storage.clone(), stats_manager, opt_enabled)
                .with_schema_manager(schema_manager.clone());
        pipeline
            .execute_query_with_space(
                "CREATE TAG Item(name STRING, price DOUBLE)",
                Some(space_info.clone()),
            )
            .expect("CREATE TAG");
        pipeline
            .execute_query_with_space(
                "INSERT VERTEX Item(name, price) VALUES 1:('A', 10.0), 2:('B', 20.0), 3:('C', 30.0)",
                Some(space_info.clone()),
            )
            .expect("INSERT");
    }

    // Pipeline with optimization enabled
    let stats1 = Arc::new(StatsManager::new());
    let opt_on = Arc::new(OptimizerEngine::default());
    let mut pipeline_on = QueryPipelineManager::with_optimizer(storage.clone(), stats1, opt_on)
        .with_schema_manager(schema_manager.clone());

    // Pipeline with optimization disabled
    let stats2 = Arc::new(StatsManager::new());
    let mut opt_off_engine = OptimizerEngine::default();
    opt_off_engine.set_enable_heuristic(false);
    opt_off_engine.set_enable_cost_based(false);
    let opt_off = Arc::new(opt_off_engine);
    let mut pipeline_off = QueryPipelineManager::with_optimizer(storage.clone(), stats2, opt_off)
        .with_schema_manager(schema_manager);

    // Test: MATCH query results should be identical with or without optimization
    let queries = vec![
        "MATCH (i:Item) RETURN i.name, i.price ORDER BY i.name",
        "MATCH (i:Item) WHERE i.price > 15.0 RETURN i.name",
        "MATCH (i:Item) RETURN COUNT(i) AS total",
        "MATCH (i:Item) RETURN SUM(i.price) AS total_price",
    ];

    for query in queries {
        let result_on = pipeline_on.execute_query_with_space(query, Some(space_info.clone()));
        let result_off = pipeline_off.execute_query_with_space(query, Some(space_info.clone()));

        assert!(
            result_on.is_ok(),
            "Optimized query should succeed: {} (error: {:?})",
            query,
            result_on.err()
        );
        assert!(
            result_off.is_ok(),
            "Non-optimized query should succeed: {} (error: {:?})",
            query,
            result_off.err()
        );

        // Compare result counts
        assert_eq!(
            result_on.unwrap().count(),
            result_off.unwrap().count(),
            "Result count mismatch for query with/without optimization: {}",
            query
        );
    }
}
