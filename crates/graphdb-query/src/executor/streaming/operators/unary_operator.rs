use std::sync::Arc;

use crate::executor::expression::evaluator::compiled::{compiled_eval_enabled, CompiledExpr};
use crate::executor::expression::evaluator::ExpressionEvaluator;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::executor::ValueRowContext;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use crate::executor::streaming::subquery::{EvalEnv, SubqueryExecutor};
use graphdb_core::error::QueryError;
use graphdb_core::types::expr::Expression;
use graphdb_core::Value;

mod filter;
mod project;
mod limit;
mod dedup;
mod assign;
mod remove;
mod unwind;
mod append_vertices;
mod sample;
mod flatten;

#[derive(Debug, Default)]
pub struct UnaryOperatorState {
    pub env: EvalEnv,
    /// Lazily-compiled Filter predicate (compiled on the first chunk whose
    /// layout binds the expression's slots; reused for all later chunks).
    pub compiled_predicate: Option<CompiledExpr>,
    /// Lazily-compiled Project output expressions, one per output column.
    pub compiled_project: Option<Vec<CompiledExpr>>,
}

#[derive(Debug)]
pub enum UnaryOperatorKind {
    Filter {
        predicate: Expression,
        state: UnaryOperatorState,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
        state: UnaryOperatorState,
    },
    Limit {
        offset: u32,
        limit: u32,
        skipped: u32,
        consumed: u32,
    },
    Dedup {
        seen_rows: std::collections::HashSet<Vec<Value>>,
    },
    Assign {
        assignments: Vec<(String, Expression)>,
        state: UnaryOperatorState,
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
        list_expression: Option<Expression>,
        col_index: Option<usize>,
        layout: Option<Arc<SlotLayout>>,
        all_rows: Vec<Vec<Value>>,
        /// Multiplicity of the chunk `all_rows` was taken from: every
        /// emitted unwound row still occurs this many times.
        input_multiplicity: u64,
        current_row_index: usize,
        current_unwind_index: usize,
        input_done: bool,
    },
    AppendVertices {
        entity_var: String,
        entity_expr: Expression,
        prop_names: Vec<String>,
        tag: String,
        storage: Option<Arc<parking_lot::RwLock<dyn crate::storage::QueryStorage>>>,
        space_name: String,
        state: UnaryOperatorState,
    },
    Sample {
        count: u64,
        consumed: u64,
    },
    Flatten {
        group_pos: u32,
        group_columns: Vec<String>,
        expected_groups: Option<u32>,
        current_idx: usize,
        size_to_flatten: usize,
        saved_sel_vector: Option<Vec<usize>>,
        buffered_chunk: Option<DataChunk>,
        /// Rows emitted per output chunk. Defaults to the vectorized morsel
        /// size so the batched flatten path produces one chunk per input
        /// morsel; tests may set `1` for the single-row path.
        batch_size: usize,
    },
}

/// Unary operator.
///
/// Wraps [`UnaryOperatorKind`] with the runtime context injected at `open()`.
/// Lifecycle state is owned exclusively by the executor; operators never
/// write it.
#[derive(Debug)]
pub struct UnaryOperator {
    pub kind: UnaryOperatorKind,
    pub runtime: Option<Arc<ExecutionRuntime>>,
    pub output_layout: Arc<SlotLayout>,
    pub config: OperatorConfig,
}

impl UnaryOperatorKind {
    /// Plan level factorization group this flatten replays, if any.
    pub fn flatten_group_pos(&self) -> Option<u32> {
        match self {
            UnaryOperatorKind::Flatten { group_pos, .. } => Some(*group_pos),
            _ => None,
        }
    }
}

