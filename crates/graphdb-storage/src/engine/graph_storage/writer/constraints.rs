use graphdb_core::types::{LabelId, TagInfo};
use graphdb_core::{StorageError, StorageResult, Value};

use super::super::context::GraphStorageContext;
use super::batch::SerialBatchState;

/// Batch variant of [`apply_tag_constraints`] operating on an already
/// resolved [`TagInfo`]. Explicit SERIAL values are validated against the
/// batch pre-scan instead of re-scanning the column per row.
pub(super) fn apply_tag_constraints_prechecked(
    ctx: &GraphStorageContext,
    space_id: u64,
    tag: &TagInfo,
    serial_state: &mut SerialBatchState,
    props: Vec<(String, Value)>,
) -> StorageResult<Vec<(String, Value)>> {
    let mut result = props;
    for prop_def in &tag.properties {
        if let Some((_, value)) = result.iter().find(|(name, _)| name == &prop_def.name) {
            if !prop_def.nullable && value.is_null() {
                return Err(StorageError::null_value_not_allowed(&prop_def.name));
            }
            if prop_def.serial {
                serial_state.check_explicit(
                    ctx,
                    space_id,
                    &tag.tag_name,
                    tag.tag_id,
                    &prop_def.name,
                    value,
                )?;
            }
            continue;
        }
        if prop_def.serial {
            let key = super::super::serial::SerialKey::new(space_id, tag.tag_name.clone());
            let next = ctx.serial_allocator().next(&key);
            result.push((prop_def.name.clone(), Value::BigInt(next as i64)));
            continue;
        }
        if let Some(default) = &prop_def.default {
            result.push((prop_def.name.clone(), default.clone()));
        } else if !prop_def.nullable {
            return Err(StorageError::null_value_not_allowed(&prop_def.name));
        }
    }
    Ok(result)
}

/// Apply tag schema constraints (DEFAULT values, NOT NULL and SERIAL) to a
/// property list before persisting a vertex tag.
pub(super) fn apply_tag_constraints(
    ctx: &GraphStorageContext,
    space: &str,
    tag_name: &str,
    props: Vec<(String, Value)>,
) -> StorageResult<Vec<(String, Value)>> {
    let tag = ctx
        .schema_manager()
        .get_tag(space, tag_name)?
        .ok_or_else(|| StorageError::label_not_found(tag_name.to_string()))?;
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .map(|space| space.space_id)
        .unwrap_or(0);
    let mut result = props;
    for prop_def in &tag.properties {
        if let Some((_, value)) = result.iter().find(|(name, _)| name == &prop_def.name) {
            if !prop_def.nullable && value.is_null() {
                return Err(StorageError::null_value_not_allowed(&prop_def.name));
            }
            if prop_def.serial {
                validate_explicit_serial_value(
                    ctx,
                    space_id,
                    tag_name,
                    tag.tag_id,
                    &prop_def.name,
                    value,
                    super::super::serial::scan_vertex_serial_column,
                )?;
            }
            continue;
        }
        if prop_def.serial {
            // Auto-allocate the next value for this tag's serial column.
            let key = super::super::serial::SerialKey::new(space_id, tag_name.to_string());
            let next = ctx.serial_allocator().next(&key);
            result.push((prop_def.name.clone(), Value::BigInt(next as i64)));
            continue;
        }
        if let Some(default) = &prop_def.default {
            result.push((prop_def.name.clone(), default.clone()));
        } else if !prop_def.nullable {
            return Err(StorageError::null_value_not_allowed(&prop_def.name));
        }
    }
    Ok(result)
}

/// Apply edge schema constraints (DEFAULT values, NOT NULL and SERIAL) to a
/// property list before persisting an edge.
pub(super) fn apply_edge_type_constraints(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    props: Vec<(String, Value)>,
) -> StorageResult<Vec<(String, Value)>> {
    let et = ctx
        .schema_manager()
        .get_edge_type(space, edge_type)?
        .ok_or_else(|| StorageError::label_not_found(edge_type.to_string()))?;
    let space_id = ctx
        .schema_manager()
        .get_space(space)?
        .map(|space| space.space_id)
        .unwrap_or(0);
    let mut result = props;
    for prop_def in &et.properties {
        if let Some((_, value)) = result.iter().find(|(name, _)| name == &prop_def.name) {
            if !prop_def.nullable && value.is_null() {
                return Err(StorageError::null_value_not_allowed(&prop_def.name));
            }
            if prop_def.serial {
                validate_explicit_serial_value(
                    ctx,
                    space_id,
                    edge_type,
                    et.edge_type_id,
                    &prop_def.name,
                    value,
                    super::super::serial::scan_edge_serial_column,
                )?;
            }
            continue;
        }
        if prop_def.serial {
            // Auto-allocate the next value for this edge type's serial column.
            let key = super::super::serial::SerialKey::new(space_id, edge_type.to_string());
            let next = ctx.serial_allocator().next(&key);
            result.push((prop_def.name.clone(), Value::BigInt(next as i64)));
            continue;
        }
        if let Some(default) = &prop_def.default {
            result.push((prop_def.name.clone(), default.clone()));
        } else if !prop_def.nullable {
            return Err(StorageError::null_value_not_allowed(&prop_def.name));
        }
    }
    Ok(result)
}

/// Validate an explicitly supplied SERIAL value and advance the counter.
///
/// Explicit values are rejected when they collide with an already-allocated
/// value in the column's occupied interval. On success the counter is advanced
/// past the supplied value so later auto-allocations never collide with it.
fn validate_explicit_serial_value(
    ctx: &GraphStorageContext,
    space_id: u64,
    table_name: &str,
    label: LabelId,
    prop_name: &str,
    value: &Value,
    scan_column: fn(
        &GraphStorageContext,
        LabelId,
        &str,
    ) -> Option<super::super::serial::SerialColumnScan>,
) -> StorageResult<()> {
    let Some(integer) = serial_value_as_i64(value) else {
        // Non-integer values are rejected later by the column type coercion.
        return Ok(());
    };
    if integer >= 0 {
        if let Some(scan) = scan_column(ctx, label, prop_name) {
            if scan.contains(integer) {
                return Err(StorageError::invalid_operation(format!(
                    "Duplicate value {} for SERIAL column '{}': the value is already allocated",
                    integer, prop_name
                )));
            }
        }
        ctx.serial_allocator().advance_to(
            &super::super::serial::SerialKey::new(space_id, table_name),
            integer as u64,
        );
    }
    Ok(())
}

pub(super) fn serial_value_as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::BigInt(v) => Some(*v),
        Value::Int(v) => Some(*v as i64),
        Value::SmallInt(v) => Some(*v as i64),
        _ => None,
    }
}
