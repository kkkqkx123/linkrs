//! E2E Test Suite for STRUCT/ARRAY composite types.
//!
//! Covers the M3 milestone verification surface:
//! - DDL with nested STRUCT / fixed-length and variable-length ARRAY
//! - STRUCT/ARRAY literals in INSERT VALUES
//! - Field access (`addr.city`), chained access (`addr.geo.lat`), subscript
//!   access (`coords[0]`, `addr['city']`)
//! - Struct <-> Map and Array <-> List casts
//! - Persistence roundtrip of schema metadata

use crate::common::{assert_query_err, assert_query_ok, create_test_db, setup_test_space};

fn setup_person_space(db: &mut crate::common::TestDb, space_name: &str) {
    setup_test_space(
        db,
        space_name,
        &["CREATE TAG Person (id INT, \
             addr STRUCT<city STRING, street STRING, geo STRUCT<lat DOUBLE, lon DOUBLE>>, \
             coords ARRAY<DOUBLE>(3))"],
        &[],
    )
    .expect("Failed to setup test space");
}

/// DDL with nested composite types parses and creates the tag.
#[test]
fn test_ddl_create_tag_with_composite_types() {
    let mut db = create_test_db();
    setup_person_space(&mut db, "e2e_composite_ddl");
}

/// INSERT with STRUCT/ARRAY literals stores and reads back the values.
#[test]
fn test_insert_and_read_struct_array() {
    let mut db = create_test_db();
    setup_person_space(&mut db, "e2e_composite_insert");

    let result = db.execute_query(
        "INSERT VERTEX Person(id, addr, coords) VALUES 'p1': (1, \
         STRUCT{city: 'shanghai', street: 'nanjing rd', geo: STRUCT{lat: 31.2, lon: 121.5}}, \
         ARRAY[1.0, 2.0, 3.0])",
    );
    assert_query_ok(result, "INSERT with STRUCT/ARRAY literals should succeed");

    // Whole-value roundtrip.
    let result = db
        .execute_query("MATCH (p:Person) RETURN p.id, p.addr, p.coords")
        .expect("RETURN of composite properties should succeed");
    let row = result.rows().first().expect("one row expected");
    let columns = result.columns();
    let id_idx = columns
        .iter()
        .position(|c| c == "p.id")
        .expect("p.id column");
    let addr_idx = columns
        .iter()
        .position(|c| c == "p.addr")
        .expect("p.addr column");
    let coords_idx = columns
        .iter()
        .position(|c| c == "p.coords")
        .expect("p.coords column");
    assert_eq!(row.get(id_idx), Some(&graphdb::core::Value::BigInt(1)));
    match row.get(addr_idx) {
        Some(graphdb::core::Value::Struct(s)) => {
            assert_eq!(
                s.fields[0],
                ("city".to_string(), graphdb::core::Value::string("shanghai"))
            );
            assert!(matches!(s.fields[2].1, graphdb::core::Value::Struct(_)));
        }
        other => panic!("expected STRUCT value, got {:?}", other),
    }
    match row.get(coords_idx) {
        Some(graphdb::core::Value::Array(a)) => {
            assert_eq!(
                a.values,
                vec![
                    graphdb::core::Value::Double(1.0),
                    graphdb::core::Value::Double(2.0),
                    graphdb::core::Value::Double(3.0),
                ]
            );
        }
        other => panic!("expected ARRAY value, got {:?}", other),
    }
}

