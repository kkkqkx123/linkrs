//! E2E Test Suite for Social Network Scenario
//!
//! Tests basic graph operations using the top-level API:
//! - Schema management
//! - Data insertion (vertices and edges)
//! - MATCH queries
//! - GO traversals
//! - LOOKUP queries
//! - Transaction management

use crate::common::{assert_query_ok, create_test_db, setup_test_space};

/// Test basic connection and show spaces
#[test]
fn test_connect_and_show_spaces() {
    let mut db = create_test_db();
    let result = db.execute_query("SHOW SPACES");
    assert_query_ok(result, "SHOW SPACES should succeed");
}

/// Test creating and using a space
#[test]
fn test_create_and_use_space() {
    let mut db = create_test_db();

    let result = db.execute_query("CREATE SPACE e2e_social_network (vid_type=STRING)");
    assert_query_ok(result, "CREATE SPACE should succeed");

    let result = db.execute_query("USE e2e_social_network");
    assert_query_ok(result, "USE should succeed");
}

/// Test creating tags and edges
#[test]
fn test_create_tags_and_edges() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_social_network_tags",
        &[
            "CREATE TAG IF NOT EXISTS person(name: STRING NOT NULL, age: INT, email: STRING, city: STRING)",
            "CREATE TAG IF NOT EXISTS company(name: STRING NOT NULL, industry: STRING)",
        ],
        &[
            "CREATE EDGE IF NOT EXISTS friend(degree: FLOAT)",
            "CREATE EDGE IF NOT EXISTS works_at(position: STRING)",
        ],
    ).expect("Failed to setup test space");

    // Verify tags were created
    let result = db.execute_query("SHOW TAGS");
    assert_query_ok(result, "SHOW TAGS should succeed after creating tags");

    // Verify edges were created
    let result = db.execute_query("SHOW EDGES");
    assert_query_ok(result, "SHOW EDGES should succeed after creating edges");
}

/// Test showing tags
#[test]
fn test_show_tags() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_show_tags",
        &["CREATE TAG person(name: STRING, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("SHOW TAGS");
    assert_query_ok(result, "SHOW TAGS should succeed");
}

/// Test showing edges
#[test]
fn test_show_edges() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_show_edges",
        &["CREATE TAG person(name: STRING)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("SHOW EDGES");
    assert_query_ok(result, "SHOW EDGES should succeed");
}

/// Test inserting vertex data
#[test]
fn test_insert_vertex() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_insert_vertex",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT, email: STRING)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query(
        "INSERT VERTEX person(name, age, email) VALUES 'p1': ('Alice', 30, 'alice@example.com')",
    );
    assert_query_ok(result, "INSERT VERTEX should succeed");
}

/// Test inserting multiple vertices
#[test]
fn test_insert_multiple_vertices() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_insert_multiple",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query(
        "INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30), 'p2': ('Bob', 25)",
    );
    assert_query_ok(result, "INSERT VERTEX with multiple values should succeed");
}

/// Test inserting edge data
#[test]
fn test_insert_edge() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_insert_edge",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert vertices first
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT VERTEX should succeed");

    // Insert edge
    let result = db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2': (0.8)");
    assert_query_ok(result, "INSERT EDGE should succeed");
}

/// Test fetching vertex properties
#[test]
fn test_fetch_vertex() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_fetch_vertex",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT, email: STRING)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Insert vertex
    db.execute_query(
        "INSERT VERTEX person(name, age, email) VALUES 'p_fetch': ('Alice', 30, 'alice@test.com')",
    )
    .expect("INSERT VERTEX should succeed");

    // Fetch vertex
    let result = db.execute_query("FETCH PROP ON person 'p_fetch'");
    assert_query_ok(result, "FETCH PROP should succeed");
}

/// Test fetching edge properties
#[test]
fn test_fetch_edge() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_fetch_edge",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert vertices and edge
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2' @0: (0.8)")
        .expect("INSERT EDGE should succeed");

    // Fetch edge
    let result = db.execute_query("FETCH PROP ON friend 'p1' -> 'p2'");
    assert_query_ok(result, "FETCH PROP ON EDGE should succeed");
}

/// Test basic MATCH query
#[test]
fn test_match_basic() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_match_basic",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT, city: STRING)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age, city) VALUES 'p1': ('Alice', 30, 'Beijing')")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age, city) VALUES 'p2': ('Bob', 25, 'Shanghai')")
        .expect("INSERT VERTEX should succeed");

    // Match query
    let result = db.execute_query("MATCH (p:person) RETURN p.name, p.age");
    assert_query_ok(result, "MATCH should succeed");
}

/// Test MATCH with filter
#[test]
fn test_match_with_filter() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_match_filter",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query(
        "INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30), 'p2': ('Bob', 25)",
    )
    .expect("INSERT VERTEX should succeed");

    // Match with filter
    let result = db.execute_query("MATCH (p:person) WHERE p.age > 28 RETURN p.name");
    assert_query_ok(result, "MATCH with filter should succeed");
}

