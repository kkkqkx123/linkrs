use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::{EdgeDirection, Value};
use crate::query::executor::streaming::chunk::{ColumnInfo, DataChunk, Schema};
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::storage::StorageClient;

use super::common;

pub(super) fn handle(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !base.lifecycle.is_opened() {
        return Err(QueryError::execution("Expand not opened".to_string()));
    }

    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
    let chunk = input.advance()?;
    if let Some(chunk) = chunk {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            common::expand_on_chunk(
                chunk,
                &*reader,
                space_name,
                edge_types,
                direction,
                filter_expr,
                cancel_token,
            )
        } else {
            let mut new_cols: Vec<ColumnInfo> = chunk
                .schema
                .columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                })
                .collect();
            new_cols.push(ColumnInfo {
                name: "_expand_edge_type".to_string(),
                data_type: "string".to_string(),
            });
            new_cols.push(ColumnInfo {
                name: "_expand_direction".to_string(),
                data_type: "string".to_string(),
            });
            let schema = Arc::new(Schema::new(new_cols));
            let mut rows = chunk.rows;
            for row in rows.iter_mut() {
                row.push(Value::String(edge_types.join("/")));
                row.push(Value::String(format!("{:?}", direction).to_lowercase()));
            }
            let out_col_names = schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>();
            rows.retain(|row| common::row_passes_filter(row, &out_col_names, filter_expr));
            Ok(Some(DataChunk::new(rows, schema)))
        }
    } else {
        Ok(None)
    }
}

pub(super) fn handle_all(
    storage: &Option<Arc<RwLock<dyn StorageClient>>>,
    space_name: &str,
    edge_types: &[String],
    direction: EdgeDirection,
    filter_expr: &Option<Expression>,
    base: &mut OperatorBase,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    if !base.lifecycle.is_opened() {
        return Err(QueryError::execution("ExpandAll not opened".to_string()));
    }

    let cancel_token = base.runtime.as_ref().map(|rt| rt.cancel_token());
    let chunk = input.advance()?;
    if let Some(chunk) = chunk {
        if let Some(storage_lock) = storage {
            let reader = storage_lock.read();
            common::expand_on_chunk(
                chunk,
                &*reader,
                space_name,
                edge_types,
                direction,
                filter_expr,
                cancel_token,
            )
        } else {
            let mut new_cols: Vec<ColumnInfo> = chunk
                .schema
                .columns
                .iter()
                .map(|c| ColumnInfo {
                    name: c.name.clone(),
                    data_type: c.data_type.clone(),
                })
                .collect();
            new_cols.push(ColumnInfo {
                name: "_expand_edge_type".to_string(),
                data_type: "string".to_string(),
            });
            new_cols.push(ColumnInfo {
                name: "_expand_direction".to_string(),
                data_type: "string".to_string(),
            });
            let schema = Arc::new(Schema::new(new_cols));
            let mut rows = chunk.rows;
            for row in rows.iter_mut() {
                row.push(Value::String(edge_types.join("/")));
                row.push(Value::String(format!("{:?}", direction).to_lowercase()));
            }
            Ok(Some(DataChunk::new(rows, schema)))
        }
    } else {
        Ok(None)
    }
}
