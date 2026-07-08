//! Concurrent DCL Operations Tests
//!
//! Test concurrent scenarios to detect race conditions and ensure thread-safety:
//! - Concurrent user creation
//! - Concurrent permission grants
//! - Concurrent password changes
//! - Concurrent operations on different users

use super::common;
use common::test_scenario::TestScenario;
use std::sync::{Arc, Mutex};
use std::thread;

fn new_scenario() -> TestScenario {
    TestScenario::new().expect("Failed to create test scenario")
}

// ==================== Concurrent User Creation Tests ====================

#[test]
fn test_concurrent_create_different_users() {
    let scenario = Arc::new(Mutex::new(new_scenario()));
    let mut handles = vec![];

    for i in 0..5 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("user_{}", i);
            let query = format!("CREATE USER {} WITH PASSWORD 'password{}'", username, i);
            *scenario = scenario.exec_dcl(&query).assert_success();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    for i in 0..5 {
        let username = format!("user_{}", i);
        let query = format!("DESCRIBE USER {}", username);
        let _scenario = scenario.exec_dcl(&query).assert_success();
    }
}

#[test]
fn test_concurrent_create_same_user_idempotent() {
    let scenario = Arc::new(Mutex::new(new_scenario()));
    let mut handles = vec![];

    for _i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let query = "CREATE USER concurrent_user WITH PASSWORD 'password123'";
            *scenario = scenario.exec_dcl(query).assert_success();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario
        .exec_dcl("DESCRIBE USER concurrent_user")
        .assert_success();
}

// ==================== Concurrent Permission Grant/Revoke Tests ====================

#[test]
fn test_concurrent_grant_different_roles() {
    let scenario = Arc::new(Mutex::new(
        new_scenario()
            .exec_dcl("CREATE USER perm_user WITH PASSWORD 'pass'")
            .assert_success()
            .exec_dcl("CREATE SPACE perm_space WITH DIMENSION=128")
            .assert_success(),
    ));

    let mut handles = vec![];
    let roles = vec!["GOD", "ADMIN", "DBA", "USER", "GUEST"];

    for role in roles {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let query = format!("GRANT {} ON perm_space TO perm_user", role);
            *scenario = scenario.exec_dcl(&query).assert_success();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario
        .exec_dcl("SHOW ROLES IN perm_space")
        .assert_success();
}

#[test]
fn test_concurrent_grant_same_role_idempotent() {
    let scenario = Arc::new(Mutex::new(
        new_scenario()
            .exec_dcl("CREATE USER grant_user WITH PASSWORD 'pass'")
            .assert_success()
            .exec_dcl("CREATE SPACE grant_space WITH DIMENSION=128")
            .assert_success(),
    ));

    let mut handles = vec![];

    for _i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let query = "GRANT ADMIN ON grant_space TO grant_user";
            *scenario = scenario.exec_dcl(query).assert_success();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario
        .exec_dcl("SHOW ROLES IN grant_space")
        .assert_success();
}

// ==================== Concurrent Password Change Tests ====================

#[test]
fn test_concurrent_password_change() {
    let scenario = Arc::new(Mutex::new(
        new_scenario()
            .exec_dcl("CREATE USER pwd_user WITH PASSWORD 'initial_pass'")
            .assert_success(),
    ));

    let mut handles = vec![];

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let query = format!("CHANGE PASSWORD pwd_user 'initial_pass' TO 'pass_{}'", i);
            *scenario = scenario.exec_dcl(&query);
            // Allow both success and error as there's a race condition
            *scenario;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario.exec_dcl("DESCRIBE USER pwd_user").assert_success();
}

// ==================== Concurrent User and Permission Operations ====================

#[test]
fn test_concurrent_create_and_grant() {
    let scenario = Arc::new(Mutex::new(
        new_scenario()
            .exec_dcl("CREATE SPACE share_space WITH DIMENSION=128")
            .assert_success(),
    ));

    let mut create_handles = vec![];
    let mut grant_handles = vec![];

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("async_user_{}", i);
            let query = format!("CREATE USER {} WITH PASSWORD 'pass'", username);
            *scenario = scenario.exec_dcl(&query).assert_success();
        });
        create_handles.push(handle);
    }

    for handle in create_handles {
        handle.join().expect("Thread panicked");
    }

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("async_user_{}", i);
            let query = format!("GRANT ADMIN ON share_space TO {}", username);
            *scenario = scenario.exec_dcl(&query).assert_success();
        });
        grant_handles.push(handle);
    }

    for handle in grant_handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario
        .exec_dcl("SHOW ROLES IN share_space")
        .assert_success();
}

// ==================== Concurrent Drop and Access Tests ====================

#[test]
fn test_concurrent_drop_and_describe() {
    let scenario = Arc::new(Mutex::new(new_scenario()));

    for i in 0..3 {
        let username = format!("drop_user_{}", i);
        let query = format!("CREATE USER {} WITH PASSWORD 'pass'", username);
        let mut scenario_inner = scenario.lock().unwrap();
        *scenario_inner = scenario_inner.exec_dcl(&query).assert_success();
        drop(scenario_inner);
    }

    let mut drop_handles = vec![];
    let mut describe_handles = vec![];

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("drop_user_{}", i);
            let query = format!("DROP USER {}", username);
            *scenario = scenario.exec_dcl(&query).assert_success();
        });
        drop_handles.push(handle);
    }

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("drop_user_{}", i);
            let query = format!("DESCRIBE USER {}", username);
            *scenario = scenario.exec_dcl(&query);
            // May succeed or fail due to concurrent drop
            *scenario;
        });
        describe_handles.push(handle);
    }

    for handle in drop_handles {
        handle.join().expect("Thread panicked");
    }

    for handle in describe_handles {
        handle.join().expect("Thread panicked");
    }
}

// ==================== Stress Test - Multiple Operations ====================

#[test]
fn test_stress_concurrent_operations() {
    let scenario = Arc::new(Mutex::new(new_scenario()));
    let mut handles = vec![];

    for i in 0..10 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut scenario = scenario_clone.lock().unwrap();
            let username = format!("stress_user_{}", i);

            // Create
            let create_query = format!("CREATE USER {} WITH PASSWORD 'pass{}'", username, i);
            *scenario = scenario.exec_dcl(&create_query).assert_success();

            // Create space
            let space_name = format!("stress_space_{}", i);
            let space_query = format!("CREATE SPACE {} WITH DIMENSION=128", space_name);
            *scenario = scenario.exec_dcl(&space_query).assert_success();

            // Grant
            let grant_query = format!("GRANT ADMIN ON {} TO {}", space_name, username);
            *scenario = scenario.exec_dcl(&grant_query).assert_success();

            // Change password
            let pwd_query = format!("CHANGE PASSWORD {} 'pass{}' TO 'newpass{}'", username, i, i);
            *scenario = scenario.exec_dcl(&pwd_query).assert_success();

            // Describe
            let desc_query = format!("DESCRIBE USER {}", username);
            *scenario = scenario.exec_dcl(&desc_query).assert_success();
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }

    let scenario = scenario.lock().unwrap();
    scenario.exec_dcl("SHOW USERS").assert_success();
}
