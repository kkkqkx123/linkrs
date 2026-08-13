use std::sync::Arc;

use crate::core::error::QueryError;
use crate::core::{NullType, Value};
use crate::query::executor::base::{MemoryBudget, MemoryTracker};
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::instance::QueryBindings;
use crate::query::executor::streaming::join_helpers::evaluate_join_key;
use crate::query::executor::streaming::operators::base::OperatorBase;
use crate::query::executor::streaming::operators::spec::{ApplyKind, ApplySpec};
use crate::query::executor::streaming::plan::materializer::PhysicalPlanMaterializer;
use crate::query::executor::streaming::plan::types::PhysicalPlan;
use crate::query::executor::streaming::slot::SlotLayout;

#[derive(Debug)]
pub enum ApplyOperator {
    Apply {
        kind: ApplyKind,
        correlated_columns: Vec<String>,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
    PatternApply {
        hash_keys: Vec<crate::core::types::expr::Expression>,
        probe_keys: Vec<crate::core::types::expr::Expression>,
        anti: bool,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
    CorrelatedApply {
        /// Self-contained right subtree (rooted at an `Argument` source),
        /// re-executed per outer row with the outer row bound as the
        /// correlation frame.
        sub_plan: Arc<PhysicalPlan>,
        /// Bindings with the parameter maps stripped, cached for the per-row
        /// nested materialization.
        bindings: Box<QueryBindings>,
        anti: bool,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
    RollUpApply {
        compare_columns: Vec<String>,
        collect_column: Option<String>,
        right_rows: Option<Vec<Vec<Value>>>,
        right_layout: Option<Arc<SlotLayout>>,
        memory_tracker: MemoryTracker,
    },
}

impl ApplyOperator {
    pub fn from_spec(spec: &ApplySpec, budget: &MemoryBudget) -> Self {
        match spec {
            ApplySpec::Apply {
                kind,
                correlated_columns,
            } => Self::Apply {
                kind: *kind,
                correlated_columns: correlated_columns.clone(),
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            ApplySpec::PatternApply {
                hash_keys,
                probe_keys,
                anti,
            } => Self::PatternApply {
                hash_keys: hash_keys.clone(),
                probe_keys: probe_keys.clone(),
                anti: *anti,
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
            ApplySpec::CorrelatedApply { .. } => {
                // `CorrelatedApply` carries a nested physical plan and the
                // stripped bindings, neither available from a spec alone; the
                // materializer constructs it via the literal variant.
                unreachable!("CorrelatedApply is constructed by the materializer")
            }
            ApplySpec::RollUpApply {
                compare_columns,
                collect_column,
            } => Self::RollUpApply {
                compare_columns: compare_columns.clone(),
                collect_column: collect_column.clone(),
                right_rows: None,
                right_layout: None,
                memory_tracker: MemoryTracker::new(budget.clone()),
            },
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        match self {
            Self::Apply { memory_tracker, .. }
            | Self::PatternApply { memory_tracker, .. }
            | Self::CorrelatedApply { memory_tracker, .. }
            | Self::RollUpApply { memory_tracker, .. } => memory_tracker,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        left.open()?;
        // CorrelatedApply keeps only the left input; the right placeholder
        // (`SourceOperator::Start`) is never executed because the nested right
        // subtree is re-materialized per outer row at runtime.
        if !matches!(self, Self::CorrelatedApply { .. }) {
            right.open()?;
        }
        base.lifecycle.mark_opened();
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        left: &mut StreamingExecutor,
        right: &mut StreamingExecutor,
    ) -> Result<Option<DataChunk>, QueryError> {
        match self {
            Self::Apply {
                kind,
                correlated_columns,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let output_layout = Arc::clone(&base.output_layout);
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::new(Vec::new())));
                loop {
                    let Some(mut left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    left_chunk.materialize_selection_by("Apply");
                    let mut output = Vec::new();
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        let matches = matching_rows(
                            &left_row,
                            &left_chunk.layout,
                            right_rows,
                            &right_layout,
                            correlated_columns,
                        )?;
                        match kind {
                            ApplyKind::Standard => {
                                for right_row in matches {
                                    let mut row = left_row.clone();
                                    row.extend_from_slice(right_row);
                                    output.push(row);
                                }
                            }
                            ApplyKind::Semi if !matches.is_empty() => output.push(left_row),
                            ApplyKind::Anti if matches.is_empty() => output.push(left_row),
                            ApplyKind::Single => match matches.as_slice() {
                                [] => {
                                    let mut row = left_row;
                                    row.extend(std::iter::repeat_n(
                                        Value::Null(NullType::Null),
                                        right_layout.len(),
                                    ));
                                    output.push(row);
                                }
                                [right_row] => {
                                    let mut row = left_row;
                                    row.extend_from_slice(right_row);
                                    output.push(row);
                                }
                                _ => {
                                    return Err(QueryError::execution(
                                        "Single Apply produced more than one matching row"
                                            .to_string(),
                                    ));
                                }
                            },
                            ApplyKind::All if matches.len() == right_rows.len() => {
                                output.push(left_row);
                            }
                            _ => {}
                        }
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            output,
                            Arc::clone(&output_layout),
                        )));
                    }
                }
            }
            Self::PatternApply {
                hash_keys,
                probe_keys,
                anti,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let output_layout = Arc::clone(&base.output_layout);
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::new(Vec::new())));
                loop {
                    let Some(mut left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    left_chunk.materialize_selection_by("Apply");
                    let mut output = Vec::new();
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        let left_key =
                            evaluate_join_key(&left_row, left_chunk.layout.clone(), hash_keys)?;
                        let mut exists = false;
                        for right_row in right_rows {
                            let right_key =
                                evaluate_join_key(right_row, right_layout.clone(), probe_keys)?;
                            if keys_match(&left_key, &right_key) {
                                exists = true;
                                break;
                            }
                        }
                        if exists != *anti {
                            output.push(left_row);
                        }
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            output,
                            Arc::clone(&output_layout),
                        )));
                    }
                }
            }
            Self::CorrelatedApply {
                sub_plan,
                bindings,
                anti,
                ..
            } => {
                let rt = base.runtime.clone().ok_or_else(|| {
                    QueryError::execution(
                        "CorrelatedApply requires an execution runtime".to_string(),
                    )
                })?;
                let output_layout = Arc::clone(&base.output_layout);
                loop {
                    let Some(mut left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    left_chunk.materialize_selection_by("CorrelatedApply");
                    let mut output = Vec::new();
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        // Bind this outer row as the correlation frame consumed
                        // by the Argument source at the root of the right
                        // subtree. The right subtree has a single Argument
                        // consumer, so overwriting the previous frame is safe
                        // (the frame is `Mutex::take`n on the first read).
                        rt.set_correlation_frame(left_chunk.layout.clone(), left_row.clone());
                        // Re-materialize the self-contained right subtree fresh
                        // for this row, sharing the parent runtime so storage,
                        // parameter values, and the correlation frame resolve
                        // against the same execution context.
                        let (mut sub_exec, _) =
                            PhysicalPlanMaterializer::materialize(sub_plan, bindings)?;
                        sub_exec.set_chunk_size(bindings.chunk_size);
                        sub_exec.set_runtime(Some(rt.clone()));
                        sub_exec.open()?;
                        let mut exists = false;
                        while let Some(mut sub_chunk) = sub_exec.advance()? {
                            sub_chunk.materialize_selection_by("CorrelatedApply");
                            if !sub_chunk.rows.is_empty() {
                                exists = true;
                                break;
                            }
                        }
                        sub_exec.close()?;
                        if exists != *anti {
                            output.push(left_row);
                        }
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            output,
                            Arc::clone(&output_layout),
                        )));
                    }
                }
            }
            Self::RollUpApply {
                compare_columns,
                collect_column,
                right_rows,
                right_layout,
                memory_tracker,
            } => {
                materialize_right(base, right, right_rows, right_layout, memory_tracker)?;
                let output_layout = Arc::clone(&base.output_layout);
                let right_rows = right_rows.as_deref().unwrap_or_default();
                let right_layout = right_layout
                    .as_ref()
                    .cloned()
                    .unwrap_or_else(|| Arc::new(SlotLayout::new(Vec::new())));
                let collect_slot = collect_column
                    .as_deref()
                    .map(|column| {
                        right_layout.resolve(column).ok_or_else(|| {
                            QueryError::execution(format!(
                                "RollUpApply collect column not found: {column}"
                            ))
                        })
                    })
                    .transpose()?;
                loop {
                    let Some(mut left_chunk) = left.advance()? else {
                        return Ok(None);
                    };
                    left_chunk.materialize_selection_by("Apply");
                    let mut output = Vec::with_capacity(left_chunk.rows.len());
                    for left_row in left_chunk.rows {
                        base.ensure_not_cancelled()?;
                        let matches = matching_rows(
                            &left_row,
                            &left_chunk.layout,
                            right_rows,
                            &right_layout,
                            compare_columns,
                        )?;
                        let values = matches
                            .into_iter()
                            .map(|row| {
                                collect_slot
                                    .and_then(|slot| row.get(slot).cloned())
                                    .unwrap_or_else(|| {
                                        Value::List(Box::new(crate::core::value::List {
                                            values: row.clone(),
                                        }))
                                    })
                            })
                            .collect();
                        let mut row = left_row;
                        row.push(Value::List(Box::new(crate::core::value::List { values })));
                        output.push(row);
                    }
                    if !output.is_empty() {
                        return Ok(Some(DataChunk::new_with_layout(
                            output,
                            Arc::clone(&output_layout),
                        )));
                    }
                }
            }
        }
    }

    pub fn stop(
        &mut self,
        base: &mut OperatorBase,
        _left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        base.lifecycle.mark_stopped();
        Ok(())
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        _left: &mut StreamingExecutor,
        _right: &mut StreamingExecutor,
    ) -> Result<(), QueryError> {
        if !base.lifecycle.can_close() {
            return Ok(());
        }
        match self {
            Self::Apply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            }
            | Self::PatternApply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            }
            | Self::CorrelatedApply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            }
            | Self::RollUpApply {
                right_rows,
                right_layout,
                memory_tracker,
                ..
            } => {
                right_rows.take();
                right_layout.take();
                memory_tracker.reset();
            }
        }
        base.lifecycle.mark_closed();
        Ok(())
    }
}

