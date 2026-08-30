use std::sync::Arc;

use graphdb_core::error::QueryError;
use graphdb_core::Value;

use crate::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::executor::streaming::operators::ddl_operator::DdlOperator;

fn build_plan_chunk(plan: &graphdb_migration::MigrationPlan) -> Result<DataChunk, QueryError> {
    let plan_json = serde_json::to_string(plan)
        .map_err(|e| QueryError::execution(format!("failed to serialize plan: {}", e)))?;
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "plan_json".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "safety_level".to_string(),
            data_type: "string".to_string(),
        },
        ColumnInfo {
            name: "estimated_rows".to_string(),
            data_type: "int".to_string(),
        },
        ColumnInfo {
            name: "steps".to_string(),
            data_type: "int".to_string(),
        },
    ]));
    let row = vec![
        Value::string(plan_json),
        Value::string(format!("{:?}", plan.overall_safety)),
        Value::BigInt(plan.estimated_rows as i64),
        Value::Int(plan.steps.len() as i32),
    ];
    Ok(DataChunk::new(vec![row], schema))
}

fn build_report_chunk(
    report: &graphdb_migration::MigrationReport,
) -> Result<DataChunk, QueryError> {
    let schema = Arc::new(Schema::new(vec![
        ColumnInfo {
            name: "success".to_string(),
            data_type: "bool".to_string(),
        },
        ColumnInfo {
            name: "steps_completed".to_string(),
            data_type: "int".to_string(),
        },
        ColumnInfo {
            name: "rows_migrated".to_string(),
            data_type: "int".to_string(),
        },
        ColumnInfo {
            name: "errors".to_string(),
            data_type: "string".to_string(),
        },
    ]));
    let errors_json = serde_json::to_string(&report.errors).unwrap_or_else(|_| "[]".to_string());
    let row = vec![
        Value::Bool(report.success),
        Value::Int(report.steps_completed as i32),
        Value::BigInt(report.rows_migrated as i64),
        Value::string(errors_json),
    ];
    Ok(DataChunk::new(vec![row], schema))
}

pub(super) fn execute_migrate_plan(op: &mut DdlOperator) -> Result<Option<DataChunk>, QueryError> {
    let crate::executor::streaming::operators::ddl_operator::DdlOperatorKind::MigratePlan {
        storage,
        space_name,
        label,
        is_edge,
        from_version,
        to_version,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let storage = storage
        .as_ref()
        .ok_or_else(|| QueryError::execution("storage not available"))?;
    let storage_lock = storage.read();

    let plan = if *is_edge {
        graphdb_migration::generate_edge_plan(
            &*storage_lock,
            space_name,
            label,
            *from_version,
            *to_version,
        )
    } else {
        graphdb_migration::generate_vertex_plan(
            &*storage_lock,
            space_name,
            label,
            *from_version,
            *to_version,
        )
    }
    .map_err(|e| QueryError::execution(e.to_string()))?;

    let chunk = build_plan_chunk(&plan)?;
    Ok(Some(chunk))
}

pub(super) fn execute_migrate_run(op: &mut DdlOperator) -> Result<Option<DataChunk>, QueryError> {
    let crate::executor::streaming::operators::ddl_operator::DdlOperatorKind::MigrateRun {
        storage,
        plan_json,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let storage = storage
        .as_ref()
        .ok_or_else(|| QueryError::execution("storage not available"))?;
    let plan: graphdb_migration::MigrationPlan = serde_json::from_str(plan_json)
        .map_err(|e| QueryError::execution(format!("invalid plan JSON: {}", e)))?;

    let mut storage_write = storage.write();
    let report = graphdb_migration::execute_migration_plan(&mut *storage_write, &plan)
        .map_err(|e| QueryError::execution(e.to_string()))?;

    let chunk = build_report_chunk(&report)?;
    Ok(Some(chunk))
}

pub(super) fn execute_migrate_rollback(
    op: &mut DdlOperator,
) -> Result<Option<DataChunk>, QueryError> {
    let crate::executor::streaming::operators::ddl_operator::DdlOperatorKind::MigrateRollback {
        storage,
        plan_json,
        emitted,
    } = &mut op.kind
    else {
        return Ok(None);
    };
    if *emitted {
        return Ok(None);
    }
    *emitted = true;

    let storage = storage
        .as_ref()
        .ok_or_else(|| QueryError::execution("storage not available"))?;
    let plan: graphdb_migration::MigrationPlan = serde_json::from_str(plan_json)
        .map_err(|e| QueryError::execution(format!("invalid plan JSON: {}", e)))?;

    let mut storage_write = storage.write();
    let report = graphdb_migration::rollback_migration(&mut *storage_write, &plan)
        .map_err(|e| QueryError::execution(e.to_string()))?;

    let chunk = build_report_chunk(&report)?;
    Ok(Some(chunk))
}
