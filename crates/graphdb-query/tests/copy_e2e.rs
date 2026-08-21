use std::io::Write;

use graphdb_query::core::Value;
use std::collections::HashMap;

mod common;
use common::test_scenario::TestScenario;

#[test]
fn test_copy_vertex_parallel() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "vid,name,age").unwrap();
    writeln!(tmp, "1,Alice,30").unwrap();
    writeln!(tmp, "2,Bob,25").unwrap();
    writeln!(tmp, "3,Carol,28").unwrap();
    writeln!(tmp, "4,David,35").unwrap();
    writeln!(tmp, "5,Eve,22").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();

    // Need to keep file alive until after test, but we will persist path string
    // TestScenario creates its own storage, so file must remain.
    // We'll copy file to a stable location in temp dir via keep?
    // Use persist to keep
    let sql = format!(
        "COPY VERTEX person FROM '{}' WITH HEADER BATCH_SIZE 2",
        path
    );
    // Execute with TestScenario - it will handle space
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING, age: INT)")
        .assert_success()
        .query(&sql)
        .assert_success()
        .assert_vertex_count("person", 5)
        .assert_vertex_exists(1, "person")
        .assert_vertex_props(
            3,
            "person",
            HashMap::from([("name", Value::string("Carol")), ("age", Value::Int(28))]),
        );

    drop(tmp);
}

#[test]
fn test_copy_edge_parallel() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "src,dst,since").unwrap();
    writeln!(tmp, "1,2,2020").unwrap();
    writeln!(tmp, "2,3,2021").unwrap();
    writeln!(tmp, "3,4,2022").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    // Need vertices pre-inserted for edge endpoints (1..4)
    // COPY EDGE will insert edges; we need to first create vertices via inserts then copy edges
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE knows(since: INT)")
        .assert_success()
        .exec_dml("INSERT VERTEX person(name) VALUES 1:('A'), 2:('B'), 3:('C'), 4:('D')")
        .assert_success()
        .query(&format!(
            "COPY EDGE knows FROM '{}' WITH HEADER BATCH_SIZE 2",
            path
        ))
        .assert_success()
        .assert_edge_count("knows", 3)
        .assert_edge_exists(1, 2, "knows")
        .assert_edge_exists(3, 4, "knows");
    drop(tmp);
}

#[test]
fn test_copy_explain() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "vid,name").unwrap();
    writeln!(tmp, "1,Alice").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING)")
        .assert_success()
        .query(&format!(
            "EXPLAIN COPY VERTEX person FROM '{}' WITH HEADER",
            path
        ))
        .assert_success()
        .assert_plan_contains("CopyFrom");
    drop(tmp);
}

#[test]
fn test_copy_no_header() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    // No header, columns: vid,name,age order matches tag properties order
    writeln!(tmp, "10,Frank,40").unwrap();
    writeln!(tmp, "11,Grace,31").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING, age: INT)")
        .assert_success()
        .query(&format!(
            "COPY VERTEX person FROM '{}' WITH NO HEADER BATCH_SIZE 1",
            path
        ))
        .assert_success()
        .assert_vertex_count("person", 2)
        .assert_vertex_exists(10, "person");
    drop(tmp);
}

#[test]
fn test_copy_delimiter_semicolon() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    writeln!(tmp, "vid;name;age").unwrap();
    writeln!(tmp, "20;Heidi;27").unwrap();
    writeln!(tmp, "21;Ivan;29").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING, age: INT)")
        .assert_success()
        .query(&format!(
            "COPY VERTEX person FROM '{}' WITH HEADER DELIMITER ';'",
            path
        ))
        .assert_success()
        .assert_vertex_count("person", 2);
    drop(tmp);
}

#[test]
fn test_copy_edge_missing_dst_column_errors() {
    let mut tmp = tempfile::NamedTempFile::new().unwrap();
    // Header names a source column but no destination column: the import
    // must fail instead of guessing which column is the destination.
    writeln!(tmp, "src,since").unwrap();
    writeln!(tmp, "1,2020").unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    TestScenario::new()
        .unwrap()
        .setup_space("test")
        .exec_ddl("CREATE TAG person(name: STRING)")
        .assert_success()
        .exec_ddl("CREATE EDGE knows(since: INT)")
        .assert_success()
        .query(&format!("COPY EDGE knows FROM '{}' WITH HEADER", path))
        .assert_error();
    drop(tmp);
}
