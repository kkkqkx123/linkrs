use graphdb::core::{DataType, Value};
use graphdb::migration::{
    convert_value, generate_edge_plan, generate_vertex_plan, is_compatible_type, MigrationPlan,
    MigrationReport, MigrationTarget, SafetyLevel, VersionRange,
};
use graphdb::storage::MockStorage;
use graphdb_core::value::null::NullType;

#[test]
fn test_generate_vertex_plan_empty() {
    let storage = MockStorage::new().expect("mock storage");
    let plan = generate_vertex_plan(&storage, "test_space", "test_tag", 1, 2);
    assert!(
        plan.is_ok(),
        "plan generation should succeed: {:?}",
        plan.err()
    );
    let plan = plan.unwrap();
    assert_eq!(plan.target.space, "test_space");
    assert_eq!(plan.target.label, "test_tag");
    assert!(!plan.target.is_edge);
    assert_eq!(plan.version_range.from, 1);
    assert_eq!(plan.version_range.to, 2);
}

#[test]
fn test_generate_edge_plan_empty() {
    let storage = MockStorage::new().expect("mock storage");
    let plan = generate_edge_plan(&storage, "test_space", "my_edge", 1, 2);
    assert!(plan.is_ok());
    let plan = plan.unwrap();
    assert!(plan.target.is_edge);
    assert_eq!(plan.target.label, "my_edge");
}

#[test]
fn test_execute_migration_plan_empty() {
    let mut storage = MockStorage::new().expect("mock storage");
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "test_space".into(),
            label: "test_tag".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![],
        0,
        SafetyLevel::Safe,
        None,
    );
    let report = graphdb::migration::execute_migration_plan(&mut storage, &plan);
    assert!(report.is_ok());
    let report = report.unwrap();
    assert!(report.success);
}

#[test]
fn test_rollback_migration_no_plan() {
    let mut storage = MockStorage::new().expect("mock storage");
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "test_space".into(),
            label: "test_tag".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![],
        0,
        SafetyLevel::Safe,
        None,
    );
    let report = graphdb::migration::rollback_migration(&mut storage, &plan);
    assert!(report.is_err());
}

#[test]
fn test_type_conversion() {
    let value = Value::Int(42);
    let result = convert_value(&value, &DataType::BigInt);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Value::BigInt(42));
    assert!(is_compatible_type(&DataType::Int, &DataType::BigInt));
    assert!(!is_compatible_type(&DataType::Int, &DataType::Date));
}

#[test]
fn test_migration_plan_serde() {
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "l".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![],
        10,
        SafetyLevel::Safe,
        None,
    );
    let json = serde_json::to_string(&plan).expect("serialize plan");
    let decoded: MigrationPlan = serde_json::from_str(&json).expect("deserialize plan");
    assert_eq!(decoded.target.space, "s");
    assert_eq!(decoded.estimated_rows, 10);
}

#[test]
fn test_migration_report_serde() {
    let report = MigrationReport {
        success: true,
        steps_completed: 1,
        rows_migrated: 5,
        errors: vec![],
        completed_step_indices: vec![0],
    };
    let json = serde_json::to_string(&report).unwrap();
    let decoded: MigrationReport = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded.rows_migrated, 5);
}

#[test]
fn test_null_conversion() {
    let null_val = Value::Null(NullType::Null);
    let result = convert_value(&null_val, &DataType::String).unwrap();
    assert!(result.is_null());
}

#[test]
fn test_parse_migrate_plan() {
    let mut parser = graphdb::query::parser::Parser::new(
        "MIGRATE PLAN FOR TAG person FROM VERSION 1 TO 2 IN test_space",
    );
    let result = parser.parse();
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let ast = result.unwrap().ast;
    match &ast.stmt {
        graphdb::query::parser::ast::Stmt::Migrate(m) => match m {
            graphdb::query::parser::ast::MigrateStmt::Plan(p) => {
                assert_eq!(p.label, "person");
                assert_eq!(p.space, "test_space");
                assert!(!p.is_edge);
                assert_eq!(p.from_version, 1);
                assert_eq!(p.to_version, 2);
            }
            _ => panic!("expected Plan"),
        },
        other => panic!("expected Migrate, got {:?}", other.kind()),
    }
}

#[test]
fn test_parse_migrate_execute() {
    let mut parser = graphdb::query::parser::Parser::new("MIGRATE EXECUTE '{\"test\":1}'");
    let result = parser.parse();
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let ast = result.unwrap().ast;
    assert!(matches!(
        ast.stmt,
        graphdb::query::parser::ast::Stmt::Migrate(_)
    ));
}

#[test]
fn test_parse_migrate_edge() {
    let mut parser = graphdb::query::parser::Parser::new(
        "MIGRATE PLAN FOR EDGE knows FROM VERSION 2 TO 5 IN my_space",
    );
    let result = parser.parse();
    assert!(result.is_ok(), "parse failed: {:?}", result.err());
    let ast = result.unwrap().ast;
    match &ast.stmt {
        graphdb::query::parser::ast::Stmt::Migrate(m) => match m {
            graphdb::query::parser::ast::MigrateStmt::Plan(p) => {
                assert!(p.is_edge);
                assert_eq!(p.label, "knows");
            }
            _ => panic!("expected Plan"),
        },
        other => panic!("expected Migrate, got {:?}", other.kind()),
    }
}

#[test]
fn test_ddl_migrate_plan_operator() {
    use graphdb::query::executor::streaming::operators::spec::DdlSpec;
    use graphdb::query::executor::streaming::operators::ddl_operator::DdlOperator;
    use graphdb::query::executor::streaming::slot::SlotLayout;
    use std::sync::Arc;
    let spec = DdlSpec::MigratePlan {
        space_name: "test_space".to_string(),
        label: "person".to_string(),
        is_edge: false,
        from_version: 1,
        to_version: 2,
    };
    let storage = Arc::new(parking_lot::RwLock::new(MockStorage::new().unwrap()))
        as std::sync::Arc<parking_lot::RwLock<dyn graphdb::storage::QueryStorage>>;
    let layout = Arc::new(SlotLayout::new(vec![]));
    let op = DdlOperator::from_spec(&spec, Some(storage), layout);
    match op.kind {
        graphdb::query::executor::streaming::operators::ddl_operator::DdlOperatorKind::MigratePlan {
            space_name,
            label,
            is_edge,
            from_version,
            to_version,
            ..
        } => {
            assert_eq!(space_name, "test_space");
            assert_eq!(label, "person");
            assert!(!is_edge);
            assert_eq!(from_version, 1);
            assert_eq!(to_version, 2);
        }
        _ => panic!("expected MigratePlan"),
    }
}
