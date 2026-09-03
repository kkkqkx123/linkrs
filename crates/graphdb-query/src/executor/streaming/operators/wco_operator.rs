//! Streaming N-way WCO intersect operator.
//!
//! One probe input plus N build inputs share a single intersect variable.
//! Every build side is drained into a sorted
//! [`IntersectBuild`](super::intersect_build::IntersectBuild) table keyed by
//! its bound value; each probe row then emits one output row per intersect
//! value present on ALL sides, crossed with the matching payload rows.
//!
//! Column positions resolve per input chunk from the spec's variable names,
//! so upstream layout changes cannot silently misalign keys. Build sides
//! are memory-tracked per row (the hash-join discipline); spilling the
//! sorted adjacency tables is future work, shared with the other blocking
//! join builds.

use std::sync::Arc;

use crate::executor::base::MemoryTracker;
use crate::executor::streaming::chunk::DataChunk;
use crate::executor::streaming::executor::StreamingExecutor;
use crate::executor::streaming::operators::intersect_build::IntersectBuild;
use crate::executor::streaming::operators::source_operator::OperatorConfig;
use crate::executor::streaming::runtime::ExecutionRuntime;
use crate::executor::streaming::slot::SlotLayout;
use graphdb_core::error::QueryError;
use graphdb_core::value::NullType;
use graphdb_core::Value;

use super::intersect::WcoIntersectExecutor;
use super::spec::WcoSpec;

/// Column positions of one build side inside its own chunks.
#[derive(Debug, Clone, Default)]
struct BuildLayout {
    bound_col: usize,
    intersect_col: usize,
    col_names: Vec<String>,
}

/// N-way WCO intersect operator.
///
/// Wraps the value-level [`WcoIntersectExecutor`] with chunk draining,
/// name-based column resolution, memory tracking, and output assembly in
/// the plan's column order. Lifecycle state is owned exclusively by the
/// executor; operators never write it.
#[derive(Debug)]
pub struct WcoIntersectOperator {
    spec: WcoSpec,
    builds: Vec<IntersectBuild>,
    build_layouts: Vec<BuildLayout>,
    build_done: bool,
    memory_tracker: MemoryTracker,
    runtime: Option<Arc<ExecutionRuntime>>,
    output_layout: Arc<SlotLayout>,
    config: OperatorConfig,
}

impl WcoIntersectOperator {
    pub fn new(spec: WcoSpec, output_layout: Arc<SlotLayout>) -> Self {
        Self {
            spec,
            builds: Vec::new(),
            build_layouts: Vec::new(),
            build_done: false,
            memory_tracker: MemoryTracker::new(crate::executor::base::MemoryBudget::new(
                crate::executor::base::MemoryBudget::DEFAULT_MAX,
            )),
            runtime: None,
            output_layout,
            config: OperatorConfig::default(),
        }
    }