fn materialize_right(
    base: &OperatorBase,
    right: &mut StreamingExecutor,
    rows: &mut Option<Vec<Vec<Value>>>,
    layout: &mut Option<Arc<SlotLayout>>,
    memory_tracker: &mut MemoryTracker,
) -> Result<(), QueryError> {
    if rows.is_some() {
        return Ok(());
    }
    let mut materialized = Vec::new();
    while let Some(mut chunk) = right.advance()? {
        chunk.materialize_selection_by("Apply");
        base.ensure_not_cancelled()?;
        if layout.is_none() {
            *layout = Some(chunk.get_layout());
        }
        for row in &chunk.rows {
            memory_tracker.try_reserve_row(row)?;
        }
        materialized.extend(chunk.rows);
    }
    *rows = Some(materialized);
    Ok(())
}

fn matching_rows<'a>(
    left_row: &[Value],
    left_layout: &SlotLayout,
    right_rows: &'a [Vec<Value>],
    right_layout: &SlotLayout,
    correlated_columns: &[String],
) -> Result<Vec<&'a Vec<Value>>, QueryError> {
    let mut slots = Vec::with_capacity(correlated_columns.len());
    for column in correlated_columns {
        let left_slot = left_layout.resolve(column).ok_or_else(|| {
            QueryError::execution(format!("Apply left correlation column not found: {column}"))
        })?;
        let right_slot = right_layout.resolve(column).ok_or_else(|| {
            QueryError::execution(format!(
                "Apply right correlation column not found: {column}"
            ))
        })?;
        slots.push((left_slot, right_slot));
    }
    Ok(right_rows
        .iter()
        .filter(|right_row| {
            slots.iter().all(|(left_slot, right_slot)| {
                match (left_row.get(*left_slot), right_row.get(*right_slot)) {
                    (Some(Value::Null(_)), _) | (_, Some(Value::Null(_))) => false,
                    (Some(left), Some(right)) => left == right,
                    _ => false,
                }
            })
        })
        .collect())
}

