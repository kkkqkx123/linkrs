use super::*;

pub(super) fn handle_copy_from(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::CopyFrom {
        storage,
        space_name,
        target,
        file_paths,
        by_column,
        header,
        delimiter,
        batch_size,
        rows_inserted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "copy::handle_copy_from called for a non-copy-from sink".to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }
    // Drain input (dummy single row)
    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("CopyFrom");
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
    }
    if let Some(rt) = op.runtime.as_ref() {
        rt.ensure_not_cancelled()?;
    }
    if let Some(storage_lock) = storage {
        let count = super::super::copy::execute_copy_from(
            storage_lock,
            space_name,
            target,
            file_paths,
            *by_column,
            *header,
            *delimiter,
            *batch_size,
            op.runtime.as_ref().map(|r| r.clone()),
        )?;
        *rows_inserted = count;
    } else {
        // Mock storage: estimate from file line count if possible
        *rows_inserted = 0;
    }
    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "copy_from",
        *rows_inserted,
    )))
}

pub(super) fn handle_copy_to(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::CopyTo {
        storage,
        space_name,
        target,
        file_path,
        header,
        delimiter,
        rows_exported,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "copy::handle_copy_to called for a non-copy-to sink".to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }
    // Drain input (dummy single row)
    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("CopyTo");
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
    }
    if let Some(rt) = op.runtime.as_ref() {
        rt.ensure_not_cancelled()?;
    }
    if let Some(storage_lock) = storage {
        let count = super::super::copy::execute_copy_to(
            storage_lock,
            space_name,
            target,
            file_path,
            *header,
            *delimiter,
        )?;
        *rows_exported = count;
    } else {
        // Mock storage: nothing to scan.
        *rows_exported = 0;
    }
    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "copy_to",
        *rows_exported,
    )))
}