    pub fn from_spec(
        spec: &WcoSpec,
        memory_budget: &crate::executor::base::MemoryBudget,
        output_layout: Arc<SlotLayout>,
    ) -> Self {
        let mut op = Self::new(spec.clone(), output_layout);
        op.memory_tracker = MemoryTracker::new(memory_budget.clone());
        op
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

    pub fn memory_tracker(&self) -> &MemoryTracker {
        &self.memory_tracker
    }

    pub fn open(
        &mut self,
        probe: &mut StreamingExecutor,
        builds: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        probe.open()?;
        for build in builds.iter_mut() {
            build.open()?;
        }
        Ok(())
    }

    pub fn next(
        &mut self,
        probe: &mut StreamingExecutor,
        builds: &mut [StreamingExecutor],
    ) -> Result<Option<DataChunk>, QueryError> {
        if builds.len() != self.spec.num_builds() {
            return Err(QueryError::execution(format!(
                "WcoIntersect planned {} build sides but received {} inputs",
                self.spec.num_builds(),
                builds.len()
            )));
        }
        if !self.build_done {
            self.drain_builds(builds)?;
        }
        let output_layout = Arc::clone(&self.output_layout);
        while let Some(probe_chunk) = probe.advance()? {
            if let Some(rt) = self.runtime.as_ref() {
                rt.ensure_not_cancelled()?;
            }
            let chunk_names = probe_chunk.col_names();
            let mut bound_cols = Vec::with_capacity(self.spec.bound_names.len());
            for name in &self.spec.bound_names {
                bound_cols.push(chunk_names.iter().position(|c| c == name).ok_or_else(|| {
                    QueryError::execution(format!(
                        "WcoIntersect probe side is missing bound column `{name}`"
                    ))
                })?);
            }
            // The executor borrows the sealed tables; move them out and
            // restore afterwards so consecutive chunks reuse the same state.
            let builds = std::mem::take(&mut self.builds);
            let executor = WcoIntersectExecutor::new(builds, bound_cols);
            let mut result_rows = Vec::new();
            for row_idx in probe_chunk.visible_indices() {
                for wide in executor.probe_row(&probe_chunk.rows[row_idx]) {
                    result_rows.push(self.assemble_output_row(&wide, &chunk_names));
                }
            }
            self.builds = executor.into_builds();
            if !result_rows.is_empty() {
                return Ok(Some(DataChunk::new_with_layout(result_rows, output_layout)));
            }
        }
        Ok(None)
    }

    /// Reset build tables and rewind every input so the operator
    /// re-produces the same result set.
    pub fn reset(
        &mut self,
        probe: &mut StreamingExecutor,
        builds: &mut [StreamingExecutor],
    ) -> Result<bool, QueryError> {
        self.builds.clear();
        self.build_layouts.clear();
        self.build_done = false;
        self.memory_tracker.reset();
        probe.reset()?;
        for build in builds.iter_mut() {
            build.reset()?;
        }
        Ok(false)
    }

    pub fn stop(&mut self) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn close(&mut self) -> Result<(), QueryError> {
        self.memory_tracker.reset();
        for build in &mut self.builds {
            build.clear();
        }
        Ok(())
    }

    pub fn spill_with_manager(
        &mut self,
        _sm: &crate::executor::streaming::spill::SpillManager,
    ) -> Result<(), QueryError> {
        Ok(())
    }

    pub fn spilled_bytes(&self) -> u64 {
        0
    }

    /// Drain every build input into its sorted adjacency table.
    fn drain_builds(&mut self, builds: &mut [StreamingExecutor]) -> Result<(), QueryError> {
        self.builds = Vec::with_capacity(builds.len());
        self.build_layouts = Vec::with_capacity(builds.len());
        for (side, build) in builds.iter_mut().enumerate() {
            let mut table: Option<IntersectBuild> = None;
            let mut layout = BuildLayout::default();
            let mut col_names = Vec::new();
            while let Some(mut chunk) = build.advance()? {
                if let Some(rt) = self.runtime.as_ref() {
                    rt.ensure_not_cancelled()?;
                }
                chunk.materialize_selection_by("WcoBuild");
                if col_names.is_empty() {
                    col_names = chunk.col_names();
                    layout = self.resolve_build_layout(side, &col_names)?;
                }
                for row in chunk.rows.iter() {
                    self.memory_tracker.try_reserve_row(row)?;
                }
                let table = table.get_or_insert_with(|| {
                    IntersectBuild::new(layout.bound_col, layout.intersect_col)
                });
                for row in chunk.rows.iter().cloned() {
                    table.append_row(row);
                }
            }
            // An empty build side yields an empty (sealed) table: every
            // probe misses, which is the correct inner-join semantics.
            let mut table = table.unwrap_or_else(|| IntersectBuild::new(0, 0));
            table.finish();
            layout.col_names = col_names;
            self.builds.push(table);
            self.build_layouts.push(layout);
        }
        self.build_done = true;
        Ok(())
    }

    /// Resolve one build side's column positions from its chunk names.
    fn resolve_build_layout(
        &self,
        side: usize,
        col_names: &[String],
    ) -> Result<BuildLayout, QueryError> {
        let bound_name = &self.spec.bound_names[side];
        let bound_col = col_names
            .iter()
            .position(|c| c == bound_name)
            .ok_or_else(|| {
                QueryError::execution(format!(
                    "WcoIntersect build side {side} is missing bound column `{bound_name}`"
                ))
            })?;
        let intersect_name = &self.spec.intersect_name;
        let intersect_col = col_names
            .iter()
            .position(|c| c == intersect_name)
            .ok_or_else(|| {
                QueryError::execution(format!(
                    "WcoIntersect build side {side} is missing intersect column `{intersect_name}`"
                ))
            })?;
        Ok(BuildLayout {
            bound_col,
            intersect_col,
            col_names: Vec::new(),
        })
    }

    /// Map one wide library row (`probe ++ [intersect] ++ builds`) to the
    /// plan's output column order.
    fn assemble_output_row(&self, wide: &[Value], probe_names: &[String]) -> Vec<Value> {
        // Library layout: probe row, one intersect value, then each full
        // build row in side order.
        let probe_len = probe_names.len();
        let mut build_offsets = Vec::with_capacity(self.build_layouts.len());
        let mut offset = probe_len + 1;
        for layout in &self.build_layouts {
            build_offsets.push(offset);
            offset += layout.col_names.len();
        }
        let rep = wide
            .get(probe_len)
            .cloned()
            .unwrap_or(Value::Null(NullType::Null));
        let mut out = Vec::with_capacity(self.spec.output_col_names.len());
        for name in &self.spec.output_col_names {
            if name == &self.spec.intersect_name {
                out.push(rep.clone());
                continue;
            }
            if let Some(idx) = probe_names.iter().position(|c| c == name) {
                out.push(
                    wide.get(idx)
                        .cloned()
                        .unwrap_or(Value::Null(NullType::Null)),
                );
                continue;
            }
            let mut pushed = false;
            for (side, layout) in self.build_layouts.iter().enumerate() {
                if let Some(idx) = layout.col_names.iter().position(|c| c == name) {
                    let pos = build_offsets[side] + idx;
                    out.push(
                        wide.get(pos)
                            .cloned()
                            .unwrap_or(Value::Null(NullType::Null)),
                    );
                    pushed = true;
                    break;
                }
            }
            if !pushed {
                out.push(Value::Null(NullType::Null));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::streaming::operators::source_operator::{
        SourceOperator, SourceOperatorKind,
    };
    use crate::executor::streaming::slot::SlotLayout;
    use graphdb_core::Value;

    fn source(rows: Vec<Vec<Value>>, col_names: Vec<String>) -> StreamingExecutor {
        let layout = Arc::new(SlotLayout::from_names(&col_names));
        let base = crate::executor::streaming::operators::base::OperatorBase::new(1)
            .with_output_layout(layout.clone());
        let op = SourceOperator::new(
            SourceOperatorKind::ScanVertices {
                buffer: rows,
                current_index: 0,
                col_names,
            },
            layout,
        );
        StreamingExecutor::Source(base, op)
    }

    fn triangle_spec() -> WcoSpec {
        WcoSpec {
            bound_names: vec!["a".to_string(), "b".to_string()],
            intersect_name: "c".to_string(),
            output_col_names: vec![
                "a".to_string(),
                "b".to_string(),
                "e1".to_string(),
                "c".to_string(),
                "e2".to_string(),
                "e3".to_string(),
            ],
        }
    }

    /// Triangle: probe (a,b,e1); build e2 keyed by a carries c; build e3
    /// keyed by b carries c. Only c=20 is common to a=1 and b=2.
    #[test]
    fn triangle_probes_common_adjacency() {
        let layout = Arc::new(SlotLayout::from_names(&triangle_spec().output_col_names));
        let mut op = WcoIntersectOperator::from_spec(
            &triangle_spec(),
            &crate::executor::base::MemoryBudget::new(
                crate::executor::base::MemoryBudget::DEFAULT_MAX,
            ),
            layout.clone(),
        );
        let mut probe = source(
            vec![vec![Value::Int(1), Value::Int(2), Value::Int(7)]],
            vec!["a".to_string(), "b".to_string(), "e1".to_string()],
        );
        let mut builds = vec![
            source(
                vec![
                    vec![Value::Int(1), Value::Int(10), Value::Int(11)],
                    vec![Value::Int(1), Value::Int(20), Value::Int(12)],
                ],
                vec!["a".to_string(), "c".to_string(), "e2".to_string()],
            ),
            source(
                vec![
                    vec![Value::Int(2), Value::Int(20), Value::Int(13)],
                    vec![Value::Int(2), Value::Int(40), Value::Int(14)],
                ],
                vec!["b".to_string(), "c".to_string(), "e3".to_string()],
            ),
        ];
        op.open(&mut probe, &mut builds).expect("open");
        let chunk = op
            .next(&mut probe, &mut builds)
            .expect("next")
            .expect("one chunk");
        assert_eq!(chunk.rows.len(), 1);
        assert_eq!(
            chunk.rows[0],
            vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(7),
                Value::Int(20),
                Value::Int(12),
                Value::Int(13),
            ]
        );
        assert!(op.next(&mut probe, &mut builds).expect("drain").is_none());
        op.close().expect("close");
    }

    #[test]
    fn disjoint_adjacency_emits_nothing() {
        let layout = Arc::new(SlotLayout::from_names(&triangle_spec().output_col_names));
        let mut op = WcoIntersectOperator::from_spec(
            &triangle_spec(),
            &crate::executor::base::MemoryBudget::new(
                crate::executor::base::MemoryBudget::DEFAULT_MAX,
            ),
            layout,
        );
        let mut probe = source(
            vec![vec![Value::Int(2), Value::Int(2), Value::Int(7)]],
            vec!["a".to_string(), "b".to_string(), "e1".to_string()],
        );
        let mut builds = vec![
            source(
                vec![vec![Value::Int(2), Value::Int(30), Value::Int(11)]],
                vec!["a".to_string(), "c".to_string(), "e2".to_string()],
            ),
            source(
                vec![vec![Value::Int(2), Value::Int(40), Value::Int(13)]],
                vec!["b".to_string(), "c".to_string(), "e3".to_string()],
            ),
        ];
        op.open(&mut probe, &mut builds).expect("open");
        assert!(op.next(&mut probe, &mut builds).expect("next").is_none());
        op.close().expect("close");
    }

    #[test]
    fn missing_bound_column_is_loud_error() {
        let layout = Arc::new(SlotLayout::from_names(&triangle_spec().output_col_names));
        let mut op = WcoIntersectOperator::from_spec(
            &triangle_spec(),
            &crate::executor::base::MemoryBudget::new(
                crate::executor::base::MemoryBudget::DEFAULT_MAX,
            ),
            layout,
        );
        let mut probe = source(
            vec![vec![Value::Int(1), Value::Int(2), Value::Int(7)]],
            vec!["a".to_string(), "b".to_string(), "e1".to_string()],
        );
        // e2 side lacks the intersect column `c`.
        let mut builds = vec![
            source(
                vec![vec![Value::Int(1), Value::Int(11)]],
                vec!["a".to_string(), "e2".to_string()],
            ),
            source(
                vec![vec![Value::Int(2), Value::Int(20), Value::Int(13)]],
                vec!["b".to_string(), "c".to_string(), "e3".to_string()],
            ),
        ];
        op.open(&mut probe, &mut builds).expect("open");
        let err = op.next(&mut probe, &mut builds).expect_err("must fail");
        assert!(err.to_string().contains("intersect column"));
    }

    #[test]
    fn reset_rebuilds_and_replays() {
        let layout = Arc::new(SlotLayout::from_names(&triangle_spec().output_col_names));
        let mut op = WcoIntersectOperator::from_spec(
            &triangle_spec(),
            &crate::executor::base::MemoryBudget::new(
                crate::executor::base::MemoryBudget::DEFAULT_MAX,
            ),
            layout,
        );
        let mut probe = source(
            vec![vec![Value::Int(1), Value::Int(2), Value::Int(7)]],
            vec!["a".to_string(), "b".to_string(), "e1".to_string()],
        );
        let mut builds = vec![
            source(
                vec![vec![Value::Int(1), Value::Int(20), Value::Int(12)]],
                vec!["a".to_string(), "c".to_string(), "e2".to_string()],
            ),
            source(
                vec![vec![Value::Int(2), Value::Int(20), Value::Int(13)]],
                vec!["b".to_string(), "c".to_string(), "e3".to_string()],
            ),
        ];
        op.open(&mut probe, &mut builds).expect("open");
        let first = op
            .next(&mut probe, &mut builds)
            .expect("next")
            .expect("rows");
        assert_eq!(first.rows.len(), 1);
        op.reset(&mut probe, &mut builds).expect("reset");
        let replayed = op
            .next(&mut probe, &mut builds)
            .expect("next")
            .expect("rows");
        assert_eq!(replayed.rows, first.rows);
        op.close().expect("close");
    }
}
