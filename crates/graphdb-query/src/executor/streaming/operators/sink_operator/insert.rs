use super::*;

pub(super) fn handle_insert_vertices(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::InsertVertices {
        storage,
        space_name,
        vertex_properties,
        tag,
        tag_property_names,
        if_not_exists,
        rows_inserted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "insert::handle_insert_vertices called for a non-insert-vertices sink".to_string(),
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
            let params = op.runtime.as_ref().and_then(|rt| rt.parameter_values());

            for row in &chunk.rows {
                let mut context =
                    row_context(row.clone(), layout.clone(), params.clone());

                let vid = if let Some((_name, expr)) = vertex_properties.first() {
                    let val = eval_expr(expr, &mut context)?;
                    VertexId::try_from(&val).map_err(|e| {
                        QueryError::execution(format!("Invalid vertex id: {}", e))
                    })?
                } else {
                    return Err(QueryError::execution(
                        "InsertVertices requires a vertex id expression".to_string(),
                    ));
                };

                if *if_not_exists
                    && writer
                        .get_vertex(space_name, tag, &vid)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                        .is_some()
                {
                    continue;
                }

                let mut props = HashMap::new();
                for name in tag_property_names.iter() {
                    if let Some((_n, expr)) =
                        vertex_properties.iter().find(|(n, _)| n == name)
                    {
                        if let Ok(val) = eval_expr(expr, &mut context) {
                            props.insert(name.clone(), val);
                        }
                    }
                }

                let vertex = Vertex::new(vid, Tag::new(tag.clone(), props));
                StorageWriter::insert_vertex(&mut *writer, space_name, vertex)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                *rows_inserted += 1;
            }
        } else {
            *rows_inserted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "insert_vertices",
        *rows_inserted,
    )))
}

pub(super) fn handle_insert_edges(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::InsertEdges {
        storage,
        space_name,
        src_col,
        dst_col,
        edge_type,
        edge_properties,
        if_not_exists,
        rows_inserted,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "insert::handle_insert_edges called for a non-insert-edges sink".to_string(),
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
            let params = op.runtime.as_ref().and_then(|rt| rt.parameter_values());

            for row in &chunk.rows {
                let mut context =
                    row_context(row.clone(), layout.clone(), params.clone());
                let src_val = context
                    .get_variable(src_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                let dst_val = context
                    .get_variable(dst_col)
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));

                let src = VertexId::try_from(&src_val).map_err(|e| {
                    QueryError::execution(format!("Invalid edge source id: {}", e))
                })?;
                let dst = VertexId::try_from(&dst_val).map_err(|e| {
                    QueryError::execution(format!("Invalid edge destination id: {}", e))
                })?;
                // Multi-edge semantics: the storage layer
                // assigns an increasing rank when a
                // (src, dst, edge_type) pair already exists,
                // so plain INSERT always succeeds. The
                // if-not-exists guard only skips duplicates.
                if *if_not_exists
                    && writer
                        .get_edge(space_name, &src, &dst, edge_type, 0)
                        .map_err(|e| QueryError::execution(e.to_string()))?
                        .is_some()
                {
                    continue;
                }
                let mut props = HashMap::new();
                for (prop_name, expr) in edge_properties.iter() {
                    let val = eval_expr(expr, &mut context)?;
                    props.insert(prop_name.clone(), val);
                }
                let edge = Edge::new(src, dst, edge_type.clone(), 0, props);
                StorageWriter::insert_edge(&mut *writer, space_name, edge)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                *rows_inserted += 1;
            }
        } else {
            *rows_inserted += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "insert_edges",
        *rows_inserted,
    )))
}
