use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Remove { columns_to_remove } = &mut op.kind else {
        return Err(QueryError::execution(
            "remove::handle called for a non-remove unary operator".to_string(),
        ));
    };
    loop {
        if let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("Remove");
            // 1:1 row-preserving rebuild: carry the input multiplicity.
            let multiplicity = chunk.multiplicity();
            let col_names = chunk.col_names();
            let mut indices_to_keep = vec![];
            for (idx, col_name) in col_names.iter().enumerate() {
                if !columns_to_remove.contains(col_name) {
                    indices_to_keep.push(idx);
                }
            }
            let mut result_rows = vec![];
            for row in chunk.rows {
                let mut new_row = vec![];
                for idx in &indices_to_keep {
                    if *idx < row.len() {
                        new_row.push(row[*idx].clone());
                    }
                }
                result_rows.push(new_row);
            }
            if !result_rows.is_empty() {
                return Ok(Some(
                    DataChunk::new_with_layout(result_rows, Arc::clone(&op.output_layout))
                        .with_multiplicity(multiplicity),
                ));
            }
        } else {
            return Ok(None);
        }
    }
}
