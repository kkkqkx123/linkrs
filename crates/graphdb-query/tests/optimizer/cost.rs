//! Cost Model and Statistics Tests
//!
//! Test coverage:
//! - Cost estimation accuracy
//! - Statistics collection
//! - Cardinality estimation
//! - Cost model configuration

use crate::common::test_scenario::TestScenario;

// ==================== Cost Estimation Tests ====================

mod cost_estimation {
    use super::*;

    #[test]
    fn test_scan_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_scan_cost")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n")
            .assert_success();
    }

    #[test]
    fn test_index_scan_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_scan_cost")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_traversal_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_cost")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_join_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_cost")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p, c")
            .assert_success();
    }

    #[test]
    fn test_aggregate_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_cost")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN count(n), avg(n.age)")
            .assert_success();
    }

    #[test]
    fn test_sort_cost() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_sort_cost")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n ORDER BY n.age")
            .assert_success();
    }
}

// ==================== Statistics Collection Tests ====================

mod statistics_collection {
    use super::*;

    #[test]
    fn test_tag_statistics() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_tag_stats")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN count(n)")
            .assert_success();
    }

    #[test]
    fn test_edge_statistics() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_edge_stats")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) RETURN count(e)")
            .assert_success();
    }

    #[test]
    fn test_property_statistics() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_prop_stats")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.age, count(n) GROUP BY n.age")
            .assert_success();
    }

    #[test]
    fn test_index_statistics() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_stats")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }
}

// ==================== Cardinality Estimation Tests ====================

mod cardinality_estimation {
    use super::*;

    #[test]
    fn test_scan_cardinality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_scan_card")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN count(n)")
            .assert_success();
    }

    #[test]
    fn test_filter_cardinality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_filter_card")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 18 RETURN count(n)")
            .assert_success();
    }

    #[test]
    fn test_join_cardinality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_card")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN count(p)")
            .assert_success();
    }

    #[test]
    fn test_aggregate_cardinality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_card")
            .exec_ddl("CREATE TAG person(city STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n)")
            .assert_success();
    }
}

// ==================== Cost Model Configuration Tests ====================

mod cost_model_config {
    use super::*;

    #[test]
    fn test_default_cost_model() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_default_cost")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n")
            .assert_success();
    }

    #[test]
    fn test_cost_weights() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_cost_weights")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_memory_cost_factor() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_mem_cost")
            .exec_ddl("CREATE TAG person(name STRING, age INT, city STRING, salary INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age ORDER BY n.age LIMIT 100")
            .assert_success();
    }

    #[test]
    fn test_io_cost_factor() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_io_cost")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .query("MATCH (n:person) RETURN n LIMIT 1000")
            .assert_success();
    }
}

// ==================== Plan Comparison Tests ====================

mod plan_comparison {
    use super::*;

    #[test]
    fn test_index_vs_full_scan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_vs_scan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_nested_loop_vs_hash_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_method")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p, c")
            .assert_success();
    }

    #[test]
    fn test_sort_merge_vs_hash_aggregate() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_method")
            .exec_ddl("CREATE TAG person(city STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n), avg(n.age)")
            .assert_success();
    }
}

// ==================== Selectivity Estimation Tests ====================

mod selectivity_estimation {
    use super::*;

    #[test]
    fn test_selectivity_simple_equality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_selectivity_eq")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age = 30 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_selectivity_range_predicate() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_selectivity_range")
            .exec_ddl("CREATE TAG person(age INT, salary INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 25 AND n.age < 65 RETURN n")
            .assert_success();
    }

    #[test]
    fn test_selectivity_inequality() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_selectivity_neq")
            .exec_ddl("CREATE TAG person(status STRING)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.status <> 'inactive' RETURN n")
            .assert_success();
    }

    #[test]
    fn test_selectivity_compound_predicates() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_selectivity_compound")
            .exec_ddl("CREATE TAG person(age INT, city STRING, salary INT)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 25 AND n.city = 'NYC' AND n.salary > 50000 RETURN n")
            .assert_success();
    }
}

// ==================== Index Selection Tests ====================

mod index_selection_scenarios {
    use super::*;

