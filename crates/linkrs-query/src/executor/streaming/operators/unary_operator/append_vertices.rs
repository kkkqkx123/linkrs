use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::AppendVertices {
        entity_var: _,
        entity_expr,
        prop_names,
        tag,
        storage,
        space_name,
        state,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "append_vertices::handle called for a non-append-vertices unary operator".to_string(),
        ));
    };
    loop {
        if let Some(mut chunk) = input.advance()? {
            chunk.materialize_selection_by("AppendVertices");
            // 1:1 row-preserving rebuild: carry the input multiplicity.
            let multiplicity = chunk.multiplicity();
            let layout = chunk.get_layout();
            let storage_ref = storage.as_ref().ok_or_else(|| {
                QueryError::execution("AppendVertices requires storage".to_string())
            })?;
            let guard = storage_ref.read();
            let flat = !prop_names.is_empty();
            let mut result_rows = Vec::new();
            for row in chunk.rows {
                let mut new_row = row.clone();
                let mut ctx = if let Some(ref params) = state.env.params {
                    ValueRowContext::with_parameters(row.clone(), layout.clone(), params.clone())
                } else {
                    ValueRowContext::new(row.clone(), layout.clone())
                };
                let entity = match ExpressionEvaluator::evaluate(entity_expr, &mut ctx) {
                    Ok(val) => val,
                    Err(_) => Value::Null(linkrs_core::NullType::Null),
                };
                let vid = match linkrs_core::types::storage_ids::VertexId::try_from(&entity) {
                    Ok(vid) => vid,
                    Err(_) => {
                        new_row.push(Value::Null(linkrs_core::NullType::Null));
                        result_rows.push(new_row);
                        continue;
                    }
                };
                if tag.is_empty() {
                    return Err(QueryError::execution(
                        "AppendVertices requires a tag qualifier".to_string(),
                    ));
                }
                match guard.get_vertex_projected(space_name, tag, &vid, prop_names) {
                    Ok(Some(vertex)) => {
                        if flat {
                            for prop in prop_names.iter() {
                                new_row.push(
                                    vertex.property_value(prop).unwrap_or_else(|| {
                                        Value::Null(linkrs_core::NullType::Null)
                                    }),
                                );
                            }
                        } else {
                            new_row.push(Value::Vertex(Box::new(vertex)));
                        }
                    }
                    Ok(None) => {
                        if flat {
                            for _ in prop_names.iter() {
                                new_row.push(Value::Null(linkrs_core::NullType::Null));
                            }
                        } else {
                            new_row.push(Value::Null(linkrs_core::NullType::Null));
                        }
                    }
                    Err(_) => {
                        new_row.push(Value::Null(linkrs_core::NullType::Null));
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
