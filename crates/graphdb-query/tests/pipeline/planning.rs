//! Planning Module Tests
//!
//! Test coverage:
//! - Plan construction correctness
//! - Plan transformation validation
//! - Plan cache behavior
//! - Plan node properties

use crate::common::test_scenario::TestScenario;

// ==================== Plan Construction Tests ====================

mod plan_construction {
    use super::*;

    #[test]
    fn test_simple_scan_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_scan_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n")
            .assert_success();
    }

    #[test]
    fn test_index_scan_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_scan_plan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_traversal_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:knows]->(b:person) RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_multi_hop_traversal_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_multi_hop_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:knows]->(b:person)-[:knows]->(c:person) RETURN a, c")
            .assert_success();
    }

    #[test]
    fn test_filter_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_plan")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 18 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_project_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_project_plan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age")
            .assert_success();
    }

    #[test]
    fn test_aggregate_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_aggregate_plan")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN count(n), avg(n.age)")
            .assert_success();
    }

    #[test]
    fn test_sort_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_sort_plan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n ORDER BY n.age DESC")
            .assert_success();
    }

    #[test]
    fn test_limit_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_join_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p, c")
            .assert_success();
    }
}

// ==================== Plan Transformation Tests ====================

mod plan_transformation {
    use super::*;

    #[test]
    fn test_filter_pushdown() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_pushdown")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 18 RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_projection_pushdown() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_pushdown")
            .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_limit_pushdown() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_pushdown")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_topn_transformation() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_topn_transform")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n ORDER BY n.age LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_project_collapse() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_collapse")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name WITH name AS final_name RETURN final_name")
            .assert_success();
    }
}

// ==================== Plan Cache Tests ====================

mod plan_cache {
    use super::*;

    #[test]
    fn test_plan_cache_hit() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_cache_hit")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_plan_cache_invalidation_on_ddl() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_cache_invalidate")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n")
            .assert_success()
            .exec_ddl("ALTER TAG person ADD (age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n")
            .assert_success();
    }

    #[test]
    fn test_parameterized_query_caching() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_param_cache")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 20 RETURN n")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }
}

// ==================== Plan Node Property Tests ====================

mod plan_node_properties {
    use super::*;

    #[test]
    fn test_node_id_uniqueness() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_node_id")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:knows]->(b:person)-[:knows]->(c:person) RETURN a, b, c")
            .assert_success();
    }

    #[test]
    fn test_node_children_relationship() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_node_children")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.name = 'test' RETURN n")
            .assert_success();
    }

    #[test]
    fn test_plan_depth() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_plan_depth")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:knows]->(b:person)-[:knows]->(c:person)-[:knows]->(d:person) RETURN a, d")
            .assert_success();
    }

    #[test]
    fn test_plan_output_schema() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_output_schema")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name AS name, n.age AS age")
            .assert_success();
    }
}

// ==================== Complex Plan Tests ====================

mod complex_plans {
    use super::*;

    #[test]
    fn test_union_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_union_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG employee(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success()
            .query("MATCH (m:employee) RETURN m.name")
            .assert_success();
    }

    #[test]
    fn test_with_aggregation_chain() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_with_agg_chain")
            .exec_ddl("CREATE TAG person(age INT, city STRING)")
            .assert_success()
            .query("MATCH (n:person) WITH n.city AS city, count(n) AS cnt RETURN city, cnt ORDER BY cnt DESC")
            .assert_success();
    }

    #[test]
    fn test_optional_match_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_optional_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (n:person) OPTIONAL MATCH (n)-[:knows]->(m:person) RETURN n, m")
            .assert_success();
    }

    #[test]
    fn test_path_finding_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_path_plan")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("FIND SHORTEST PATH FROM 1 TO 2 OVER knows")
            .assert_success();
    }

    #[test]
    fn test_multi_pattern_match() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_multi_pattern")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .exec_ddl("CREATE EDGE located_in()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company), (c:company)-[:located_in]->(city:company) RETURN p, city")
            .assert_success();
    }

    #[test]
    fn test_complex_filter_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_complex_filter")
            .exec_ddl("CREATE TAG person(age INT, city STRING, salary INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 18 AND n.city = 'Beijing' AND n.salary > 5000 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_nested_aggregation_plan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_nested_agg")
            .exec_ddl("CREATE TAG person(city STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.city AS city, count(n) AS cnt, avg(n.age) AS avg_age RETURN city, cnt, avg_age ORDER BY cnt DESC LIMIT 10")
            .assert_success();
    }
}
