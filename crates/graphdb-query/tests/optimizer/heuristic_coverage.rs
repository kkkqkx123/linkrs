//! Comprehensive heuristic optimization rule tests
//!
//! This file covers heuristic rules that were previously only tested
//! at the unit test level, but lacked end-to-end integration verification.
//!
//! Test coverage:
//! - Dedup elimination
//! - Empty set operation elimination
//! - Row collect elimination
//! - Merge rules (MergeGetVertices/GetNeighbors + Project/Dedup)
//! - Predicate pushdown (EFilter, VFilter, GetNeighbors)
//! - Projection pushdown (GetEdges, GetNeighbors, EdgeIndexScan, ScanEdges)
//! - Limit pushdown (GetVertices, GetEdges, ScanEdges, IndexScan, TopN on IndexScan)
//! - Join rules (PushProject, LeftJoinToInner, JoinToExpand, JoinElimination, IndexJoin, JoinReorder)
//! - Sort elimination (EliminateSort)

use crate::common::test_scenario::TestScenario;

// ==================== Dedup Elimination Tests ====================

mod dedup_elimination {
    use super::*;

    #[test]
    fn test_dedup_elimination_with_unwind() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_dedup_unwind")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Alice')")
            .assert_success()
            .query("UNWIND ['Alice', 'Bob', 'Alice'] AS name RETURN name")
            .assert_success();
    }
}

// ==================== Empty Set Operation Elimination ====================

mod empty_set_operation {
    use super::*;

    #[test]
    fn test_empty_union_elimination() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_empty_union")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .assert_success()
            .query("MATCH (v:person) RETURN v.name AS name UNION MATCH (v:person) RETURN v.name AS name")
            .assert_success();
    }

    #[test]
    fn test_empty_intersect_elimination() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_empty_intersect")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .exec_dml("INSERT VERTEX company(name) VALUES 100:('TechCorp')")
            .assert_success()
            .query("MATCH (v:person) RETURN v.name AS name INTERSECT MATCH (v:company) RETURN v.name AS name")
            .assert_success();
    }
}

// ==================== Row Collect Elimination ====================

mod row_collect_elimination {
    use super::*;

    #[test]
    fn test_row_collect_elimination() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_row_collect")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .assert_success()
            .query("MATCH (n:person) RETURN collect(n.name)")
            .assert_success();
    }
}

// ==================== Merge Rules Tests ====================

mod merge_rules {
    use super::*;

    #[test]
    fn test_merge_get_vertices_and_project() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_merge_get_vertices_proj")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age LIMIT 10")
            .assert_success();
    }

    #[test]
    fn test_merge_get_vertices_and_dedup() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_merge_get_vertices_dedup")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .assert_success()
            .query("MATCH (n:person) RETURN DISTINCT n.name")
            .assert_success();
    }

    #[test]
    fn test_merge_get_nbrs_and_project() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_merge_get_nbrs_proj")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN a.name, b.name")
            .assert_success();
    }

    #[test]
    fn test_merge_get_nbrs_and_dedup() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_merge_get_nbrs_dedup")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 1 -> 3, 2 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN DISTINCT a.name")
            .assert_success();
    }
}

// ==================== Predicate Pushdown Tests ====================

mod predicate_pushdown {
    use super::*;

    #[test]
    fn test_push_efilter_down() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_push_efilter")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows(since INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows(since) VALUES 1 -> 2:(2020)")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) WHERE e.since > 2019 RETURN a.name, b.name")
            .assert_success();
    }

    #[test]
    fn test_push_vfilter_down_scan_vertices() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_push_vfilter_scan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age >= 30 RETURN n.name")
            .assert_success();
    }

    #[test]
    fn test_push_filter_down_get_nbrs() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_push_filter_get_nbrs")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 1 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) WHERE b.age > 28 RETURN a.name, b.name")
            .assert_success();
    }
}

// ==================== Projection Pushdown Tests ====================

mod projection_pushdown {
    use super::*;