impl UnaryOperator {
    /// Create a UnaryOperator with fresh mutable state from an immutable spec.
    pub fn from_spec(spec: &super::spec::UnarySpec, output_layout: Arc<SlotLayout>) -> Self {
        let state = UnaryOperatorState::default();
        let kind = match spec {
            super::spec::UnarySpec::Filter {
                predicate,
                subquery_runners: _,
            } => UnaryOperatorKind::Filter {
                predicate: predicate.clone(),
                state,
            },
            super::spec::UnarySpec::Project {
                output_expressions,
                output_col_names,
                ..
            } => UnaryOperatorKind::Project {
                output_expressions: output_expressions.clone(),
                output_col_names: output_col_names.clone(),
                state,
            },
            super::spec::UnarySpec::Limit { offset, limit } => UnaryOperatorKind::Limit {
                offset: *offset,
                limit: *limit,
                skipped: 0,
                consumed: 0,
            },
            super::spec::UnarySpec::Assign {
                assignments,
                subquery_runners: _,
            } => UnaryOperatorKind::Assign {
                assignments: assignments.clone(),
                state,
            },
            super::spec::UnarySpec::Remove { columns_to_remove } => UnaryOperatorKind::Remove {
                columns_to_remove: columns_to_remove.clone(),
            },
            super::spec::UnarySpec::Unwind {
                unwind_column,
                list_expression,
            } => UnaryOperatorKind::Unwind {
                unwind_column: unwind_column.clone(),
                list_expression: list_expression.clone(),
                col_index: None,
                layout: None,
                all_rows: Vec::new(),
                input_multiplicity: 1,
                current_row_index: 0,
                current_unwind_index: 0,
                input_done: false,
            },
            super::spec::UnarySpec::AppendVertices {
                space_name,
                tag,
                entity_var,
                entity_expr,
                prop_names,
            } => UnaryOperatorKind::AppendVertices {
                entity_var: entity_var.clone(),
                entity_expr: entity_expr.clone(),
                prop_names: prop_names.clone(),
                tag: tag.clone(),
                storage: None,
                space_name: space_name.clone(),
                state,
            },
            super::spec::UnarySpec::Sample { count } => UnaryOperatorKind::Sample {
                count: *count,
                consumed: 0,
            },
            super::spec::UnarySpec::Flatten {
                group_pos,
                group_columns,
                expected_groups,
            } => UnaryOperatorKind::Flatten {
                group_pos: *group_pos,
                group_columns: group_columns.clone(),
                expected_groups: *expected_groups,
                current_idx: 0,
                size_to_flatten: 0,
                saved_sel_vector: None,
                buffered_chunk: None,
                batch_size:
                    crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE,
            },
        };
        Self::new(kind, output_layout)
    }