    #[test]
    fn test_index_selection_single_column_index() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_single")
            .exec_ddl("CREATE TAG employee(emp_id INT, name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_emp_id ON employee(emp_id)")
            .assert_success()
            .query("MATCH (e:employee) WHERE e.emp_id = 123 RETURN e")
            .assert_success();
    }

    #[test]
    fn test_index_selection_composite_index() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_composite")
            .exec_ddl("CREATE TAG product(category STRING, price INT, availability INT)")
            .exec_ddl("CREATE TAG INDEX idx_category_price ON product(category, price)")
            .assert_success()
            .query("MATCH (p:product) WHERE p.category = 'Electronics' AND p.price < 1000 RETURN p")
            .assert_success();
    }

    #[test]
    fn test_no_index_full_scan_fallback() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_no_index")
            .exec_ddl("CREATE TAG customer(name STRING, city STRING)")
            .assert_success()
            .query("MATCH (c:customer) WHERE c.city = 'Boston' RETURN c")
            .assert_success();
    }

    #[test]
    fn test_index_selection_with_multiple_conditions() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_index_multi_cond")
            .exec_ddl("CREATE TAG account(user_id INT, status STRING, balance DOUBLE)")
            .exec_ddl("CREATE TAG INDEX idx_user_id ON account(user_id)")
            .assert_success()
            .query("MATCH (a:account) WHERE a.user_id = 456 AND a.status = 'active' RETURN a")
            .assert_success();
    }
}

// ==================== Traversal Direction & Start Tests ====================

mod traversal_optimization {
    use super::*;

    #[test]
    fn test_traversal_simple_path() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_path")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_traversal_two_hop() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_two_hop")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .exec_ddl("CREATE EDGE knows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person)-[:knows]->(c:person) RETURN a, c")
            .assert_success();
    }

    #[test]
    fn test_traversal_with_filter_on_start() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_start_filter")
            .exec_ddl("CREATE TAG person(age INT)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) WHERE a.age > 30 RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_traversal_with_filter_on_end() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_end_filter")
            .exec_ddl("CREATE TAG person(age INT)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) WHERE b.age < 50 RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_traversal_bidirectional() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_bidi")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE connected()")
            .assert_success()
            .query("MATCH (a:person)-[:connected]-(b:person) RETURN a, b")
            .assert_success();
    }

    #[test]
    fn test_traversal_with_index_on_start() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_index_start")
            .exec_ddl("CREATE TAG user(user_id INT)")
            .exec_ddl("CREATE TAG INDEX idx_user_id ON user(user_id)")
            .exec_ddl("CREATE EDGE friend_of()")
            .assert_success()
            .query("MATCH (u:user)-[:friend_of]->(v:user) WHERE u.user_id = 100 RETURN u, v")
            .assert_success();
    }
}

// ==================== Complex Optimization Scenarios ====================

mod complex_scenarios {
    use super::*;

    #[test]
    fn test_complex_query_with_multiple_filters() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_complex_multi_filter")
            .exec_ddl("CREATE TAG person(age INT, salary INT, city STRING)")
            .exec_ddl("CREATE TAG company(size INT)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) WHERE p.age > 25 AND c.size > 100 AND p.salary > 50000 RETURN p, c")
            .assert_success();
    }

    #[test]
    fn test_query_with_aggregation_and_filter() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_agg_filter")
            .exec_ddl("CREATE TAG employee(department STRING, salary INT)")
            .assert_success()
            .query("MATCH (e:employee) RETURN e.department, count(e), avg(e.salary) GROUP BY e.department HAVING count(e) > 5")
            .assert_success();
    }

    #[test]
    fn test_query_with_join_and_aggregation() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_agg")
            .exec_ddl("CREATE TAG person(name STRING, gender STRING)")
            .exec_ddl("CREATE TAG department(dept_name STRING)")
            .exec_ddl("CREATE EDGE works_in(salary INT)")
            .assert_success()
            .query("MATCH (p:person)-[w:works_in]->(d:department) RETURN d.dept_name, count(p), avg(w.salary) GROUP BY d.dept_name")
            .assert_success();
    }

    #[test]
    fn test_query_with_order_by_and_limit() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_order_limit")
            .exec_ddl("CREATE TAG employee(name STRING, salary INT)")
            .assert_success()
            .query("MATCH (e:employee) RETURN e.name, e.salary ORDER BY e.salary DESC LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_query_with_multiple_traversals() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_multi_traversal")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .exec_ddl("CREATE EDGE likes()")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person)-[:likes]->(c:person) RETURN a, b, c")
            .assert_success();
    }
}