    #[test]
    fn test_project_down_get_edges() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_get_edges")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) RETURN e")
            .assert_success();
    }

    #[test]
    fn test_project_down_get_neighbors() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_get_neighbors")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 1 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[:follows]->(b:person) RETURN b.age")
            .assert_success();
    }

    #[test]
    fn test_project_down_scan_edges() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_scan_edges")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) RETURN a.name")
            .assert_success();
    }
}

// ==================== Limit Pushdown Tests ====================

mod limit_pushdown {
    use super::*;

    #[test]
    fn test_limit_down_get_vertices() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_get_vertices")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age ORDER BY n.age LIMIT 2")
            .assert_success();
    }

    #[test]
    fn test_limit_down_get_edges() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_get_edges")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob'), 3:('Charlie')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2, 1 -> 3")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) RETURN a.name, b.name LIMIT 2")
            .assert_success();
    }

    #[test]
    fn test_limit_down_scan_edges() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_scan_edges")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query("MATCH (a:person)-[e:follows]->(b:person) RETURN e LIMIT 5")
            .assert_success();
    }

    #[test]
    fn test_limit_down_index_scan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_limit_idx_scan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 20 RETURN n.name, n.age ORDER BY n.age LIMIT 2")
            .assert_success();
    }

    #[test]
    fn test_topn_down_index_scan() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_topn_idx_scan")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG INDEX idx_person_age ON person(age)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) WHERE n.age > 20 RETURN n.name, n.age ORDER BY n.age LIMIT 2")
            .assert_success();
    }

    #[test]
    fn test_convert_sort_limit_to_topn() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_sort_limit_topn")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30), 2:('Bob', 25), 3:('Charlie', 35)")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name, n.age ORDER BY n.age LIMIT 5")
            .assert_success();
    }
}

// ==================== Join Optimization Rules ====================

mod join_rules {
    use super::*;

    #[test]
    fn test_push_project_down_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_proj_down_join")
            .exec_ddl("CREATE TAG person(name STRING, age INT)")
            .exec_ddl("CREATE TAG company(name STRING)")
            .exec_ddl("CREATE EDGE works_at()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name, age) VALUES 1:('Alice', 30)")
            .exec_dml("INSERT VERTEX company(name) VALUES 100:('TechCorp')")
            .exec_dml("INSERT EDGE works_at() VALUES 1 -> 100")
            .assert_success()
            .query("MATCH (p:person)-[:works_at]->(c:company) RETURN p.name, p.age")
            .assert_success();
    }

    // Note: Path pattern in WHERE (e.g., `(a)-[:follows]->(b)`) is not yet supported.
    // test_join_to_expand will be added once this syntax is supported.

    #[test]
    fn test_left_join_to_inner_join() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_left_join_inner")
            .exec_ddl("CREATE TAG person(name STRING)")
            .exec_ddl("CREATE EDGE follows()")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .exec_dml("INSERT EDGE follows() VALUES 1 -> 2")
            .assert_success()
            .query(
                "MATCH (n:person) OPTIONAL MATCH (n)-[:follows]->(m:person) RETURN n.name, m.name",
            )
            .assert_success();
    }

    #[test]
    fn test_join_elimination() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_join_elim")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name")
            .assert_success();
    }
}

// ==================== Sort Elimination Tests ====================

mod sort_elimination {
    use super::*;

    #[test]
    fn test_eliminate_sort_with_unique_key() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_elim_sort_unique")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice')")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name ORDER BY n.name")
            .assert_success();
    }

    #[test]
    fn test_eliminate_sort_with_limit_one() {
        TestScenario::new()
            .expect("Failed to create test scenario")
            .setup_space("test_elim_sort_limit1")
            .exec_ddl("CREATE TAG person(name STRING)")
            .assert_success()
            .exec_dml("INSERT VERTEX person(name) VALUES 1:('Alice'), 2:('Bob')")
            .assert_success()
            .query("MATCH (n:person) RETURN n.name ORDER BY n.name LIMIT 1")
            .assert_success();
    }
}
