use super::*;

pub(super) fn handle_delete_vertices(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::DeleteVertices {
        storage,
        space_name,
        tag,
        vertex_id_col,
        cascade,
        rows_deleted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "delete::handle_delete_vertices called for a non-delete-vertices sink".to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("Sink");
        if let Some(storage_lock) = storage {
            let mut writer = storage_lock.write();
            let layout = chunk.get_layout();

            for row in &chunk.rows {
                let context = ValueRowContext::new(row.clone(), layout.clone());
                if let Some(vid_val) = context.get_variable(vertex_id_col) {
                    if matches!(vid_val, Value::Null(_)) {
                        continue;
                    }
                    let vid = VertexId::try_from(&vid_val)
                        .map_err(|e| QueryError::execution(format!("Invalid vertex id: {}", e)))?;
                    {
                        if *cascade {
                            StorageWriter::delete_vertex_with_edges(
                                &mut *writer,
                                space_name,
                                tag,
                                &vid,
                            )
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        } else {
                            StorageWriter::delete_vertex(&mut *writer, space_name, tag, &vid)
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                        }
                        *rows_deleted += 1;
                    }
                }
            }
        } else {
            *rows_deleted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "delete_vertices",
        *rows_deleted,
    )))
}

pub(super) fn handle_delete_edges(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::DeleteEdges {
        storage,
        space_name,
        src_col,
        dst_col,
        edge_type,
        rows_deleted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "delete::handle_delete_edges called for a non-delete-edges sink".to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("Sink");
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if let Some(storage_lock) = storage {
            let mut writer = storage_lock.write();
            let layout = chunk.get_layout();

            for row in &chunk.rows {
                let context = ValueRowContext::new(row.clone(), layout.clone());
                let src_val = context
                    .get_variable(src_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                let dst_val = context
                    .get_variable(dst_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                if matches!(src_val, Value::Null(_)) || matches!(dst_val, Value::Null(_)) {
                    continue;
                }
                let (src, dst) = resolve_edge_endpoints(&src_val, &dst_val)
                    .ok_or_else(|| QueryError::execution("Invalid edge endpoint id".to_string()))?;
                {
                    StorageWriter::delete_edge(&mut *writer, space_name, &src, &dst, edge_type, 0)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    *rows_deleted += 1;
                }
            }
        } else {
            *rows_deleted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "delete_edges",
        *rows_deleted,
    )))
}

pub(super) fn handle_pipe_delete_edges(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::PipeDeleteEdges {
        storage,
        space_name,
        src_col,
        dst_col,
        edge_type,
        rows_deleted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "delete::handle_pipe_delete_edges called for a non-pipe-delete-edges sink".to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("Sink");
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if let Some(storage_lock) = storage {
            let mut writer = storage_lock.write();
            let layout = chunk.get_layout();

            for row in &chunk.rows {
                let context = ValueRowContext::new(row.clone(), layout.clone());
                let src_val = context
                    .get_variable(src_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                let dst_val = context
                    .get_variable(dst_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                if matches!(src_val, Value::Null(_)) || matches!(dst_val, Value::Null(_)) {
                    continue;
                }
                let (src, dst) = resolve_edge_endpoints(&src_val, &dst_val)
                    .ok_or_else(|| QueryError::execution("Invalid edge endpoint id".to_string()))?;
                {
                    StorageWriter::delete_edge(&mut *writer, space_name, &src, &dst, edge_type, 0)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    *rows_deleted += 1;
                }
            }
        } else {
            *rows_deleted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "delete_edges",
        *rows_deleted,
    )))
}

pub(super) fn handle_pipe_delete_vertices(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::PipeDeleteVertices {
        storage,
        space_name,
        vertex_id_col,
        cascade,
        rows_deleted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "delete::handle_pipe_delete_vertices called for a non-pipe-delete-vertices sink"
                .to_string(),
        ));
    };
    if *summary_returned {
        return Ok(None);
    }

    while let Some(mut chunk) = input.advance()? {
        chunk.normalize_for_opaque("Sink");
        if let Some(rt) = op.runtime.as_ref() {
            rt.ensure_not_cancelled()?;
        }
        if let Some(storage_lock) = storage {
            let mut writer = storage_lock.write();
            let layout = chunk.get_layout();

            for row in &chunk.rows {
                let context = ValueRowContext::new(row.clone(), layout.clone());
                if let Some(vid_val) = context.get_variable(vertex_id_col) {
                    if matches!(vid_val, Value::Null(_)) {
                        continue;
                    }
                    let (tag, vid) = match &vid_val {
                        Value::Vertex(vertex) => (vertex.tag.name.clone(), vertex.vid),
                        _ => {
                            return Err(QueryError::execution(
                                "Pipe DELETE VERTEX requires a vertex value with tag; bare id is illegal".to_string(),
                            ));
                        }
                    };
                    {
                        if *cascade {
                            StorageWriter::delete_vertex_with_edges(
                                &mut *writer,
                                space_name,
                                &tag,
                                &vid,
                            )
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                        } else {
                            StorageWriter::delete_vertex(&mut *writer, space_name, &tag, &vid)
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                        }
                        *rows_deleted += 1;
                    }
                }
            }
        } else {
            *rows_deleted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "pipe_delete_vertices",
        *rows_deleted,
    )))
}
