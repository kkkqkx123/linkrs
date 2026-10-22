//! Standalone SET / REMOVE Guard Tests
//!
//! Test coverage:
//! - Standalone SET with a direct vertex target is rejected explicitly
//! - Standalone REMOVE is rejected explicitly with a user-facing alternative

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
            .is_some_and(|e| e.contains("standalone SET is not supported")),
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
            .is_some_and(|e| e.contains("standalone SET is not supported")),
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
            .is_some_and(|e| e.contains("standalone REMOVE is not supported")),
        "standalone REMOVE should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_set_variable_target_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("SET p.age = 31");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("standalone SET is not supported")),
        "standalone SET on a variable target should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_set_bare_property_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("SET age = 31");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("standalone SET is not supported")),
        "standalone SET on a bare property should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_set_map_overwrite_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("SET p = {age: 31}");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("standalone SET is not supported")),
        "standalone SET with a whole-map target should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}

#[test]
fn test_standalone_set_mixed_map_and_property_rejected() {
    let scenario = TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("test_space")
        .exec_ddl("CREATE TAG Person(id INT, name STRING, age INT)")
        .exec_dml("SET v = {age: 31}, name = 'Bob'");
    assert!(
        scenario
            .error()
            .is_some_and(|e| e.contains("standalone SET is not supported")),
        "standalone SET mixing map overwrite and property assignment should fail explicitly, got: {:?}",
        scenario.error()
    );
    scenario.assert_error();
}
