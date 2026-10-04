use super::*;

/// Whether a projection expression qualifies for the columnar fast path:
/// a bare column passthrough or a constant (gather / broadcast upstream,
/// no per-row expression work, NULL/selection/multiplicity preserved).
pub(super) fn is_passthrough_or_const(expression: &Expression) -> bool {
    match expression {
        Expression::Variable(_) | Expression::Literal(_) => true,
        Expression::Property { object, .. } => {
            matches!(object.as_ref(), Expression::Variable(_))
        }
        _ => false,
    }
}

impl UnaryOperator {
    /// Evaluate the project output expressions, preferring the compiled
    /// closure tree over the scalar chunk path.
    ///
    /// Columnar fast path: when every output is a passthrough (`Variable` /
    /// `Property` over a variable) or a `Literal`, evaluation goes straight
    /// through the chunk's column gather/broadcast helpers without building
    /// per-expression compiled closures. Anything else keeps the existing
    /// compiled-batch path.
    pub(super) fn evaluate_project_expressions(
        chunk: &mut DataChunk,
        output_expressions: &[Expression],
        state: &mut UnaryOperatorState,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        if output_expressions.iter().all(is_passthrough_or_const) {
            return chunk
                .evaluate_expressions(output_expressions, Some(&state.env))
                .map_err(|e| {
                    QueryError::execution(format!("Project expression evaluation failed: {}", e))
                });
        }
        // Typed columnar fast path: expressions over raw scalars (bare
        // columns, arithmetic/cast on typed columns) are evaluated on the
        // batch. Falls through to the compiled path when the chunk has no
        // typed layout or any expression is outside the typed batch set.
        match chunk.try_evaluate_expressions_typed(output_expressions, Some(&state.env)) {
            Ok(Some(cols)) => {
                for _ in 0..output_expressions.len() {
                    chunk.count_typed_hit();
                    if let Some(stats) = &chunk.columnar_stats {
                        stats.record_b_path_hit();
                    }
                }
                return Ok(cols);
            }
            Ok(None) => {}
            Err(_) => {}
        }
        if compiled_eval_enabled() {
            if state.compiled_project.is_none() {
                let layout = chunk.get_layout();
                state.compiled_project = Some(
                    output_expressions
                        .iter()
                        .map(|e| CompiledExpr::compile(e, &layout))
                        .collect(),
                );
            }
            if let Some(compiled) = &state.compiled_project {
                let layout = chunk.get_layout();
                let len = chunk.rows.len();
                let mut columns = Vec::with_capacity(compiled.len());
                let mut ok = true;
                for expr in compiled {
                    match expr.evaluate_batch(
                        &chunk.rows,
                        layout.clone(),
                        Some(&state.env),
                        chunk.typed_columns.as_deref(),
                    ) {
                        Ok(col) => columns.push(col.into_values(len)),
                        Err(_) => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok {
                    return Ok(columns);
                }
            }
        }
        chunk
            .evaluate_expressions(output_expressions, Some(&state.env))
            .map_err(|e| {
                QueryError::execution(format!("Project expression evaluation failed: {}", e))
            })
    }
}

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let UnaryOperatorKind::Project {
        output_expressions,
        output_col_names: _,
        state,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "project::handle called for a non-project unary operator".to_string(),
        ));
    };
    loop {
        if let Some(mut chunk) = input.advance()? {
            // When the child carries a selection vector, evaluate
            // each output expression only for the visible rows — the
            // output chunk is fully materialized (small).
            if chunk.selection().is_some() {
                let mut columns = Vec::with_capacity(output_expressions.len());
                for expr in output_expressions.iter() {
                    let col = chunk
                        .evaluate_expression_visible(expr, Some(&state.env))
                        .map_err(|e| {
                            QueryError::execution(format!(
                                "Project expression evaluation failed: {}",
                                e
                            ))
                        })?;
                    columns.push(col);
                }
                if !columns.is_empty() && !columns[0].is_empty() {
                    // 1:1 row-preserving rebuild: carry the input
                    // multiplicity (each output row occurs that often).
                    let multiplicity = chunk.multiplicity();
                    return Ok(Some(
                        DataChunk::project_columns(columns, Arc::clone(&op.output_layout))
                            .with_multiplicity(multiplicity),
                    ));
                }
                continue;
            }
            let columns =
                UnaryOperator::evaluate_project_expressions(&mut chunk, output_expressions, state)?;
            if !columns.is_empty() && !columns[0].is_empty() {
                let multiplicity = chunk.multiplicity();
                return Ok(Some(
                    DataChunk::project_columns(columns, Arc::clone(&op.output_layout))
                        .with_multiplicity(multiplicity),
                ));
            }
        } else {
            return Ok(None);
        }
    }
}
