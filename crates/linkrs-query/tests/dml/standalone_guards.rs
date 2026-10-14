//! Standalone SET / REMOVE Guard Tests
//!
//! Test coverage:
//! - Standalone SET with a direct vertex target is rejected explicitly
//! - Standalone REMOVE is rejected explicitly (no storage write path)

use super::common;

use common::test_scenario::TestScenario;

#[test]
fn test_standalone_set_vertex_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .exec_dml("SET 1.age = 31");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("cannot resolve a tag qualifier")),
        "standalone SET should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_set_multi_assignment_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("SET 1.age = 31, 2.age = 32");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("cannot resolve a tag qualifier")),
        "standalone multi-assignment SET should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_remove_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
        .assert_success()
        .exec_dml("REMOVE p.age");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("not wired to storage")),
        "standalone REMOVE should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}
