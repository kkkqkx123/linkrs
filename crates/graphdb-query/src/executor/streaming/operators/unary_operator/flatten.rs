use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Flatten {
        // Plan level group identifier, carried end to end
        // (plan -> spec -> operator) and visible in EXPLAIN /
        // cbo_notes. The streaming engine stores flat row batches
        // only, so flatten replays the child selection; the
        // `group_columns`/`expected_groups` carried alongside are
        // EXPLAIN traceability; the stale-position check runs in
        // `open()` so a wrong group never replays silently.
        group_pos,
        group_columns: _,
        expected_groups: _,
        current_idx,
        size_to_flatten,
        saved_sel_vector,
        buffered_chunk,
        batch_size,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "flatten::handle called for a non-flatten unary operator".to_string(),
        ));
    };
    log::debug!("flatten: replaying selection for group {group_pos}");
    if *batch_size <= 1 {
        crate::executor::streaming::operators::flatten::flatten_next_inner(
            current_idx,
            size_to_flatten,
            saved_sel_vector,
            buffered_chunk,
            input,
        )
    } else {
        crate::executor::streaming::operators::flatten::flatten_next_batch(
            current_idx,
            size_to_flatten,
            saved_sel_vector,
            buffered_chunk,
            input,
            *batch_size,
        )
    }
}
