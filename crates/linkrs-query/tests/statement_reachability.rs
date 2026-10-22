//! Statement-level reachability contract: every `Stmt` variant terminates.
//!
//! Each [`InventoryEntry`](common::statement_inventory::InventoryEntry) pins
//! one variant to its user-visible terminal: executable with content
//! assertions, explicitly rejected with guidance, or rejected at parse time.
//! No path may surface the internal planner leak.
//!
//! The inventory lives in `common/statement_inventory.rs`; this file only
//! drives it. `integration_deadend_statements.rs` keeps the deep behavioral
//! tests for the previously dead statements.

mod common;

use common::statement_inventory::{Expectation, INVENTORY};
use common::test_scenario::TestScenario;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::parser::Parser;

/// Internal planner leak that must never reach users.
const PLANNER_LEAK: &str = "No planner for bound statement";

fn feature_enabled(feature: &str) -> bool {
    match feature {
        "fulltext" => cfg!(feature = "fulltext"),
        "vector" => cfg!(feature = "vector"),
        _ => false,
    }
}

fn run_entry(
    variant: &str,
    sql: &str,
    ddl_setup: &[&str],
    dml_setup: &[&str],
    expectation: &Expectation,
) {
    let mut scenario = TestScenario::new().expect("scenario").setup_space("reach");
    for ddl in ddl_setup {
        scenario = scenario.exec_ddl(ddl).assert_success();
    }
    for dml in dml_setup {
        scenario = scenario.exec_dml(dml).assert_success();
    }
    scenario = scenario.query(sql);
    match expectation {
        Expectation::Executable { columns, min_rows } => {
            assert!(
                scenario.error().is_none(),
                "{variant} must execute, got error: {:?}",
                scenario.error()
            );
            if let Some(expected) = columns {
                let Some(ExecutionResult::DataSet { data, .. }) = scenario.last_result() else {
                    panic!("{variant} must return a dataset for column assertions");
                };
                let actual: Vec<&str> = data.col_names.iter().map(String::as_str).collect();
                assert_eq!(actual, *expected, "{variant} column mismatch");
            }
            let count = scenario.last_result().map(|r| r.count()).unwrap_or(0);
            assert!(
                count >= *min_rows,
                "{variant} returned {count} rows, expected at least {min_rows}"
            );
        }
        Expectation::Rejected { guidance } => {
            let error = scenario
                .error()
                .unwrap_or_else(|| panic!("{variant} must be rejected, but {sql:?} succeeded"));
            assert!(
                !error.contains(PLANNER_LEAK),
                "{variant} leaks the internal planner error: {error}"
            );
            for needle in *guidance {
                assert!(
                    error.contains(needle),
                    "{variant} error must guide with {needle:?}, got: {error}"
                );
            }
        }
        Expectation::ParseRejected => {
            panic!("{variant} is marked ParseRejected but reached execution for {sql:?}");
        }
    }
}

#[test]
fn inventory_covers_every_stmt_variant() {
    const STMT_SRC: &str = include_str!("../src/parser/ast/stmt.rs");
    let body = STMT_SRC
        .split("pub enum Stmt {")
        .nth(1)
        .expect("Stmt enum must exist");
    let body = body
        .split("}\n\ncrate::define_stmt_helpers")
        .next()
        .expect("enum end");
    let mut variants = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if !line.ends_with("),") {
            continue;
        }
        let name = line.split('(').next().unwrap_or("").trim();
        if !name.is_empty()
            && name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && name.chars().all(|c| c.is_ascii_alphanumeric())
        {
            variants.push(name.to_string());
        }
    }
    assert!(
        !variants.is_empty(),
        "variant extraction must find the Stmt enum"
    );
    let listed: Vec<&str> = INVENTORY.iter().map(|e| e.variant).collect();
    for variant in &variants {
        assert!(
            listed.contains(&variant.as_str()),
            "Stmt variant {variant} has no inventory entry"
        );
    }
    assert_eq!(
        listed.len(),
        variants.len(),
        "inventory must list each Stmt variant exactly once"
    );
    let mut seen = std::collections::HashSet::new();
    for variant in listed {
        assert!(seen.insert(variant), "duplicate inventory entry: {variant}");
    }
}

#[test]
fn statement_reachability_contract() {
    for entry in INVENTORY {
        let mut parser = Parser::new(entry.sql);
        let parsed = parser.parse();
        let active = entry.feature.map(feature_enabled).unwrap_or(true);
        if !active {
            let without = entry
                .without_feature
                .unwrap_or_else(|| panic!("{} needs a without_feature expectation", entry.variant));
            let Expectation::Rejected { .. } = without else {
                panic!("{} without_feature must be Rejected", entry.variant);
            };
            assert!(
                parsed.is_ok() && !parser.has_errors(),
                "{} SQL must still parse without the feature: {:?}",
                entry.variant,
                entry.sql
            );
            let ast = parsed.expect("parse").ast;
            assert_eq!(
                ast.stmt().kind(),
                entry.kind,
                "{} kind mismatch",
                entry.variant
            );
            assert_eq!(
                ast.stmt().category().as_str(),
                entry.category,
                "{} category mismatch",
                entry.variant
            );
            run_entry(entry.variant, entry.sql, &[], &[], &without);
            continue;
        }
        match entry.expectation {
            Expectation::ParseRejected => {
                assert!(
                    parsed.is_err() || parser.has_errors(),
                    "{} must be rejected at parse time: {:?}",
                    entry.variant,
                    entry.sql
                );
            }
            _ => {
                assert!(
                    parsed.is_ok(),
                    "{} SQL must parse: {:?}",
                    entry.variant,
                    entry.sql
                );
                assert!(
                    !parser.has_errors(),
                    "{} SQL must parse without recovery errors: {:?}",
                    entry.variant,
                    entry.sql
                );
                let ast = parsed.expect("parse").ast;
                assert_eq!(
                    ast.stmt().kind(),
                    entry.kind,
                    "{} kind mismatch",
                    entry.variant
                );
                assert_eq!(
                    ast.stmt().category().as_str(),
                    entry.category,
                    "{} category mismatch",
                    entry.variant
                );
                run_entry(
                    entry.variant,
                    entry.sql,
                    entry.ddl_setup,
                    entry.dml_setup,
                    &entry.expectation,
                );
            }
        }
    }
    let _ = std::fs::remove_dir_all("/tmp/opencode/statement_inventory_export");
}
