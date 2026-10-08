use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Assign { assignments, state } = &mut op.kind else {
        return Err(QueryError::execution(
            "assign::handle called for a non-assign unary operator".to_string(),
        ));
    };
    loop {
        if let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("Assign");
            // 1:1 row-preserving rebuild: carry the input multiplicity.
            let multiplicity = chunk.multiplicity();
            // Batch-evaluate all assignment expressions first
            let mut new_cols: Vec<Vec<Value>> = Vec::with_capacity(assignments.len());
            for (_col_name, expr) in assignments.iter() {
                let col = match chunk.evaluate_expression(expr, Some(&state.env)) {
                    Ok(col) => col,
                    Err(_) => {
                        vec![Value::Null(linkrs_core::value::NullType::Null); chunk.len()]
                    }
                };
                new_cols.push(col);
            }
            // Extend each row with the computed values
            for (i, row) in chunk.rows.iter_mut().enumerate() {
                for col in &new_cols {
                    row.push(col[i].clone());
                }
            }
            if !chunk.rows.is_empty() {
                let mut rebuilt =
                    DataChunk::new_with_layout(chunk.rows, Arc::clone(&op.output_layout))
                        .with_multiplicity(multiplicity);
                // The rebuilt chunk starts row-major, so re-derive the
                // typed layout here; otherwise downstream operators
                // lose the columnar fast path at every assignment.
                rebuilt.build_typed_columns(true);
                return Ok(Some(rebuilt));
            }
        } else {
            return Ok(None);
        }
    }
}
