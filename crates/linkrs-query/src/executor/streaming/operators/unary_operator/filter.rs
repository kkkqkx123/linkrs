use super::*;

/// Convert a Value to a boolean for filter predicate evaluation.
fn matches_value(val: &Value) -> bool {
    match val {
        Value::Bool(b) => *b,
        Value::Null(_) => false,
        Value::Int(i) => *i != 0,
        Value::BigInt(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Double(f) => *f != 0.0,
        Value::String(s) => !s.is_empty(),
        _ => true,
    }
}

impl UnaryOperator {
    /// Evaluate the filter predicate, preferring the compiled closure tree.
    ///
    /// The predicate is compiled once against the first chunk's layout and
    /// reused for all later chunks. When the compiled path is disabled
    /// (rollback switch) or compilation/evaluation fails, the scalar chunk
    /// path is used so semantics stay identical.
    pub(super) fn evaluate_filter_predicate(
        chunk: &mut DataChunk,
        predicate: &Expression,
        state: &mut UnaryOperatorState,
    ) -> Result<Vec<Value>, QueryError> {
        // Typed columnar fast path: a predicate over raw scalars (e.g.
        // `p.age > 30` on a typed column) is evaluated on the batch without
        // constructing one `Value` per row. Falls through when the chunk has
        // no typed layout or the predicate is outside the typed batch set.
        match chunk
            .try_evaluate_expressions_typed(std::slice::from_ref(predicate), Some(&state.env))
        {
            Ok(Some(mut cols)) => {
                chunk.count_typed_hit();
                if let Some(stats) = &chunk.columnar_stats {
                    stats.record_b_path_hit();
                }
                return Ok(cols.pop().unwrap_or_default());
            }
            Ok(None) => {}
            Err(_) => {
                // Follow the compiled path's error policy: defer to the
                // scalar chunk path so the runtime error text stays identical.
            }
        }
        if compiled_eval_enabled() {
            if state.compiled_predicate.is_none() {
                let layout = chunk.get_layout();
                state.compiled_predicate = Some(CompiledExpr::compile(predicate, &layout));
            }
            if let Some(compiled) = &state.compiled_predicate {
                let layout = chunk.get_layout();
                let len = chunk.rows.len();
                match compiled.evaluate_batch(
                    &chunk.rows,
                    layout,
                    Some(&state.env),
                    chunk.typed_columns.as_deref(),
                ) {
                    Ok(col) => return Ok(col.into_values(len)),
                    Err(_) => {
                        // Compiled evaluation failed; fall back to the scalar
                        // path so the runtime error text stays identical.
                    }
                }
            }
        }
        chunk
            .evaluate_expression(predicate, Some(&state.env))
            .map_err(|e| {
                QueryError::execution(format!("Filter predicate evaluation failed: {}", e))
            })
    }
}

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Filter { predicate, state } = &mut op.kind else {
        return Err(QueryError::execution(
            "filter::handle called for a non-filter unary operator".to_string(),
        ));
    };
    loop {
        match input.advance()? {
            Some(mut chunk) => {
                let results =
                    UnaryOperator::evaluate_filter_predicate(&mut chunk, predicate, state)?;
                // Build a selection vector restricted to the
                // currently-visible rows (a nested filter keeps the
                // absolute row indices).
                let mut selected = Vec::new();
                match chunk.selection() {
                    None => {
                        for (i, val) in results.iter().enumerate() {
                            if matches_value(val) {
                                selected.push(i);
                            }
                        }
                    }
                    Some(sel) => {
                        for &i in sel {
                            if matches_value(&results[i]) {
                                selected.push(i);
                            }
                        }
                    }
                }
                if selected.is_empty() {
                    continue;
                }
                // All visible rows selected — hand the chunk through
                // as-is, keeping any existing selection. Rowless chunks
                // report no stored rows, so count the typed positions the
                // predicate actually evaluated.
                let total_visible = match (chunk.selection(), chunk.rows.is_empty()) {
                    (Some(sel), _) => sel.len(),
                    (None, false) => chunk.rows.len(),
                    (None, true) => chunk.typed_len().unwrap_or(0),
                };
                if selected.len() == total_visible {
                    return Ok(Some(chunk));
                }
                // Attach the selection vector instead of moving rows;
                // the columnar/typed caches stay valid for the downstream
                // selection-aware consumers.
                if let Some(stats) = &chunk.columnar_stats {
                    stats.record_selection_attached();
                }
                let selected_chunk = chunk.with_selection(selected);
                return Ok(Some(selected_chunk));
            }
            None => return Ok(None),
        }
    }
}
