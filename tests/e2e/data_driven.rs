//! Data-driven E2E tests using the pre-generated GQL data files.
//!
//! Each test loads a `.gql` file from `tests/e2e/data/` and verifies
//! the resulting data with count, filter, aggregate, and traversal queries.

use crate::common::{
    assert_count_eq, assert_query_row_count, assert_row_count, create_test_db, load_gql_file,
};
use graphdb::core::Value;

const DATA_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/e2e/data");

// ---------------------------------------------------------------------------
// e2e_social_network – 20 persons, 5 companies, 30 friend, 20 works_at,
//                        20 lives_in edges
// ---------------------------------------------------------------------------

#[test]
fn test_social_network_vertex_counts() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/social_network_data.gql", DATA_DIR))
        .expect("Failed to load social_network_data.gql");

    assert_count_eq(
        &mut db,
        "MATCH (p:person) RETURN count(p)",
        20,
        "person count",
    );
    assert_count_eq(
        &mut db,
        "MATCH (c:company) RETURN count(c)",
        5,
        "company count",
    );
}

#[test]
fn test_social_network_edge_counts() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/social_network_data.gql", DATA_DIR))
        .expect("Failed to load social_network_data.gql");

    assert_count_eq(
        &mut db,
        "MATCH ()-[f:friend]->() RETURN count(f)",
        30,
        "friend count",
    );
    assert_count_eq(
        &mut db,
        "MATCH ()-[w:works_at]->() RETURN count(w)",
        20,
        "works_at count",
    );
    assert_count_eq(
        &mut db,
        "MATCH ()-[l:lives_in]->() RETURN count(l)",
        20,
        "lives_in count",
    );
}

#[test]
fn test_social_network_filter() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/social_network_data.gql", DATA_DIR))
        .expect("Failed to load social_network_data.gql");

    // People in Beijing
    assert_count_eq(
        &mut db,
        "MATCH (p:person) WHERE p.city == 'Beijing' RETURN count(p)",
        6,
        "Beijing people count",
    );
    // People aged >= 30
    assert_count_eq(
        &mut db,
        "MATCH (p:person) WHERE p.age >= 30 RETURN count(p)",
        12,
        "people aged >= 30",
    );
}

#[test]
fn test_social_network_lookup_index() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/social_network_data.gql", DATA_DIR))
        .expect("Failed to load social_network_data.gql");

    // By name
    assert_row_count(
        db.execute_query("LOOKUP ON person WHERE person.name == 'Alice' YIELD person.name"),
        1,
        "lookup Alice",
    );
    // By age range (Bob 35, Jack 36, Paul 35 -> 3 people)
    assert_row_count(
        db.execute_query("LOOKUP ON person WHERE person.age > 34 YIELD person.name"),
        3,
        "lookup age > 34",
    );
}

#[test]
fn test_social_network_go_traversal() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/social_network_data.gql", DATA_DIR))
        .expect("Failed to load social_network_data.gql");

    // p1 has incoming friend edges in the sample data, so use REVERSELY to verify traversal.
    let result = db
        .execute_query("GO 1 STEP FROM 'p1' OVER friend REVERSELY YIELD friend.name")
        .expect("GO from p1");
    assert!(
        !result.rows.is_empty(),
        "p1 should have at least one reverse friend"
    );
}

// ---------------------------------------------------------------------------
// e2e_ecommerce – 100 users, 200 products, 500 orders
// ---------------------------------------------------------------------------

#[test]
fn test_ecommerce_vertex_counts() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/ecommerce_data.gql", DATA_DIR))
        .expect("Failed to load ecommerce_data.gql");

    assert_count_eq(&mut db, "MATCH (u:user) RETURN count(u)", 100, "user count");
    assert_count_eq(
        &mut db,
        "MATCH (p:product) RETURN count(p)",
        200,
        "product count",
    );
    assert_count_eq(
        &mut db,
        "MATCH (o:order) RETURN count(o)",
        500,
        "order count",
    );
}

// ---------------------------------------------------------------------------
// e2e_geography – 10 cities, 200 locations
// ---------------------------------------------------------------------------

#[test]
fn test_geography_vertex_counts() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/geography_data.gql", DATA_DIR))
        .expect("Failed to load geography_data.gql");

    assert_count_eq(&mut db, "MATCH (c:city) RETURN count(c)", 10, "city count");
    assert_count_eq(
        &mut db,
        "MATCH (l:location) RETURN count(l)",
        200,
        "location count",
    );
}

// ---------------------------------------------------------------------------
// e2e_optimizer – 10000 persons + 10000 works_at edges
// ---------------------------------------------------------------------------

#[test]
fn test_optimizer_vertex_count() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/optimizer_data.gql", DATA_DIR))
        .expect("Failed to load optimizer_data.gql");

    assert_count_eq(
        &mut db,
        "MATCH (p:person) RETURN count(p)",
        10000,
        "10000 persons",
    );
}

#[test]
fn test_optimizer_aggregate() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/optimizer_data.gql", DATA_DIR))
        .expect("Failed to load optimizer_data.gql");

    // SUM of salaries
    let result = db
        .execute_query("MATCH (p:person) RETURN sum(p.salary) AS total_salary")
        .expect("sum salary");
    let first_row = result.rows.first().expect("sum result should have a row");
    let total = first_row
        .values
        .values()
        .next()
        .expect("total_salary value");
    match total {
        Value::BigInt(v) => assert!(*v > 0, "total salary should be > 0"),
        Value::Int(v) => assert!(*v > 0, "total salary should be > 0"),
        _ => panic!("unexpected value type for sum: {:?}", total),
    }

    // GROUP BY city
    assert_query_row_count(
        &mut db,
        "MATCH (p:person) RETURN p.city, count(*) GROUP BY p.city",
        5,
        "distinct cities",
    );
}

// ---------------------------------------------------------------------------
// e2e_vector – 1000 product_vector entries
// ---------------------------------------------------------------------------

#[test]
fn test_vector_vertex_count() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/vector_data.gql", DATA_DIR))
        .expect("Failed to load vector_data.gql");

    assert_count_eq(
        &mut db,
        "MATCH (p:product_vector) RETURN count(p)",
        1000,
        "vector product count",
    );
}
