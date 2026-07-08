//! Heuristic Optimization Rule Tests
//!
//! Test coverage:
//! - Predicate pushdown rules
//! - Projection pushdown rules
//! - Merge rules
//! - Elimination rules
//! - Limit pushdown rules
//! - Rule interaction and ordering

use crate::common::test_scenario::TestScenario;

// ==================== Predicate Pushdown Tests ====================

mod predicate_pushdown {
    use super::*;

    #[test]
    fn test_filter_push_through_project() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_project")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name WHERE name = 'Alice' RETURN name")
            .assert_success();
    }

    #[test]
    fn test_filter_push_through_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_join")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) WHERE p.age > 30 RETURN p, c")
            .assert_success();
    }

    #[test]
    fn test_filter_push_through_aggregation() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_agg")
            .exec_ddl("CREATE TAG person(city STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.city AS city, count(n) AS cnt WHERE cnt > 5 RETURN city, cnt")
            .assert_success();
    }

    #[test]
    fn test_filter_on_different_sources() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_sources")
            .exec_ddl("CREATE TAG person(age INT)")
            .exec_ddl("CREATE TAG company(size INT)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) WHERE p.age > 25 AND c.size > 100 RETURN p, c")
            .assert_success();
    }
}

// ==================== Projection Pushdown Tests ====================

mod projection_pushdown {
    use super::*;

    #[test]
    fn test_project_column_pruning() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_col_pruning")
            .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING, salary INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_project_through_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_join")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p.name")
            .assert_success();
    }

    #[test]
    fn test_project_with_aggregate() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_agg")
            .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n)")
            .assert_success();
    }
}

// ==================== Merge Rules Tests ====================

mod merge_rules {
    use super::*;

    #[test]
    fn test_consecutive_projects_collapse() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_collapse")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name WITH name AS final_name RETURN final_name")
            .assert_success();
    }

    #[test]
    fn test_filter_project_reorder() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_proj_reorder")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name, n.age AS age WHERE age > 18 RETURN name")
            .assert_success();
    }
}

// ==================== Limit Pushdown Tests ====================

mod limit_pushdown {
    use super::*;

    #[test]
    fn test_limit_push_through_project() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_project")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name RETURN name LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_limit_through_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_join")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p LIMIT 10")
            .assert_success();
    }
}

// ==================== Rule Interaction Tests ====================

mod rule_interaction {
    use super::*;

    #[test]
    fn test_rule_application_order() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_rule_order")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WITH n.name AS name, n.age AS age WHERE age > 18 RETURN name ORDER BY name LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_complex_query_optimization() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_complex_opt")
            .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING)")
            .exec_ddl("CREATE TAG company(name STRING, size INT)")
            .exec_ddl("CREATE EDGE works_at(since INT)")
            .assert_success()
            .query("MATCH (p:person)-[e:works_at]->(c:company) WHERE p.age > 25 AND c.size > 100 WITH p, c, e.since AS since WHERE since > 2020 RETURN p.name, c.name ORDER BY since LIMIT 20")
            .assert_success();
    }
}
