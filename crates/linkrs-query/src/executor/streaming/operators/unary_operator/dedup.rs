use super::*;

use crate::executor::streaming::chunk::TypedColumn;

/// Whether the chunk can be deduplicated through a single raw `i64` key.
/// The typed column and the row view must agree on every position; otherwise
/// the generic full-row set remains authoritative.
fn integer_keys(chunk: &mut DataChunk) -> Option<Vec<i64>> {
    if chunk.selection().is_some() {
        return None;
    }
    let rowless = chunk.rows.is_empty();
    let len = if rowless {
        chunk.typed_len()?
    } else {
        if chunk
            .rows
            .iter()
            .any(|row| row.len() != 1 || !matches!(row.first(), Some(Value::BigInt(_))))
        {
            return None;
        }
        chunk.rows.len()
    };
    chunk.ensure_typed_columns();
    let TypedColumn::I64(values) = chunk.typed_column(0)? else {
        return None;
    };
    if values.len() != len
        || (!rowless
            && chunk.rows.iter().zip(values.iter()).any(
                |(row, &key)| !matches!(row.first(), Some(Value::BigInt(value)) if *value == key),
            ))
    {
        return None;
    }
    Some(values.clone())
}

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Dedup { seen_rows } = &mut op.kind else {
        return Err(QueryError::execution(
            "dedup::handle called for a non-dedup unary operator".to_string(),
        ));
    };
    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("Dedup");
        let typed_keys = integer_keys(&mut chunk);
        let keys = match typed_keys {
            Some(values) => values
                .into_iter()
                .map(|key| (vec![Value::BigInt(key)], Some(key)))
                .collect::<Vec<_>>(),
            None => {
                chunk.materialize_typed_rows();
                chunk
                    .rows
                    .drain(..)
                    .map(|row| (row, None))
                    .collect::<Vec<_>>()
            }
        };
        let mut result_rows = Vec::new();
        for (row, key) in keys {
            let inserted = match key {
                Some(key) => seen_rows.insert_integer(key),
                None => seen_rows.insert_row(&row),
            };
            if inserted {
                result_rows.push(row);
            }
        }
        if !result_rows.is_empty() {
            // Row-collapsing: a new grouping is born, so the
            // rebuilt chunk keeps the default multiplicity of 1.
            return Ok(Some(DataChunk::new_with_layout(
                result_rows,
                Arc::clone(&op.output_layout),
            )));
        }
    }
    Ok(None)
}
