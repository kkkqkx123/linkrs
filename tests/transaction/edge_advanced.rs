//! Advanced Edge Operation Tests
//!
//! Test coverage for advanced edge operations:
//! - Edge update operations
//! - Edge property updates
//! - Multiple edges between same vertices
//! - Edge with complex properties
//! - Edge direction validation
//! - Edge cascade operations
//! - Edge query patterns
//! - Edge batch operations
//! - Edge existence checks
//! - Edge type constraints

use super::common;

use common::test_scenario::TestScenario;

/// Test multiple edges of different types between same vertices
#[test]
fn test_multiple_edge_types_same_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_WITH(project STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS FRIENDS_WITH")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Create multiple edge types between same vertices
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2019)")
        .assert_success()
        .exec_dml("INSERT EDGE WORKS_WITH(project) VALUES 1->2:('ProjectX')")
        .assert_success()
        .exec_dml("INSERT EDGE FRIENDS_WITH VALUES 1->2")
        .assert_success()
        // Verify all edges exist
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(1, 2, "WORKS_WITH")
        .assert_edge_exists(1, 2, "FRIENDS_WITH");
}

/// Test multiple edges of same type between same vertices (if supported)
#[test]
fn test_multiple_same_type_edges() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS MESSAGED(content STRING, ts INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Create multiple messages between same people
        .exec_dml(
            "INSERT EDGE MESSAGED(content, ts) VALUES \
            1->2:('Hello', 1000), \
            1->2:('How are you?', 1001), \
            2->1:('Hi!', 1002)",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "MESSAGED")
        .assert_edge_exists(2, 1, "MESSAGED");
}

/// Test edge with complex property types
#[test]
fn test_edge_complex_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl(
            "CREATE EDGE IF NOT EXISTS RELATIONSHIP( \
            type STRING, \
            since INT, \
            active BOOL, \
            strength FLOAT)",
        )
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml(
            "INSERT EDGE RELATIONSHIP(type, since, active, strength) \
            VALUES 1->2:('friend', 2020, true, 0.95)",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "RELATIONSHIP");
}

/// Test self-referencing edge with properties
#[test]
fn test_self_referencing_edge_with_props() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS_SELF(confidence FLOAT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice')")
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS_SELF(confidence) VALUES 1->1:(1.0)")
        .assert_success()
        .assert_edge_exists(1, 1, "KNOWS_SELF");
}

/// Test edge direction in query patterns
#[test]
fn test_edge_direction_in_queries() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS FOLLOWS")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS BLOCKS")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie')",
        )
        .assert_success()
        // Alice follows Bob, Bob follows Charlie
        .exec_dml("INSERT EDGE FOLLOWS VALUES 1->2, 2->3")
        .assert_success()
        // Alice blocks Charlie (one-way)
        .exec_dml("INSERT EDGE BLOCKS VALUES 1->3")
        .assert_success()
        // Verify edges exist with correct direction
        .assert_edge_exists(1, 2, "FOLLOWS")
        .assert_edge_exists(2, 3, "FOLLOWS")
        .assert_edge_not_exists(2, 1, "FOLLOWS")
        .assert_edge_exists(1, 3, "BLOCKS")
        .assert_edge_not_exists(3, 1, "BLOCKS");
}

/// Test edge deletion cascade behavior
#[test]
fn test_edge_deletion_cascade() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_WITH")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie')",
        )
        .assert_success()
        // Create edges of different types
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2, 2->3")
        .assert_success()
        .exec_dml("INSERT EDGE WORKS_WITH VALUES 1->2, 1->3")
        .assert_success()
        // Delete only KNOWS edges
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        .exec_dml("DELETE EDGE KNOWS 2->3")
        .assert_success()
        // Verify KNOWS edges are gone but WORKS_WITH remain
        .assert_edge_not_exists(1, 2, "KNOWS")
        .assert_edge_not_exists(2, 3, "KNOWS")
        .assert_edge_exists(1, 2, "WORKS_WITH")
        .assert_edge_exists(1, 3, "WORKS_WITH");
}

/// Test batch edge insert
#[test]
fn test_edge_batch_insert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS CONNECTS(weight INT)")
        .assert_success()
        // Create a small network of people
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('A'), 2:('B'), 3:('C'), 4:('D'), 5:('E')",
        )
        .assert_success()
        // Batch insert edges to form a complete graph
        .exec_dml(
            "INSERT EDGE CONNECTS(weight) VALUES \
            1->2:(1), 1->3:(2), 1->4:(3), 1->5:(4), \
            2->3:(5), 2->4:(6), 2->5:(7), \
            3->4:(8), 3->5:(9), \
            4->5:(10)",
        )
        .assert_success()
        // Verify edge count
        .assert_edge_count("CONNECTS", 10);
}

/// Test edge with null/optional properties
#[test]
fn test_edge_optional_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS RELATES(description STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Edge with property
        .exec_dml("INSERT EDGE RELATES(description) VALUES 1->2:('friend')")
        .assert_success()
        // Edge without property (if schema allows)
        .exec_dml("INSERT EDGE RELATES VALUES 2->1")
        .assert_success()
        .assert_edge_exists(1, 2, "RELATES")
        .assert_edge_exists(2, 1, "RELATES");
}