fn keys_match(left: &[Value], right: &[Value]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            !matches!(left, Value::Null(_)) && !matches!(right, Value::Null(_)) && left == right
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::ContextualExpression;
    use crate::query::executor::streaming::operators::source_operator::SourceOperator;
    use crate::query::executor::streaming::runtime::ExecutionRuntime;
    use crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
    use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;

    fn scan(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        StreamingExecutor::Source(
            OperatorBase::new(0).with_output_layout(layout),
            SourceOperator::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names,
            },
        )
    }

    fn execute_apply(
        spec: ApplySpec,
        left_rows: Vec<Vec<Value>>,
        right_rows: Vec<Vec<Value>>,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        let left = scan(left_rows, vec!["id".to_string()]);
        let right = scan(right_rows, vec!["id".to_string()]);
        let budget = MemoryBudget::default_budget();
        let operator = ApplyOperator::from_spec(&spec, &budget);
        let mut executor = StreamingExecutor::Apply(
            OperatorBase::new(3),
            Box::new(left),
            Box::new(right),
            operator,
        );
        executor.open()?;
        let mut rows = Vec::new();
        while let Some(mut chunk) = executor.advance()? {
            chunk.materialize_selection_by("Apply");
            rows.extend(chunk.rows);
        }
        executor.close()?;
        Ok(rows)
    }

    #[test]
    fn semi_and_anti_apply_consume_the_right_input() {
        let semi = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Semi,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec![vec![Value::Int(2)]],
        )
        .expect("semi apply should execute");
        assert_eq!(semi, vec![vec![Value::Int(2)]]);

        let anti = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Anti,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            vec![vec![Value::Int(2)]],
        )
        .expect("anti apply should execute");
        assert_eq!(anti, vec![vec![Value::Int(1)]]);
    }

    #[test]
    fn single_apply_rejects_multiple_matches() {
        let result = execute_apply(
            ApplySpec::Apply {
                kind: ApplyKind::Single,
                correlated_columns: vec!["id".to_string()],
            },
            vec![vec![Value::Int(1)]],
            vec![vec![Value::Int(1)], vec![Value::Int(1)]],
        );
        assert!(result.is_err());
    }

    #[test]
    fn apply_uses_the_planned_output_layout() {
        let left = scan(vec![vec![Value::Int(7)]], vec!["left_input".to_string()]);
        let right = scan(vec![vec![Value::Int(7)]], vec!["left_input".to_string()]);
        let output_layout = Arc::new(SlotLayout::from_names(&[
            "planned_left".to_string(),
            "planned_right".to_string(),
        ]));
        let operator = ApplyOperator::from_spec(
            &ApplySpec::Apply {
                kind: ApplyKind::Standard,
                correlated_columns: vec!["left_input".to_string()],
            },
            &MemoryBudget::default_budget(),
        );
        let mut executor = StreamingExecutor::Apply(
            OperatorBase::new(3).with_output_layout(output_layout),
            Box::new(left),
            Box::new(right),
            operator,
        );

        executor.open().expect("apply should open");
        let chunk = executor
            .advance()
            .expect("apply should advance")
            .expect("apply should produce a row");
        executor.close().expect("apply should close");

        assert_eq!(chunk.rows, vec![vec![Value::Int(7), Value::Int(7)]]);
        assert_eq!(
            chunk.layout.names(),
            vec!["planned_left".to_string(), "planned_right".to_string()]
        );
    }

    /// Build a self-contained right subtree rooted at an `Argument` source:
    /// `Filter(condition) -> CrossJoin(Argument(col_names = ["id"]), Start)`.
    /// When `condition` is `None` the filter is omitted, so the subtree is
    /// non-empty for every correlation frame.
    fn build_sub_plan(condition: Option<ContextualExpression>) -> Arc<PhysicalPlan> {
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::query::executor::base::ExecutionContext;
        use crate::query::executor::streaming::plan::arena_builder::PhysicalPlanBuilder;
        use crate::query::executor::streaming::plan::context::PhysicalPlanBuildContext;
        use crate::query::executor::streaming::plan::validator::PhysicalPlanValidator;
        use crate::query::planning::plan::core::nodes::control_flow::start_node::StartNode;
        use crate::query::planning::plan::core::nodes::control_flow::ArgumentNode;
        use crate::query::planning::plan::core::nodes::join::CrossJoinNode;
        use crate::query::planning::plan::core::nodes::operation::filter_node::FilterNode;

        let mut argument = ArgumentNode::new(-2, "_correlated_apply");
        argument.set_col_names(vec!["id".to_string()]);
        let start = PlanNodeEnum::Start(StartNode::new());
        let cross = CrossJoinNode::new(PlanNodeEnum::Argument(argument), start)
            .expect("cross join should build");
        let node = match condition {
            Some(condition) => PlanNodeEnum::Filter(
                FilterNode::new(cross.into_enum(), condition).expect("filter should build"),
            ),
            None => cross.into_enum(),
        };

        let mut ctx = PhysicalPlanBuildContext::new();
        let exec_ctx = ExecutionContext::new(Arc::new(ExpressionAnalysisContext::new()));
        let plan =
            PhysicalPlanBuilder::build(&node, &mut ctx, &exec_ctx).expect("sub-plan should build");
        let plan = Arc::new(plan);
        PhysicalPlanValidator::validate(&plan).expect("sub-plan should validate");
        plan
    }

    fn correlated_bindings() -> QueryBindings {
        QueryBindings {
            parameters: Arc::new(std::collections::HashMap::new()),
            parameter_frame: None,
            space_name: None,
            storage: None,
            bound_snapshot: None,
            memory_budget: MemoryBudget::default_budget(),
            max_workers: 1,
            chunk_size: 1024,
            max_buffered_chunks: 4,
            query_id: 1,
            cancel_token: None,
            session_id: None,
            user_name: None,
            query_text: None,
            transaction:
                crate::query::executor::streaming::transaction_scope::TransactionScope::None,
            shared_scheduler: None,
            partition_count: 0,
            arena: None,
            feedback_history: None,
            columnar_policy: None,
            #[cfg(feature = "fulltext-search")]
            fulltext_manager: None,
            #[cfg(feature = "qdrant")]
            vector_coordinator: None,
        }
    }

    fn execute_correlated_apply(
        sub_plan: Arc<PhysicalPlan>,
        anti: bool,
        left_rows: Vec<Vec<Value>>,
    ) -> Result<Vec<Vec<Value>>, QueryError> {
        let left = scan(left_rows, vec!["id".to_string()]);
        let right = StreamingExecutor::Source(OperatorBase::new(0), SourceOperator::Start);
        let budget = MemoryBudget::default_budget();
        let operator = ApplyOperator::CorrelatedApply {
            sub_plan,
            bindings: Box::new(correlated_bindings()),
            anti,
            right_rows: None,
            right_layout: None,
            memory_tracker: MemoryTracker::new(budget),
        };
        let mut executor = StreamingExecutor::Apply(
            OperatorBase::new(3),
            Box::new(left),
            Box::new(right),
            operator,
        );
        executor.set_chunk_size(1024);
        executor.set_runtime(Some(Arc::new(ExecutionRuntime::default_budget())));
        executor.open()?;
        let mut rows = Vec::new();
        while let Some(mut chunk) = executor.advance()? {
            chunk.materialize_selection_by("CorrelatedApply");
            rows.extend(chunk.rows);
        }
        executor.close()?;
        Ok(rows)
    }

    #[test]
    fn correlated_apply_semi_and_anti_use_right_subtree_existence() {
        let sub_plan = build_sub_plan(None);

        let semi = execute_correlated_apply(
            sub_plan.clone(),
            false,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        )
        .expect("semi correlated apply should execute");
        assert_eq!(
            semi,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
            "semi keeps rows whose right subtree is non-empty"
        );

        let anti = execute_correlated_apply(
            sub_plan,
            true,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        )
        .expect("anti correlated apply should execute");
        assert!(
            anti.is_empty(),
            "anti drops rows whose right subtree is non-empty"
        );
    }

    #[test]
    fn correlated_apply_binds_the_frame_per_row() {
        // Filter that only passes the frame whose `id` equals 2. The left
        // input carries two rows, so the per-row frame must differ.
        use crate::core::types::expr::expression_context::ExpressionAnalysisContext;
        use crate::core::types::operators::BinaryOperator;
        use crate::core::types::{ContextualExpression, ExpressionMeta};
        use crate::core::Expression;

        let condition_expr = Expression::binary(
            Expression::variable("id"),
            BinaryOperator::Equal,
            Expression::literal(crate::core::Value::Int(2)),
        );
        let expr_ctx = Arc::new(ExpressionAnalysisContext::new());
        let expr_id = expr_ctx.register_expression(ExpressionMeta::new(condition_expr));
        let condition = ContextualExpression::new(expr_id, expr_ctx);
        let sub_plan = build_sub_plan(Some(condition));

        let semi = execute_correlated_apply(
            sub_plan.clone(),
            false,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        )
        .expect("semi correlated apply should execute");
        assert_eq!(
            semi,
            vec![vec![Value::Int(2)]],
            "only the row whose bound frame satisfies the filter is kept"
        );

        let anti = execute_correlated_apply(
            sub_plan,
            true,
            vec![vec![Value::Int(1)], vec![Value::Int(2)]],
        )
        .expect("anti correlated apply should execute");
        assert_eq!(
            anti,
            vec![vec![Value::Int(1)]],
            "anti keeps only rows whose bound frame makes the subtree empty"
        );
    }
}