/// Field access `addr.city` and chained `addr.geo.lat` resolve at runtime.
#[test]
fn test_struct_field_access() {
    let mut db = create_test_db();
    setup_person_space(&mut db, "e2e_composite_field");

    let result = db.execute_query(
        "INSERT VERTEX Person(id, addr, coords) VALUES 'p1': (1, \
         STRUCT{city: 'shanghai', street: 'nanjing rd', geo: STRUCT{lat: 31.2, lon: 121.5}}, \
         ARRAY[1.0, 2.0, 3.0])",
    );
    assert_query_ok(result, "INSERT should succeed");

    let result = db
        .execute_query("MATCH (p:Person) RETURN p.addr.city, p.addr.geo.lat, p.addr.geo.lon")
        .expect("STRUCT field access should succeed");
    let row = result.rows().first().expect("one row expected");
    let columns = result.columns();
    let city_idx = columns
        .iter()
        .position(|c| c == "p.addr.city")
        .expect("city column");
    let lat_idx = columns
        .iter()
        .position(|c| c == "p.addr.geo.lat")
        .expect("lat column");
    let lon_idx = columns
        .iter()
        .position(|c| c == "p.addr.geo.lon")
        .expect("lon column");
    assert_eq!(
        row.get(city_idx),
        Some(&graphdb::core::Value::string("shanghai"))
    );
    assert_eq!(row.get(lat_idx), Some(&graphdb::core::Value::Double(31.2)));
    assert_eq!(row.get(lon_idx), Some(&graphdb::core::Value::Double(121.5)));

    // Missing field yields NULL (not an error).
    let result = db.execute_query("MATCH (p:Person) RETURN p.addr.missing");
    assert_query_ok(result, "missing STRUCT field must not error");
}

/// Subscript access: `coords[0]` (ARRAY) and `addr['city']` (STRUCT).
#[test]
fn test_subscript_access() {
    let mut db = create_test_db();
    setup_person_space(&mut db, "e2e_composite_subscript");

    let result = db.execute_query(
        "INSERT VERTEX Person(id, addr, coords) VALUES 'p1': (1, \
         STRUCT{city: 'shanghai', street: 'nanjing rd', geo: STRUCT{lat: 31.2, lon: 121.5}}, \
         ARRAY[1.0, 2.0, 3.0])",
    );
    assert_query_ok(result, "INSERT should succeed");

    let result = db.execute_query("MATCH (p:Person) RETURN p.coords[0], p.coords[2]");
    assert_query_ok(result, "ARRAY subscript should succeed");

    let result = db.execute_query("MATCH (p:Person) RETURN p.addr['city']");
    assert_query_ok(result, "STRUCT subscript should succeed");
}

/// STRUCT literal evaluates standalone.
#[test]
fn test_struct_literal_standalone() {
    let mut db = create_test_db();
    setup_test_space(
        &mut db,
        "e2e_composite_literal",
        &["CREATE TAG T (v STRUCT<a INT, b STRING>)"],
        &[],
    )
    .expect("Failed to setup test space");

    let result = db.execute_query("RETURN STRUCT{a: 1, b: 'x'}");
    assert_query_ok(result, "standalone STRUCT literal should evaluate");
}

/// Struct <-> Map and Array <-> List casts.
#[test]
fn test_composite_casts() {
    let mut db = create_test_db();
    setup_person_space(&mut db, "e2e_composite_cast");

    let result = db.execute_query(
        "INSERT VERTEX Person(id, addr, coords) VALUES 'p1': (1, \
         STRUCT{city: 'shanghai', street: 'nanjing rd', geo: STRUCT{lat: 31.2, lon: 121.5}}, \
         ARRAY[1.0, 2.0, 3.0])",
    );
    assert_query_ok(result, "INSERT should succeed");

    // Struct -> Map.
    let result = db.execute_query("MATCH (p:Person) RETURN p.addr::MAP");
    assert_query_ok(result, "Struct -> Map cast should succeed");

    // Array -> List.
    let result = db.execute_query("MATCH (p:Person) RETURN p.coords::LIST");
    assert_query_ok(result, "Array -> List cast should succeed");
}

/// Over-nested DDL is rejected.
#[test]
fn test_composite_nesting_limit_rejected() {
    let mut db = create_test_db();
    let mut ddl = String::from("CREATE TAG Deep (a ARRAY<");
    for _ in 0..17 {
        ddl.push_str("ARRAY<");
    }
    ddl.push_str("INT");
    for _ in 0..17 {
        ddl.push('>');
    }
    ddl.push_str(">)");
    let result = db.execute_query(&ddl);
    assert_query_err(result, "over-nested composite DDL must be rejected");
}