/// Test edge existence after vertex operations
#[test]
fn test_edge_existence_after_vertex_ops() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie')",
        )
        .assert_success()
        .exec_dml("INSERT EDGE KNOWS VALUES 1->2, 2->3, 3->1")
        .assert_success()
        // Verify edges exist
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(2, 3, "KNOWS")
        .assert_edge_exists(3, 1, "KNOWS")
        // Update a vertex (should not affect edges)
        .exec_dml("UPDATE 1 SET name = 'AliceUpdated'")
        .assert_success()
        // Edges should still exist
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(3, 1, "KNOWS");
}

/// Test edge query by source and destination
#[test]
fn test_edge_query_by_vertices() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS RATED(score INT)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('User1'), \
            2:('Item1'), \
            3:('Item2'), \
            4:('Item3')",
        )
        .assert_success()
        // Create rating edges with different scores
        .exec_dml(
            "INSERT EDGE RATED(score) VALUES \
            1->2:(5), \
            1->3:(3), \
            1->4:(4)",
        )
        .assert_success()
        // Verify edges exist
        .assert_edge_exists(1, 2, "RATED")
        .assert_edge_exists(1, 3, "RATED")
        .assert_edge_exists(1, 4, "RATED");
}

/// Test edge type alteration and compatibility
#[test]
fn test_edge_type_alteration() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        // Create edge without properties
        .exec_ddl("CREATE EDGE IF NOT EXISTS CONNECTS")
        .assert_success()
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        .exec_dml("INSERT EDGE CONNECTS VALUES 1->2")
        .assert_success()
        // Alter edge to add property
        .exec_ddl("ALTER EDGE CONNECTS ADD (weight INT)")
        .assert_success()
        // Add new edge with property
        .exec_dml("INSERT VERTEX Person(name) VALUES 3:('Charlie')")
        .assert_success()
        .exec_dml("INSERT EDGE CONNECTS(weight) VALUES 2->3:(10)")
        .assert_success()
        // Both edges should exist
        .assert_edge_exists(1, 2, "CONNECTS")
        .assert_edge_exists(2, 3, "CONNECTS");
}

/// Test bidirectional edge pattern
#[test]
fn test_bidirectional_edge_pattern() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS FRIENDS_WITH(since INT)")
        .assert_success()
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie')",
        )
        .assert_success()
        // Create bidirectional friendships
        .exec_dml(
            "INSERT EDGE FRIENDS_WITH(since) VALUES \
            1->2:(2020), \
            2->1:(2020), \
            2->3:(2021), \
            3->2:(2021)",
        )
        .assert_success()
        // Verify bidirectional edges
        .assert_edge_exists(1, 2, "FRIENDS_WITH")
        .assert_edge_exists(2, 1, "FRIENDS_WITH")
        .assert_edge_exists(2, 3, "FRIENDS_WITH")
        .assert_edge_exists(3, 2, "FRIENDS_WITH");
}

/// Test edge in graph traversal patterns
#[test]
fn test_edge_in_traversal_patterns() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS WORKS_AT")
        .assert_success()
        // Create people
        .exec_dml(
            "INSERT VERTEX Person(name) VALUES \
            1:('Alice'), \
            2:('Bob'), \
            3:('Charlie'), \
            4:('David')",
        )
        .assert_success()
        // Create company
        .exec_ddl("CREATE TAG IF NOT EXISTS Company(name STRING)")
        .assert_success()
        .exec_dml("INSERT VERTEX Company(name) VALUES 100:('TechCorp')")
        .assert_success()
        // Create relationships
        .exec_dml(
            "INSERT EDGE KNOWS VALUES \
            1->2, \
            2->3, \
            3->4",
        )
        .assert_success()
        .exec_dml(
            "INSERT EDGE WORKS_AT VALUES \
            1->100, \
            2->100, \
            3->100",
        )
        .assert_success()
        // Verify all edges exist
        .assert_edge_exists(1, 2, "KNOWS")
        .assert_edge_exists(2, 3, "KNOWS")
        .assert_edge_exists(3, 4, "KNOWS")
        .assert_edge_exists(1, 100, "WORKS_AT")
        .assert_edge_exists(2, 100, "WORKS_AT")
        .assert_edge_exists(3, 100, "WORKS_AT");
}

/// Test edge deletion and re-insertion
#[test]
fn test_edge_delete_and_reinsert() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS KNOWS(since INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Insert edge
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2020)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS")
        // Delete edge
        .exec_dml("DELETE EDGE KNOWS 1->2")
        .assert_success()
        .assert_edge_not_exists(1, 2, "KNOWS")
        // Re-insert edge with different properties
        .exec_dml("INSERT EDGE KNOWS(since) VALUES 1->2:(2021)")
        .assert_success()
        .assert_edge_exists(1, 2, "KNOWS");
}

/// Test edge with timestamp properties using INT
#[test]
fn test_edge_timestamp_properties() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG IF NOT EXISTS Person(name STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE IF NOT EXISTS MESSAGED(content STRING, sent_at INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX Person(name) VALUES 1:('Alice'), 2:('Bob')")
        .assert_success()
        // Insert edge with timestamp as INT
        .exec_dml(
            "INSERT EDGE MESSAGED(content, sent_at) \
             VALUES 1->2:('Hello', 1704067200)",
        )
        .assert_success()
        .assert_edge_exists(1, 2, "MESSAGED");
}
