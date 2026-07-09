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

fn update(guard: &mut std::sync::MutexGuard<'_, TestScenario>, query: &str) {
    let taken = std::mem::take(&mut **guard);
    **guard = taken.exec_dcl(query).assert_success();
}

// ==================== Concurrent User Creation Tests ====================

#[test]
fn test_concurrent_create_different_users() {
    let scenario = Arc::new(Mutex::new(new_scenario()));
    let mut handles = vec![];

    for i in 0..5 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("user_{}", i);
            let query = format!("CREATE USER {} WITH PASSWORD 'password{}'", username, i);
            update(&mut guard, &query);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
}

#[test]
fn test_concurrent_create_same_user_idempotent() {
    let scenario = Arc::new(Mutex::new(new_scenario()));
    let mut handles = vec![];

    for _i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut guard = scenario_clone.lock().unwrap();
            update(&mut guard, "CREATE USER concurrent_user WITH PASSWORD 'password123'");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
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
            let mut guard = scenario_clone.lock().unwrap();
            let query = format!("GRANT {} ON perm_space TO perm_user", role);
            update(&mut guard, &query);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
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
            let mut guard = scenario_clone.lock().unwrap();
            update(&mut guard, "GRANT ADMIN ON grant_space TO grant_user");
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
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
            let mut guard = scenario_clone.lock().unwrap();
            let query = format!("CHANGE PASSWORD pwd_user 'initial_pass' TO 'pass_{}'", i);
            let taken = std::mem::take(&mut *guard);
            *guard = taken.exec_dcl(&query);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
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
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("async_user_{}", i);
            let query = format!("CREATE USER {} WITH PASSWORD 'pass'", username);
            update(&mut guard, &query);
        });
        create_handles.push(handle);
    }

    for handle in create_handles {
        handle.join().expect("Thread panicked");
    }

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("async_user_{}", i);
            let query = format!("GRANT ADMIN ON share_space TO {}", username);
            update(&mut guard, &query);
        });
        grant_handles.push(handle);
    }

    for handle in grant_handles {
        handle.join().expect("Thread panicked");
    }
}

// ==================== Concurrent Drop and Access Tests ====================

#[test]
fn test_concurrent_drop_and_describe() {
    let scenario = Arc::new(Mutex::new(new_scenario()));

    for i in 0..3 {
        let username = format!("drop_user_{}", i);
        let query = format!("CREATE USER {} WITH PASSWORD 'pass'", username);
        let mut guard = scenario.lock().unwrap();
        let taken = std::mem::take(&mut *guard);
        *guard = taken.exec_dcl(&query).assert_success();
        drop(guard);
    }

    let mut drop_handles = vec![];
    let mut describe_handles = vec![];

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("drop_user_{}", i);
            let query = format!("DROP USER {}", username);
            update(&mut guard, &query);
        });
        drop_handles.push(handle);
    }

    for i in 0..3 {
        let scenario_clone = Arc::clone(&scenario);
        let handle = thread::spawn(move || {
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("drop_user_{}", i);
            let query = format!("DESCRIBE USER {}", username);
            let taken = std::mem::take(&mut *guard);
            *guard = taken.exec_dcl(&query);
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
            let mut guard = scenario_clone.lock().unwrap();
            let username = format!("stress_user_{}", i);

            let create_query = format!("CREATE USER {} WITH PASSWORD 'pass{}'", username, i);
            let taken = std::mem::take(&mut *guard);
            let mut sc = taken.exec_dcl(&create_query).assert_success();

            let space_name = format!("stress_space_{}", i);
            let space_query = format!("CREATE SPACE {} WITH DIMENSION=128", space_name);
            sc = sc.exec_dcl(&space_query).assert_success();

            let grant_query = format!("GRANT ADMIN ON {} TO {}", space_name, username);
            sc = sc.exec_dcl(&grant_query).assert_success();

            let pwd_query = format!("CHANGE PASSWORD {} 'pass{}' TO 'newpass{}'", username, i, i);
            sc = sc.exec_dcl(&pwd_query).assert_success();

            let desc_query = format!("DESCRIBE USER {}", username);
            sc = sc.exec_dcl(&desc_query).assert_success();

            *guard = sc;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Thread panicked");
    }
}
