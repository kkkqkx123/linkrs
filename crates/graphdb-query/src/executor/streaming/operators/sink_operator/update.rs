use super::*;

pub(super) fn handle_update_vertices(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::UpdateVertices {
        storage,
        space_name,
        tag_name,
        updates,
        condition,
        is_upsert,
        replace_properties,
        rows_updated,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "update::handle_update_vertices called for a non-update-vertices sink".to_string(),
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
                let mut context = row_context(row.clone(), layout.clone(), params.clone());
                let vid_val = context
                    .get_variable("vid")
                    .or_else(|| row.first().cloned())
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                if matches!(vid_val, Value::Null(_)) {
                    continue;
                }
                let vid = VertexId::try_from(&vid_val)
                    .map_err(|e| QueryError::execution(format!("Invalid vertex id: {}", e)))?;
                if tag_name.is_empty() {
                    return Err(QueryError::execution(
                        "UPDATE vertex requires a tag qualifier".to_string(),
                    ));
                }
                let existing = writer
                    .get_vertex(space_name, tag_name, &vid)
                    .map_err(|e| QueryError::execution(e.to_string()))?;
                let existing = match existing {
                    Some(ev) => ev,
                    None => {
                        if *is_upsert {
                            let props =
                                eval_update_props(updates, *replace_properties, &mut context)?;
                            let vertex = Vertex::new(vid, Tag::new(tag_name.clone(), props));
                            StorageWriter::insert_vertex(&mut *writer, space_name, vertex)
                                .map_err(|e| QueryError::execution(e.to_string()))?;
                            *rows_updated += 1;
                        } else {
                            return Err(QueryError::execution(format!(
                                "Vertex not found: {}",
                                vid
                            )));
                        }
                        continue;
                    }
                };
                // Load existing properties into context so expressions
                // like `SET stock = stock - 1` and conditions like
                // `WHEN age > 100` can resolve existing columns.
                for (k, v) in &existing.tag.properties {
                    context.set_variable(k.to_string(), v.clone());
                }
                if let Some(cond) = condition {
                    let keep = eval_expr(cond, &mut context)?;
                    if !condition_matches(&keep) {
                        continue;
                    }
                }
                let props = eval_update_props(updates, *replace_properties, &mut context)?;
                let tag = if *replace_properties {
                    Tag::new(existing.tag.name.clone(), props)
                } else if *tag_name == existing.tag.name {
                    let mut merged = existing.tag.properties.clone();
                    for (k, v) in &props {
                        merged.insert(Arc::from(k.clone()), v.clone());
                    }
                    Tag::new(existing.tag.name.clone(), merged)
                } else {
                    Tag::new(tag_name.clone(), props)
                };
                let vertex = Vertex::new(vid, tag);
                if *replace_properties {
                    StorageWriter::update_vertex_replace(&mut *writer, space_name, vertex)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                } else {
                    StorageWriter::update_vertex(&mut *writer, space_name, vertex)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                }
                *rows_updated += 1;
            }
        } else {
            *rows_updated += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "update_vertices",
        *rows_updated,
    )))
}

pub(super) fn handle_update_edges(
    op: &mut SinkOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let SinkOperatorKind::UpdateEdges {
        storage,
        space_name,
        src_col,
        dst_col,
        edge_type,
        updates,
        condition,
        is_upsert,
        replace_properties,
        rows_updated,
        summary_returned,
        ..
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "update::handle_update_edges called for a non-update-edges sink".to_string(),
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
                let mut context = row_context(row.clone(), layout.clone(), params.clone());
                let src_val = context
                    .get_variable(src_col)
                    .or_else(|| row.first().cloned())
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));
                let dst_val = context
                    .get_variable(dst_col)
                    .or_else(|| row.get(1).cloned())
                    .unwrap_or(Value::Null(graphdb_core::NullType::Null));

                if matches!(src_val, Value::Null(_)) || matches!(dst_val, Value::Null(_)) {
                    continue;
                }
                let src = VertexId::try_from(&src_val)
                    .map_err(|e| QueryError::execution(format!("Invalid edge source id: {}", e)))?;
                let dst = VertexId::try_from(&dst_val).map_err(|e| {
                    QueryError::execution(format!("Invalid edge destination id: {}", e))
                })?;
                {
                    let existing = writer
                        .get_edge(space_name, &src, &dst, edge_type, 0)
                        .map_err(|e| QueryError::execution(e.to_string()))?;
                    let existing = match existing {
                        Some(edge) => edge,
                        None => {
                            if *is_upsert {
                                let props =
                                    eval_update_props(updates, *replace_properties, &mut context)?;
                                let edge = Edge::new(src, dst, edge_type.clone(), 0, props);
                                StorageWriter::insert_edge(&mut *writer, space_name, edge)
                                    .map_err(|e| QueryError::execution(e.to_string()))?;
                                *rows_updated += 1;
                            } else {
                                return Err(QueryError::execution(format!(
                                    "Edge not found: {} -> {} of {}",
                                    src, dst, edge_type
                                )));
                            }
                            continue;
                        }
                    };
                    for (k, v) in &existing.props {
                        context.set_variable(k.to_string(), v.clone());
                    }
                    if let Some(cond) = condition {
                        let keep = eval_expr(cond, &mut context)?;
                        if !condition_matches(&keep) {
                            continue;
                        }
                    }
                    let props = eval_update_props(updates, *replace_properties, &mut context)?;
                    let mut edge = Edge::new_empty(src, dst, edge_type.clone(), 0);
                    edge.props = props;
                    if *replace_properties {
                        StorageWriter::update_edge_replace(&mut *writer, space_name, edge)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                    } else {
                        StorageWriter::update_edge(&mut *writer, space_name, edge)
                            .map_err(|e| QueryError::execution(e.to_string()))?;
                    }
                    *rows_updated += 1;
                }
            }
        } else {
            *rows_updated += chunk.rows.len() as u64;
        }
    }

    *summary_returned = true;
    Ok(Some(make_modify_result(
        Arc::clone(&op.output_layout),
        "update_edges",
        *rows_updated,
    )))
}