/// Test MATCH path query
#[test]
fn test_match_path() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_match_path",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2': (0.8)")
        .expect("INSERT EDGE should succeed");

    // Match path
    let result = db.execute_query("MATCH (p:person)-[:friend]->(f:person) RETURN p.name, f.name");
    assert_query_ok(result, "MATCH path should succeed");
}

/// Test GO traversal
#[test]
fn test_go_traversal() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_go_traversal",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2': (0.8)")
        .expect("INSERT EDGE should succeed");

    // GO traversal
    let result = db.execute_query("GO 1 STEP FROM 'p1' OVER friend YIELD friend.name");
    assert_query_ok(result, "GO traversal should succeed");
}

/// Test GO multi-step traversal
#[test]
fn test_go_multiple_steps() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_go_multi",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &["CREATE EDGE friend(degree: FLOAT)"],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p2': ('Bob', 25)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p3': ('Charlie', 35)")
        .expect("INSERT VERTEX should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p1' -> 'p2': (0.8)")
        .expect("INSERT EDGE should succeed");
    db.execute_query("INSERT EDGE friend(degree) VALUES 'p2' -> 'p3': (0.7)")
        .expect("INSERT EDGE should succeed");

    // GO multi-step
    let result = db.execute_query("GO 2 STEPS FROM 'p1' OVER friend YIELD friend.name");
    assert_query_ok(result, "GO multi-step should succeed");
}

/// Test LOOKUP index query
#[test]
fn test_lookup_index() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_lookup",
        &["CREATE TAG person(name: STRING NOT NULL, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Create index
    db.execute_query("CREATE TAG INDEX idx_person_name ON person(name)")
        .expect("CREATE INDEX should succeed");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");

    // LOOKUP
    let result = db.execute_query("LOOKUP ON person WHERE person.name == 'Alice' YIELD person.age");
    assert_query_ok(result, "LOOKUP should succeed");
}

/// Test EXPLAIN command
#[test]
fn test_explain_basic() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_explain",
        &["CREATE TAG person(name: STRING, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // EXPLAIN
    let result = db.execute_query("EXPLAIN MATCH (p:person) RETURN p.name");
    assert_query_ok(result, "EXPLAIN should succeed");
}

/// Test PROFILE command
#[test]
fn test_profile_query() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_profile",
        &["CREATE TAG person(name: STRING, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Insert data
    db.execute_query("INSERT VERTEX person(name, age) VALUES 'p1': ('Alice', 30)")
        .expect("INSERT VERTEX should succeed");

    // PROFILE
    let result = db.execute_query("PROFILE MATCH (p:person) RETURN count(p)");
    assert_query_ok(result, "PROFILE should succeed");
}

/// Test transaction commit
#[test]
fn test_transaction_commit() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_tx_commit",
        &["CREATE TAG person(name: STRING, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Begin transaction
    let result = db.execute_query("BEGIN");
    assert_query_ok(result, "BEGIN should succeed");

    // Insert data
    let result = db.execute_query("INSERT VERTEX person(name, age) VALUES 'tx1': ('TX_Test', 20)");
    assert_query_ok(result, "INSERT should succeed");

    // Commit
    let result = db.execute_query("COMMIT");
    assert_query_ok(result, "COMMIT should succeed");
}

/// Test transaction rollback
#[test]
fn test_transaction_rollback() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_tx_rollback",
        &["CREATE TAG person(name: STRING, age: INT)"],
        &[],
    )
    .expect("Failed to setup test space");

    // Begin transaction
    let result = db.execute_query("BEGIN");
    assert_query_ok(result, "BEGIN should succeed");

    // Insert data
    let result = db.execute_query("INSERT VERTEX person(name, age) VALUES 'tx2': ('Rollback', 25)");
    assert_query_ok(result, "INSERT should succeed");

    // Rollback
    let result = db.execute_query("ROLLBACK");
    assert_query_ok(result, "ROLLBACK should succeed");
}

/// Cleanup test spaces
#[test]
fn test_cleanup_spaces() {
    let mut db = create_test_db();

    let spaces = [
        "e2e_social_network",
        "e2e_social_network_tags",
        "e2e_show_tags",
        "e2e_show_edges",
        "e2e_insert_vertex",
        "e2e_insert_multiple",
        "e2e_insert_edge",
        "e2e_fetch_vertex",
        "e2e_fetch_edge",
        "e2e_match_basic",
        "e2e_match_filter",
        "e2e_match_path",
        "e2e_go_traversal",
        "e2e_go_multi",
        "e2e_lookup",
        "e2e_explain",
        "e2e_profile",
        "e2e_tx_commit",
        "e2e_tx_rollback",
    ];

    for space in &spaces {
        let _ = db.execute_query(&format!("DROP SPACE IF EXISTS {}", space));
    }
}
