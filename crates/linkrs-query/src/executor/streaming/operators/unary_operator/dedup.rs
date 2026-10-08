use super::*;

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
        let mut result_rows = vec![];
        for row in chunk.rows {
            if seen_rows.insert(row.clone()) {
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