    pub fn new(kind: UnaryOperatorKind, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            kind,
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    /// Inject the runtime and execution config (called once by the executor
    /// before this operator produces any data).
    pub fn inject_context(
        &mut self,
        runtime: Option<&Arc<ExecutionRuntime>>,
        config: OperatorConfig,
    ) {
        if let Some(rt) = runtime {
            self.runtime = Some(rt.clone());
        }
        self.config = config;
    }

    /// Inject the hosting operator's expression-level subquery executor into
    /// the Filter/Project/Assign state. Called by the materializer
    /// after `from_spec`; no-op for the other unary kinds.
    pub fn set_subquery_executor(&mut self, executor: Arc<SubqueryExecutor>) {
        match &mut self.kind {
            UnaryOperatorKind::Filter { state, .. }
            | UnaryOperatorKind::Project { state, .. }
            | UnaryOperatorKind::Assign { state, .. } => {
                state.env.subquery_executor = Some(executor);
            }
            _ => {}
        }
    }

    pub fn open(&mut self, input: &mut StreamingExecutor) -> Result<(), QueryError> {
        let params = self.runtime.as_ref().and_then(|rt| rt.parameter_values());
        let session_variables = self
            .runtime
            .as_ref()
            .and_then(|rt| rt.session_variable_values());
        let storage = self.runtime.as_ref().and_then(|rt| rt.storage.clone());
        match &mut self.kind {
            UnaryOperatorKind::Filter { state, .. }
            | UnaryOperatorKind::Project { state, .. }
            | UnaryOperatorKind::Assign { state, .. }
            | UnaryOperatorKind::AppendVertices { state, .. } => {
                state.env.params = params;
                state.env.session_variables = session_variables;
            }
            _ => {}
        }
        if let UnaryOperatorKind::AppendVertices {
            storage: target, ..
        } = &mut self.kind
        {
            *target = storage;
        }
        // The streaming engine stores flat row batches only, so flatten
        // replays rows instead of expanding a nested layout. The
        // `expected_groups` count snapshotted at rewrite time makes the
        // plan position checkable anyway: a stale `group_pos` at or above
        // the count fails loudly here instead of replaying the wrong
        // group silently. `None` means unknown (hand-built or legacy
        // plan) and skips the check honestly. The `group_columns`
        // mapping is traceability metadata only (EXPLAIN-visible): the
        // schema alias namespace and the runtime column namespace are
        // not identical, so names are logged, never validated.
        if let UnaryOperatorKind::Flatten {
            group_pos,
            group_columns,
            expected_groups,
            ..
        } = &self.kind
        {
            match expected_groups {
                Some(count) if *group_pos >= *count => {
                    return Err(QueryError::execution(format!(
                        "Flatten(group={group_pos}): position out of range for {count} group(s)"
                    )));
                }
                Some(count) => {
                    log::debug!(
                        "flatten(group={group_pos}): position within {count} group(s), mapped columns: [{}]",
                        group_columns.join(", ")
                    );
                }
                None => {
                    log::debug!(
                        "flatten(group={group_pos}): unknown group count; skipping position validation"
                    );
                }
            }
        }
        input.open()?;
        Ok(())
    }

    pub fn next(&mut self, input: &mut StreamingExecutor) -> Result<Option<DataChunk>, QueryError> {
        if matches!(&self.kind, UnaryOperatorKind::Filter { .. }) { return filter::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Project { .. }) { return project::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Limit { .. }) { return limit::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Dedup { .. }) { return dedup::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Assign { .. }) { return assign::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Remove { .. }) { return remove::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Unwind { .. }) { return unwind::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::AppendVertices { .. }) { return append_vertices::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Sample { .. }) { return sample::handle(self, input); }
        if matches!(&self.kind, UnaryOperatorKind::Flatten { .. }) { return flatten::handle(self, input); }
        unreachable!("unary_operator::next called for an unknown kind")
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    /// Reset this operator's per-run counters/buffers and rewind the input.
    ///
    /// Stateless operators (Filter/Project/Assign/Remove/AppendVertices)
    /// are no-ops beyond rewinding the input; Limit/Sample reset their
    /// counters; Dedup clears its seen-set; Unwind clears its input buffer.
    pub fn reset(&mut self, input: &mut StreamingExecutor) -> Result<bool, QueryError> {
        match &mut self.kind {
            UnaryOperatorKind::Limit {
                skipped, consumed, ..
            } => {
                *skipped = 0;
                *consumed = 0;
            }
            UnaryOperatorKind::Dedup { seen_rows } => seen_rows.clear(),
            UnaryOperatorKind::Unwind {
                col_index,
                layout,
                all_rows,
                input_multiplicity,
                current_row_index,
                current_unwind_index,
                input_done,
                ..
            } => {
                *col_index = None;
                *layout = None;
                all_rows.clear();
                *input_multiplicity = 1;
                *current_row_index = 0;
                *current_unwind_index = 0;
                *input_done = false;
            }
            UnaryOperatorKind::Sample { consumed, .. } => *consumed = 0,
            UnaryOperatorKind::Flatten {
                current_idx,
                size_to_flatten,
                saved_sel_vector,
                buffered_chunk,
                batch_size,
                ..
            } => {
                *current_idx = 0;
                *size_to_flatten = 0;
                *saved_sel_vector = None;
                *buffered_chunk = None;
                *batch_size =
                    crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE;
            }
            UnaryOperatorKind::Filter { .. }
            | UnaryOperatorKind::Project { .. }
            | UnaryOperatorKind::Assign { .. }
            | UnaryOperatorKind::Remove { .. }
            | UnaryOperatorKind::AppendVertices { .. } => {}
        }
        input.reset()?;
        Ok(false)
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        if let UnaryOperatorKind::Flatten { buffered_chunk, .. } = &mut self.kind {
            if let Some(chunk) = buffered_chunk.take() {
                if let Some(stats) = chunk.columnar_stats {
                    stats.record_selection_materialized();
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::base::OperatorBase;
    use crate::executor::streaming::operators::source_operator::SourceOperator;
    use crate::executor::streaming::operators::source_operator::SourceOperatorKind;
    use crate::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::storage::StorageWriter;
    use graphdb_core::types::storage_ids::VertexId;
    use graphdb_core::{Tag, Vertex};
    use parking_lot::RwLock;

    fn runtime_with_storage(
        storage: Arc<RwLock<dyn crate::storage::QueryStorage>>,
    ) -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            Some(storage),
            crate::executor::base::SearchContext::default(),
        ))
    }

    fn scan_source(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> Box<StreamingExecutor> {
        Box::new(StreamingExecutor::Source(
            OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: rows,
                    current_index: 0,
                    col_names,
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        ))
    }

    #[test]
    fn append_vertices_fetches_vertex_and_appends_flat_columns() {
        let mut mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        mock.insert_vertex(
            "test",
            Vertex::new(
                VertexId::try_from_int64(1).expect("valid vertex id"),
                Tag::new(
                    "person".to_string(),
                    vec![
                        ("name".to_string(), Value::string("Alice")),
                        ("age".to_string(), Value::Int(30)),
                    ]
                    .into_iter()
                    .collect(),
                ),
            ),
        )
        .expect("insert vertex");
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));

        // Input row: [vid]. The id is typed: a numeric string stays text
        // under the typed VertexId contract, so the integer form is used
        // for an integer vertex (the mock has no space vid_type to
        // normalize through; real storage normalizes at assembly).
        let input = scan_source(vec![vec![Value::Int(1)]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string(), "age".to_string()],
                    tag: "person".to_string(),
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance should succeed");
        let chunk = chunk.expect("one output chunk");
        assert_eq!(chunk.len(), 1);
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 3, "vid + name + age");
        assert_eq!(row[0], Value::Int(1));
        assert_eq!(row[1], Value::string("Alice"));
        assert_eq!(row[2], Value::Int(30));
        assert!(append.advance().expect("advance").is_none());
    }

    #[test]
    fn append_vertices_full_value_appends_vertex_object() {
        let mut mock = crate::storage::MockStorage::new().expect("MockStorage should be created");
        mock.insert_vertex(
            "test",
            Vertex::new(
                VertexId::try_from_int64(7).expect("valid vertex id"),
                Tag::new(
                    "person".to_string(),
                    vec![("name".to_string(), Value::string("Bob"))]
                        .into_iter()
                        .collect(),
                ),
            ),
        )
        .expect("insert vertex");
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(mock));

        let input = scan_source(vec![vec![Value::Int(7)]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec![],
                    tag: "person".to_string(),
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance").expect("one chunk");
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2, "vid + full vertex");
        match &row[1] {
            Value::Vertex(vertex) => {
                assert_eq!(
                    vertex.vid,
                    VertexId::try_from_int64(7).expect("valid vertex id")
                );
                assert_eq!(vertex.property_value("name"), Some(Value::string("Bob")));
            }
            other => panic!("expected full vertex, got {:?}", other),
        }
    }

    #[test]
    fn append_vertices_missing_vertex_yields_null_columns() {
        let storage: Arc<RwLock<dyn crate::storage::QueryStorage>> = Arc::new(RwLock::new(
            crate::storage::MockStorage::new().expect("MockStorage should be created"),
        ));
        let input = scan_source(
            vec![vec![Value::string("missing")]],
            vec!["vid".to_string()],
        );
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string()],
                    tag: "person".to_string(),
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.base_mut().runtime = Some(runtime_with_storage(storage.clone()));
        append.open().expect("open should succeed");
        let chunk = append.advance().expect("advance").expect("one chunk");
        let row = &chunk.rows[0];
        assert_eq!(row.len(), 2);
        assert!(matches!(row[1], Value::Null(_)));
    }

    #[test]
    fn flatten_group_pos_is_observable() {
        let kind = UnaryOperatorKind::Flatten {
            group_pos: 7,
            group_columns: vec!["a".to_string()],
            expected_groups: Some(8),
            current_idx: 0,
            size_to_flatten: 0,
            saved_sel_vector: None,
            buffered_chunk: None,
            batch_size: crate::executor::streaming::operators::flatten::DEFAULT_FLATTEN_BATCH_SIZE,
        };
        assert_eq!(kind.flatten_group_pos(), Some(7));
        let filter = UnaryOperatorKind::Limit {
            offset: 0,
            limit: 1,
            skipped: 0,
            consumed: 0,
        };
        assert_eq!(filter.flatten_group_pos(), None);
    }

    fn flatten_op_with_mapping(
        group_columns: Vec<String>,
        expected_groups: Option<u32>,
        layout_names: &[&str],
    ) -> (UnaryOperator, Box<StreamingExecutor>) {
        let layout = Arc::new(SlotLayout::from_names(
            &layout_names
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>(),
        ));
        let op = UnaryOperator::from_spec(
            &crate::executor::streaming::operators::spec::UnarySpec::Flatten {
                group_pos: 1,
                group_columns,
                expected_groups,
            },
            layout,
        );
        let input = scan_source(
            vec![vec![Value::Int(1)]],
            layout_names.iter().map(|s| s.to_string()).collect(),
        );
        (op, input)
    }

    #[test]
    fn flatten_open_accepts_position_within_expected_groups() {
        let (mut op, mut input) =
            flatten_op_with_mapping(vec!["b".to_string()], Some(2), &["a", "b"]);
        assert!(op.open(&mut input).is_ok());
    }

    #[test]
    fn flatten_open_without_expected_groups_skips_validation() {
        let (mut op, mut input) = flatten_op_with_mapping(vec![], None, &["a", "b"]);
        assert!(op.open(&mut input).is_ok());
    }

    #[test]
    fn flatten_open_rejects_stale_group_position() {
        // Plan/executor drift: group 1 cannot exist when the rewrite saw
        // only one group. The stale position fails loudly at open instead
        // of replaying the wrong group silently.
        let (mut op, mut input) =
            flatten_op_with_mapping(vec!["b".to_string()], Some(1), &["a", "b"]);
        let err = op
            .open(&mut input)
            .expect_err("stale group position must fail");
        assert!(
            err.to_string().contains("out of range"),
            "error must report the range violation, got: {err}"
        );
    }

    #[test]
    fn append_vertices_without_storage_fails() {
        let input = scan_source(vec![vec![Value::Int(1)]], vec!["vid".to_string()]);
        let mut append = StreamingExecutor::Unary(
            OperatorBase::new(0),
            input,
            UnaryOperator::new(
                UnaryOperatorKind::AppendVertices {
                    entity_var: "v".to_string(),
                    entity_expr: Expression::Variable("vid".to_string()),
                    prop_names: vec!["name".to_string()],
                    tag: "person".to_string(),
                    storage: None,
                    space_name: "test".to_string(),
                    state: UnaryOperatorState::default(),
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        );
        append.open().expect("open should succeed");
        let error = append.advance().expect_err("storage required");
        assert!(error.to_string().contains("requires storage"));
    }
}

#[cfg(test)]
mod project_fast_path_tests {
    use super::*;
    use super::project::is_passthrough_or_const;
    use graphdb_core::Value;

    fn layout() -> Arc<SlotLayout> {
        Arc::new(SlotLayout::from_names(&[
            "n".to_string(),
            "n.age".to_string(),
        ]))
    }

    fn chunk_with_selection() -> DataChunk {
        let rows = vec![
            vec![Value::Int(1), Value::Int(30)],
            vec![
                Value::Int(2),
                Value::Null(graphdb_core::value::NullType::Null),
            ],
            vec![Value::Int(3), Value::Int(40)],
        ];
        DataChunk::new_with_layout(rows, layout())
            .with_selection(vec![0, 2])
            .with_multiplicity(3)
    }

    #[test]
    fn test_is_passthrough_or_const() {
        assert!(is_passthrough_or_const(&Expression::Variable(
            "n".to_string()
        )));
        assert!(is_passthrough_or_const(&Expression::Literal(Value::Int(1))));
        assert!(is_passthrough_or_const(&Expression::Property {
            object: Box::new(Expression::Variable("n".to_string())),
            property: "age".to_string(),
        }));
        assert!(!is_passthrough_or_const(&Expression::Binary {
            left: Box::new(Expression::Variable("n".to_string())),
            op: graphdb_core::types::operators::BinaryOperator::Add,
            right: Box::new(Expression::Literal(Value::Int(1))),
        }));
    }

    #[test]
    fn test_project_fast_path_gathers_all_rows() {
        // The fast path serves the no-selection branch of `Project::next`:
        // every visible row is evaluated, including NULLs.
        let exprs = vec![
            Expression::Variable("n".to_string()),
            Expression::Property {
                object: Box::new(Expression::Variable("n".to_string())),
                property: "age".to_string(),
            },
            Expression::Literal(Value::Int(7)),
        ];
        let mut state = UnaryOperatorState::default();
        let rows = vec![
            vec![Value::Int(1), Value::Int(30)],
            vec![
                Value::Int(2),
                Value::Null(graphdb_core::value::NullType::Null),
            ],
            vec![Value::Int(3), Value::Int(40)],
        ];
        let mut chunk = DataChunk::new_with_layout(rows, layout());
        let fast = UnaryOperator::evaluate_project_expressions(&mut chunk, &exprs, &mut state)
            .expect("fast path evaluates");
        assert_eq!(
            fast,
            vec![
                vec![Value::Int(1), Value::Int(2), Value::Int(3)],
                vec![
                    Value::Int(30),
                    Value::Null(graphdb_core::value::NullType::Null),
                    Value::Int(40)
                ],
                vec![Value::Int(7), Value::Int(7), Value::Int(7)],
            ]
        );

        // The selection branch keeps visible-row semantics (NULL row hidden
        // here) and carries multiplicity into the projected chunk.
        let mut selected = chunk_with_selection();
        let mut visible = Vec::new();
        for expr in &exprs {
            visible.push(
                selected
                    .evaluate_expression_visible(expr, Some(&state.env))
                    .expect("visible evaluates"),
            );
        }
        let out_layout = Arc::new(SlotLayout::from_names(&[
            "n".to_string(),
            "n.age".to_string(),
            "c".to_string(),
        ]));
        let out = DataChunk::project_columns(visible, Arc::clone(&out_layout)).with_multiplicity(3);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(
            out.rows[0],
            vec![Value::Int(1), Value::Int(30), Value::Int(7)]
        );
        assert_eq!(
            out.rows[1],
            vec![Value::Int(3), Value::Int(40), Value::Int(7)]
        );
        assert_eq!(out.multiplicity(), 3);
    }
}
