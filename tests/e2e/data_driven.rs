//! Data-driven E2E tests using the pre-generated GQL data files.
//!
//! Each test loads a `.gql` file from `tests/e2e/data/` and verifies
//! the resulting data with count, filter, aggregate, and traversal queries.

use crate::common::{
    assert_count_eq, assert_query_row_count, assert_row_count, create_test_db, load_gql_file,
    setup_test_space,
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

// ---------------------------------------------------------------------------
// staged-WAL / retired-generation bounds after a bulk load
// ---------------------------------------------------------------------------

#[test]
fn test_load_bounds_staged_wal_and_retired_generations() {
    let mut db = create_test_db();
    load_gql_file(&mut db, &format!("{}/optimizer_data.gql", DATA_DIR))
        .expect("Failed to load optimizer_data.gql");

    let storage = db.storage();
    let guard = storage.read();
    let staged_wal = guard.staged_wal_len();
    assert!(
        staged_wal <= 8,
        "staged WAL must not grow unboundedly across 20100 auto-commit statements, got {staged_wal}"
    );
    let retired = guard.retired_generation_count();
    assert!(
        retired <= 64,
        "retired index generations must stay bounded, got {retired}"
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
// P4/P6: batch load reuses the auto-commit window and DML plan memo
// ---------------------------------------------------------------------------

#[test]
fn test_batch_load_reuses_dml_plan() {
    let mut db = create_test_db();
    // 10000 person INSERTs (one shape) followed by 10000 works_at edge INSERTs
    // (another shape) must reuse the same-shape physical plan instead of
    // re-extracting params / rebuilding cache keys per statement.
    load_gql_file(&mut db, &format!("{}/optimizer_data.gql", DATA_DIR))
        .expect("Failed to load optimizer_data.gql");

    let hits = db.query_api().dml_plan_memo_hits();
    assert!(
        hits > 15_000,
        "consecutive same-shape DML should hit the plan memo, got {hits}"
    );
    assert_count_eq(
        &mut db,
        "MATCH (p:person) RETURN count(p)",
        10000,
        "10000 persons",
    );
    assert_count_eq(
        &mut db,
        "MATCH ()-[w:works_at]->() RETURN count(w)",
        10000,
        "10000 works_at edges",
    );
}

// ---------------------------------------------------------------------------
// shape-normalized DML plan-cache correctness
// ---------------------------------------------------------------------------

#[test]
fn test_dml_shape_cache_roundtrip() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "cache_space",
        &["CREATE TAG person(name: STRING, age: INT, city: STRING)"],
        &[],
    )
    .expect("setup space");

    // Distinct literal values per statement — must round-trip through the
    // parameterized (shape-cached) execution path.
    let rows = [
        ("p001", "Alice", 30i64, "Beijing"),
        ("p002", "Bob", 25i64, "Shanghai"),
        ("p003", "Carol", 41i64, "Shenzhen"),
        ("p004", "David", 19i64, "Guangzhou"),
    ];
    for (vid, name, age, city) in rows {
        let query = format!(
            "INSERT VERTEX person(name, age, city) VALUES \"{}\": (\"{}\", {}, \"{}\")",
            vid, name, age, city
        );
        db.execute_query(&query).expect("insert should succeed");
    }

    for (vid, name, age, city) in rows {
        let result = db
            .execute_query(&format!(
                "MATCH (p:person) WHERE id(p) == \"{}\" RETURN p.name, p.age, p.city",
                vid
            ))
            .expect("lookup should succeed");
        let row = result.rows.first().expect("row should exist");
        let key = |name: &str| -> String {
            result
                .columns
                .iter()
                .find(|c| **c == name || **c == format!("p.{name}"))
                .cloned()
                .unwrap_or_else(|| panic!("no column for {name}, columns={:?}", result.columns))
        };
        let name_val = row.values.get(&key("name")).expect("name col");
        let age_val = row.values.get(&key("age")).expect("age col");
        let city_val = row.values.get(&key("city")).expect("city col");
        assert_eq!(*name_val, Value::from(name), "name for {vid}");
        assert_eq!(*age_val, Value::from(age), "age for {vid}");
        assert_eq!(*city_val, Value::from(city), "city for {vid}");
    }

    // The four same-shape INSERTs should have produced plan-cache hits after
    // the first one compiled the template.
    let hit_rate = db.query_api().plan_cache_hit_rate();
    assert!(
        hit_rate > 0.0,
        "expected DML plan-cache hits, hit rate was {hit_rate}"
    );
}

#[test]
fn test_dml_shape_cache_edge_roundtrip() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "edge_cache_space",
        &[
            "CREATE TAG person(name: STRING)",
            "CREATE TAG company(name: STRING)",
        ],
        &["CREATE EDGE works_at(position: STRING, salary: INT) FROM person TO company"],
    )
    .expect("setup space");

    let edges = [
        ("p001", "c001", 0i64, "Engineer", 45000i64),
        ("p002", "c002", 1i64, "Manager", 90000i64),
        ("p003", "c003", 2i64, "Analyst", 60000i64),
    ];
    for (vid, name) in [
        ("p001", "Person A"),
        ("p002", "Person B"),
        ("p003", "Person C"),
    ] {
        db.execute_query(&format!(
            "INSERT VERTEX person(name) VALUES \"{}\": (\"{}\")",
            vid, name
        ))
        .expect("person insert should succeed");
    }
    for (vid, name) in [
        ("c001", "Company A"),
        ("c002", "Company B"),
        ("c003", "Company C"),
    ] {
        db.execute_query(&format!(
            "INSERT VERTEX company(name) VALUES \"{}\": (\"{}\")",
            vid, name
        ))
        .expect("company insert should succeed");
    }
    for (src, dst, rank, position, salary) in edges {
        let query = format!(
            "INSERT EDGE works_at(position, salary) VALUES \"{}\" -> \"{}\" @{}: (\"{}\", {})",
            src, dst, rank, position, salary
        );
        db.execute_query(&query)
            .expect("edge insert should succeed");
    }

    let result = db
        .execute_query("MATCH (a:person)-[w:works_at]->(b:company) RETURN w.position, w.salary")
        .expect("edge query");
    assert_eq!(result.rows.len(), 3, "all three edges loaded");
}

#[test]
fn test_read_streaming_plan_cache_hit() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "stream_cache_space",
        &["CREATE TAG person(name: STRING)"],
        &[],
    )
    .expect("setup space");
    db.execute_query("INSERT VERTEX person(name) VALUES \"p1\": (\"Alice\")")
        .expect("insert should succeed");

    let query = "MATCH (p:person) WHERE id(p) == \"p1\" RETURN p.name";

    // First streaming execution compiles and stores the plan.
    let first = db
        .execute_stream_query(query)
        .expect("first streaming read should succeed");
    let first_rows = first.collect().expect("collect first").rows;
    assert_eq!(first_rows.len(), 1, "first streaming read should return one row");

    // Second identical streaming read must reuse the cached plan.
    let second = db
        .execute_stream_query(query)
        .expect("second streaming read should succeed");
    let second_rows = second.collect().expect("collect second").rows;
    assert_eq!(
        second_rows, first_rows,
        "identical streaming reads should produce the same rows"
    );

    let hit_rate = db.query_api().plan_cache_hit_rate();
    assert!(
        hit_rate > 0.0,
        "expected streaming read plan-cache hit, hit rate was {hit_rate}"
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
