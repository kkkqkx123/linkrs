use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Unwind {
        unwind_column,
        list_expression,
        col_index,
        layout,
        all_rows,
        input_multiplicity,
        current_row_index,
        current_unwind_index,
        input_done,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "unwind::handle called for a non-unwind unary operator".to_string(),
        ));
    };
    loop {
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if *current_row_index >= all_rows.len() && !*input_done {
            match input.advance()? {
                Some(mut chunk) => {
                    // boundary materialization.
                    chunk.materialize_selection_by("Unwind");
                    let col_names = chunk.col_names();
                    *col_index = col_names.iter().position(|c| c == unwind_column.as_str());
                    *layout = Some(chunk.get_layout());
                    *input_multiplicity = chunk.multiplicity();
                    *all_rows = chunk.rows;
                    *current_row_index = 0;
                    *current_unwind_index = 0;
                }
                None => {
                    *input_done = true;
                    if all_rows.is_empty() && list_expression.is_some() {
                        *all_rows = vec![Vec::new()];
                        *current_row_index = 0;
                        *current_unwind_index = 0;
                    } else {
                        return Ok(None);
                    }
                }
            }
            continue;
        }
        if *current_row_index >= all_rows.len() {
            return Ok(None);
        }
        let row = &all_rows[*current_row_index];
        let list_val: Option<Value> = if let Some(expr) = list_expression {
            let row_layout = match layout {
                Some(l) => l.clone(),
                None => Arc::new(SlotLayout::new(vec![])),
            };
            let mut ctx = ValueRowContext::new(row.clone(), row_layout);
            ExpressionEvaluator::evaluate(expr, &mut ctx).ok()
        } else {
            col_index.and_then(|idx| row.get(idx).cloned())
        };
        let result_row = match list_val {
            Some(Value::List(items)) if *current_unwind_index < items.len() => {
                let mut result_row = row.clone();
                result_row.push(items[*current_unwind_index].clone());
                *current_unwind_index += 1;
                Some(result_row)
            }
            Some(Value::List(items)) if items.is_empty() => None,
            _ => None,
        };
        if let Some(result_row) = result_row {
            return Ok(Some(
                DataChunk::new_with_layout(vec![result_row], Arc::clone(&op.output_layout))
                    .with_multiplicity(*input_multiplicity),
            ));
        }
        *current_row_index += 1;
        *current_unwind_index = 0;
    }
}
