//! Integration tests for cost-based optimization strategies
//!
//! This file tests cost-based strategies that were previously only covered
//! at the unit test level, without end-to-end integration verification.
//!
//! Test coverage:
//! - TraversalStartSelector
//! - BidirectionalTraversalOptimizer
//! - ExpressionPrecomputationOptimizer
//! - SubqueryUnnestingOptimizer
//! - MemoryBudgetAllocator
//! - SortEliminationOptimizer (cost-based phase)
//! - AggregateStrategySelector (cost-based phase)

use crate::common::test_scenario::TestScenario;

// ==================== Traversal Start Selector Tests ====================

mod traversal_start {
    use super::*;

    #[test]
    fn test_traversal_start_single_hop() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_traversal_start")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 2 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN a.name, b.name")
            .assert_success();
    }

    #[test]
    fn test_traversal_start_multi_hop() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_trav_start_multi")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 2 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows*2]->(b:person) RETURN a.name, b.name")
            .assert_success();
    }
}

// ==================== Bidirectional Traversal Tests ====================

mod bidirectional_traversal {
    use super::*;

    #[test]
    fn test_bidirectional_shallow() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_bidir_shallow")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query("MATCH (a:person)-[:follows]-(b:person) RETURN a.name, b.name")
            .assert_success();
    }

    #[test]
    fn test_bidirectional_multi_hop() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_bidir_multi")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 2 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows*2]-(b:person) RETURN a.name, b.name")
            .assert_success();
    }
}

// ==================== Expression Precomputation Tests ====================

mod expression_precomputation {
    use super::*;

    #[test]
    fn test_constant_expression_precompute() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_expr_precompute")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(age) VALUES 1:(30), 2:(25)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.age + 10 AS age_plus10, n.age * 2 AS age_double")
            .assert_success();
    }

    #[test]
    fn test_repeated_expression_precompute() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_repeated_expr")
            .exec_ddl("CREATE TAG person(age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(age) VALUES 1:(30), 2:(25)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.age + 10 AS a, n.age + 10 AS b WHERE n.age + 10 > 20")
            .assert_success();
    }
}

// ==================== Subquery Unnesting Tests ====================

// Note: Pattern expressions in predicates (e.g., `size((a)-[:KNOWS]->()) > 0`)
// are not yet supported by the parser. See `docs/query/unsupported_syntax.md`
// for details. SubqueryUnnestingOptimizer is tested at the unit level with
// programmatically constructed plans (cost_based/subquery_unnesting.rs).

// ==================== Memory Budget Tests ====================

mod memory_budget {
    use super::*;

    #[test]
    fn test_memory_budget_small_result() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_mem_small")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("MATCH (n:person) RETURN n LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_memory_budget_aggregate() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_mem_agg")
            .exec_ddl("CREATE TAG person(city STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(city, age) VALUES 1:('Beijing', 30), 2:('Beijing', 25)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n), avg(n.age)")
            .assert_success();
    }
}

// ==================== Sort Elimination Optimizer (TopN) ====================

mod topn_optimization {
    use super::*;

    #[test]
    fn test_topn_small_limit() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_topn_small")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age ORDER BY n.age LIMIT 1")
            .assert_success();
    }

    #[test]
    fn test_topn_with_filter() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_topn_filter")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 20 RETURN n.name, n.age ORDER BY n.age LIMIT 2")
            .assert_success();
    }
}

// ==================== Aggregate Strategy Selector ====================

mod aggregate_strategy {
    use super::*;

    #[test]
    fn test_hash_aggregate_simple() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_hash_agg")
            .exec_ddl("CREATE TAG person(city STRING)")
            .assert_success()
            .exec_dml(
                "INSERT VERTEX person(city) VALUES 1:('Beijing'), 2:('Shanghai'), 3:('Beijing')",
            )
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n)")
            .assert_success();
    }

    #[test]
    fn test_streaming_aggregate_sorted() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_stream_agg")
            .exec_ddl("CREATE TAG person(city STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(city, age) VALUES 1:('Beijing', 30), 2:('Beijing', 25), 3:('Shanghai', 35)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.city, count(n) ORDER BY n.city")
            .assert_success();
    }
}
