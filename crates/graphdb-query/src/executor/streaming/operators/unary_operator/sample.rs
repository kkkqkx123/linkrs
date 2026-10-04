use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Sample { count, consumed } = &mut op.kind else {
        return Err(QueryError::execution(
            "sample::handle called for a non-sample unary operator".to_string(),
        ));
    };
    if *consumed >= *count {
        return Ok(None);
    }
    loop {
        match input.advance()? {
            Some(mut chunk) => {
                // boundary materialization.
                chunk.materialize_selection_by("Sample");
                // 1:1 row-preserving rebuild: carry the input multiplicity.
                let multiplicity = chunk.multiplicity();
                let remaining = (*count - *consumed) as usize;
                let take_count = chunk.rows.len().min(remaining);
                let rows: Vec<Vec<Value>> = chunk.rows.into_iter().take(take_count).collect();
                *consumed += take_count as u64;
                if !rows.is_empty() {
                    return Ok(Some(
                        DataChunk::new_with_layout(rows, Arc::clone(&op.output_layout))
                            .with_multiplicity(multiplicity),
                    ));
                } else {
                    continue;
                }
            }
            None => return Ok(None),
        }
    }
}
