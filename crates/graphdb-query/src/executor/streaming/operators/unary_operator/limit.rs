use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Limit {
        offset,
        limit,
        skipped,
        consumed,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "limit::handle called for a non-limit unary operator".to_string(),
        ));
    };
    if *consumed >= *limit {
        return Ok(None);
    }

    loop {
        let Some(mut chunk) = input.advance()? else {
            return Ok(None);
        };
        // consume offset/limit directly on the visible rows.
        // The selection vector (if any) is trimmed in place and
        // handed downstream, so the chunk stays compact across
        // the boundary instead of materializing.
        let mut vis = chunk.visible_indices();
        let remain_offset = (*offset).saturating_sub(*skipped) as usize;
        *skipped += vis.len().min(remain_offset) as u32;
        if remain_offset < vis.len() {
            vis.drain(..remain_offset);
        } else {
            vis.clear();
        }
        if vis.is_empty() {
            continue;
        }
        let remaining_limit = (*limit - *consumed) as usize;
        if vis.len() > remaining_limit {
            vis.truncate(remaining_limit);
        }
        *consumed += vis.len() as u32;
        if vis.len() == chunk.rows.len() {
            // Every row is visible again — drop the redundant
            // selection so the invariant stays tight.
            let _ = chunk.take_selection();
            return Ok(Some(chunk));
        }
        return Ok(Some(chunk.with_selection(vis)));
    }
}
