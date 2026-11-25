//! Contract boundary for a proven ordered existence source.
//!
//! The source emits one ascending, unique integer key per row and at most
//! `limit` rows while its snapshot generation is stable. This operator keeps
//! that shape without buffering, and still applies the row bound if the
//! source had to disable early stopping after drift.

use super::*;

pub(super) fn handle(
    op: &mut UnaryOperator,
    input: &mut StreamingExecutor,
) -> Result<Option<DataChunk>, QueryError> {
    let stats = op.runtime.as_ref().map(|rt| rt.columnar_stats());
    let UnaryOperatorKind::OrderedExistenceContract {
        limit,
        emitted,
        last_key,
    } = &mut op.kind
    else {
        return Err(QueryError::execution(
            "ordered_existence_contract::handle called for a different operator".to_string(),
        ));
    };
    if *emitted >= *limit {
        return Ok(None);
    }

    loop {
        let Some(mut chunk) = input.advance()? else {
            return Ok(None);
        };
        let visible = chunk.visible_indices();
        let mut kept_indices = Vec::with_capacity(visible.len());
        let mut changed = false;
        for index in visible.iter().copied() {
            let Some(row) = chunk.rows.get(index) else {
                return Err(QueryError::execution(
                    "OrderedExistenceContract received a short chunk".to_string(),
                ));
            };
            let Some(key) = key_of(row.first()) else {
                return Err(QueryError::execution(
                    "OrderedExistenceContract requires an integer primary key".to_string(),
                ));
            };
            if let Some(previous) = *last_key {
                if key < previous {
                    return Err(QueryError::execution(format!(
                        "OrderedExistenceContract observed a descending key after {previous}: {key}"
                    )));
                }
                if key == previous {
                    if let Some(stats) = &stats {
                        stats
                            .ordered_existence_contract_violations
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    changed = true;
                    continue;
                }
            }
            if *emitted as usize + kept_indices.len() >= *limit as usize {
                if let Some(stats) = &stats {
                    stats
                        .ordered_existence_contract_discarded
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                changed = true;
                continue;
            }
            *last_key = Some(key);
            kept_indices.push(index);
        }
        *emitted += kept_indices.len() as u32;
        if kept_indices.is_empty() {
            continue;
        }
        if changed {
            return Ok(Some(chunk.with_selection(kept_indices)));
        }
        if visible.len() == chunk.rows.len() {
            let _ = chunk.take_selection();
        }
        return Ok(Some(chunk));
    }
}

fn key_of(value: Option<&Value>) -> Option<i64> {
    match value? {
        Value::BigInt(value) => Some(*value),
        Value::Int(value) => Some(*value as i64),
        Value::SmallInt(value) => Some(*value as i64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::base::MemoryBudget;
    use crate::executor::streaming::operators::source_operator::{
        SourceOperator, SourceOperatorKind,
    };
    use crate::executor::streaming::runtime::{ExecutionRuntime, QueryIdentity};
    use crate::executor::streaming::slot::SlotLayout;

    fn runtime() -> Arc<ExecutionRuntime> {
        Arc::new(ExecutionRuntime::new(
            QueryIdentity::default(),
            MemoryBudget::new(1024 * 1024),
            None,
            crate::executor::base::SearchContext::default(),
        ))
    }

    fn source(rows: Vec<Vec<Value>>) -> Box<StreamingExecutor> {
        Box::new(StreamingExecutor::Source(
            crate::executor::streaming::operators::base::OperatorBase::new(0),
            SourceOperator::new(
                SourceOperatorKind::ScanVertices {
                    buffer: rows,
                    current_index: 0,
                    col_names: vec!["p.id".to_string()],
                },
                Arc::new(SlotLayout::new(Vec::new())),
            ),
        ))
    }

    fn values(chunk: &DataChunk) -> Vec<i64> {
        chunk
            .visible_indices()
            .into_iter()
            .map(|index| match &chunk.rows[index][0] {
                Value::BigInt(value) => *value,
                other => panic!("unexpected key {other:?}"),
            })
            .collect()
    }

    #[test]
    fn contract_enforces_bound_without_buffering() {
        let rt = runtime();
        let mut input = source((1..=10).map(|id| vec![Value::BigInt(id)]).collect());
        input.set_runtime(Some(rt.clone()));
        let mut op = UnaryOperator::from_spec(
            &crate::executor::streaming::operators::spec::UnarySpec::OrderedExistenceContract {
                limit: 4,
            },
            Arc::new(SlotLayout::from_names(&["p.id".to_string()])),
        );
        op.inject_context(Some(&rt), OperatorConfig::default());
        op.open(&mut input).expect("open contract");

        let mut output = Vec::new();
        while let Some(chunk) = op.next(&mut input).expect("pull contract") {
            output.extend(values(&chunk));
        }
        assert_eq!(output, vec![1, 2, 3, 4]);
        assert_eq!(
            rt.columnar_stats()
                .ordered_existence_contract_discarded
                .load(std::sync::atomic::Ordering::Relaxed),
            6
        );
    }

    #[test]
    fn contract_drops_duplicate_keys_and_records_diagnostic() {
        let rt = runtime();
        let mut input = source(vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(1)],
            vec![Value::BigInt(2)],
        ]);
        input.set_runtime(Some(rt.clone()));
        let mut op = UnaryOperator::from_spec(
            &crate::executor::streaming::operators::spec::UnarySpec::OrderedExistenceContract {
                limit: 10,
            },
            Arc::new(SlotLayout::from_names(&["p.id".to_string()])),
        );
        op.inject_context(Some(&rt), OperatorConfig::default());
        op.open(&mut input).expect("open contract");

        let mut output = Vec::new();
        while let Some(chunk) = op.next(&mut input).expect("pull contract") {
            output.extend(values(&chunk));
        }
        assert_eq!(output, vec![1, 2]);
        assert_eq!(
            rt.columnar_stats()
                .ordered_existence_contract_violations
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn contract_rejects_descending_keys() {
        let rt = runtime();
        let mut input = source(vec![
            vec![Value::BigInt(1)],
            vec![Value::BigInt(3)],
            vec![Value::BigInt(2)],
        ]);
        input.set_runtime(Some(rt.clone()));
        let mut op = UnaryOperator::from_spec(
            &crate::executor::streaming::operators::spec::UnarySpec::OrderedExistenceContract {
                limit: 10,
            },
            Arc::new(SlotLayout::from_names(&["p.id".to_string()])),
        );
        op.inject_context(Some(&rt), OperatorConfig::default());
        op.open(&mut input).expect("open contract");

        assert!(op.next(&mut input).is_err());
    }
}
