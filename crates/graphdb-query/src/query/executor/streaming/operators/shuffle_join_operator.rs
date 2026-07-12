use std::collections::HashMap;

use crate::core::error::QueryError;
use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::base::MemoryTracker;
use crate::query::executor::streaming::chunk::DataChunk;
use crate::query::executor::streaming::executor::StreamingExecutor;
use crate::query::executor::streaming::join_helpers::{
    build_combined_layout_from_schemas, evaluate_join_key, evaluate_residual_condition,
};
use crate::query::executor::streaming::operator_base::OperatorBase;

const CHUNK_SIZE: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashJoinKind {
    Inner,
    Left,
}

#[derive(Debug)]
struct Bucket {
    left_rows: Vec<Vec<Value>>,
    right_rows: Vec<Vec<Value>>,
    right_hash: Option<HashMap<Vec<Value>, Vec<usize>>>,
    current_probe_index: usize,
}

#[derive(Debug)]
enum ShufflePhase {
    Building,
    Processing(usize),
    Exhausted,
}

#[derive(Debug)]
pub struct HashShuffleJoinOperator {
    pub join_kind: HashJoinKind,
    pub left_key_expressions: Vec<Expression>,
    pub right_key_expressions: Vec<Expression>,
    pub join_condition: Option<Expression>,
    pub bucket_count: usize,
    pub left_schema: Vec<String>,
    pub right_schema: Vec<String>,
    pub memory_tracker: MemoryTracker,
    buckets: Vec<Bucket>,
    phase: ShufflePhase,
}

impl HashShuffleJoinOperator {
    pub fn new(
        join_kind: HashJoinKind,
        left_key_expressions: Vec<Expression>,
        right_key_expressions: Vec<Expression>,
        join_condition: Option<Expression>,
        bucket_count: usize,
        left_schema: Vec<String>,
        right_schema: Vec<String>,
        memory_tracker: MemoryTracker,
    ) -> Self {
        let mut buckets = Vec::with_capacity(bucket_count);
        for _ in 0..bucket_count {
            buckets.push(Bucket {
                left_rows: Vec::new(),
                right_rows: Vec::new(),
                right_hash: None,
                current_probe_index: 0,
            });
        }
        Self {
            join_kind,
            left_key_expressions,
            right_key_expressions,
            join_condition,
            bucket_count,
            left_schema,
            right_schema,
            memory_tracker,
            buckets,
            phase: ShufflePhase::Building,
        }
    }

    pub fn open(
        &mut self,
        base: &mut OperatorBase,
        left_trees: &mut [StreamingExecutor],
        right_trees: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        for tree in left_trees.iter_mut() {
            tree.open()?;
        }
        for tree in right_trees.iter_mut() {
            tree.open()?;
        }
        base.opened = true;
        Ok(())
    }

    fn hash_row_to_bucket(
        row: &[Value],
        col_names: &[String],
        key_expressions: &[Expression],
        bucket_count: usize,
    ) -> Result<usize, QueryError> {
        if key_expressions.is_empty() || bucket_count == 0 {
            return Ok(0);
        }
        let key = evaluate_join_key(row, col_names, key_expressions)?;
        let hash = Self::hash_values(&key);
        Ok((hash % bucket_count as u64) as usize)
    }

    fn hash_values(values: &[Value]) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        values.hash(&mut hasher);
        hasher.finish()
    }

    fn read_all_trees(
        &mut self,
        base: &mut OperatorBase,
        left_trees: &mut [StreamingExecutor],
        right_trees: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        for tree in left_trees.iter_mut() {
            while let Some(chunk) = tree.advance()? {
                base.ensure_not_cancelled()?;
                let col_names = chunk.col_names();
                for row in &chunk.rows {
                    self.memory_tracker.try_reserve_row(row)?;
                    let bucket_idx = Self::hash_row_to_bucket(
                        row,
                        &col_names,
                        &self.left_key_expressions,
                        self.bucket_count,
                    )?;
                    self.buckets[bucket_idx].left_rows.push(row.clone());
                }
            }
        }
        for tree in right_trees.iter_mut() {
            while let Some(chunk) = tree.advance()? {
                base.ensure_not_cancelled()?;
                let col_names = chunk.col_names();
                for row in &chunk.rows {
                    self.memory_tracker.try_reserve_row(row)?;
                    let bucket_idx = Self::hash_row_to_bucket(
                        row,
                        &col_names,
                        &self.right_key_expressions,
                        self.bucket_count,
                    )?;
                    self.buckets[bucket_idx].right_rows.push(row.clone());
                }
            }
        }
        Ok(())
    }

    pub fn next(
        &mut self,
        base: &mut OperatorBase,
        left_trees: &mut [StreamingExecutor],
        right_trees: &mut [StreamingExecutor],
    ) -> Result<Option<DataChunk>, QueryError> {
        loop {
            match self.phase {
                ShufflePhase::Building => {
                    self.read_all_trees(base, left_trees, right_trees)?;
                    self.phase = ShufflePhase::Processing(0);
                }
                ShufflePhase::Processing(current) => {
                    if current >= self.bucket_count {
                        self.phase = ShufflePhase::Exhausted;
                        return Ok(None);
                    }
                    if let Some(chunk) = self.process_bucket_chunked(current)? {
                        return Ok(Some(chunk));
                    }
                    self.release_bucket(current);
                    self.phase = ShufflePhase::Processing(current + 1);
                }
                ShufflePhase::Exhausted => return Ok(None),
            }
        }
    }

    fn release_bucket(&mut self, idx: usize) {
        let bucket = &mut self.buckets[idx];
        bucket.left_rows.clear();
        bucket.right_rows.clear();
        bucket.right_hash = None;
        bucket.current_probe_index = 0;
    }

    fn process_bucket_chunked(&mut self, bucket_idx: usize) -> Result<Option<DataChunk>, QueryError> {
        let bucket = &mut self.buckets[bucket_idx];

        if bucket.right_hash.is_none() && !bucket.right_rows.is_empty() {
            let mut hash: HashMap<Vec<Value>, Vec<usize>> = HashMap::new();
            for (idx, row) in bucket.right_rows.iter().enumerate() {
                let key = evaluate_join_key(row, &self.right_schema, &self.right_key_expressions)?;
                hash.entry(key).or_default().push(idx);
            }
            bucket.right_hash = Some(hash);
        }

        let right_width = self.right_schema.len();
        let mut chunk_rows = Vec::with_capacity(CHUNK_SIZE);

        while bucket.current_probe_index < bucket.left_rows.len() {
            let left_row = &bucket.left_rows[bucket.current_probe_index];

            if self.left_key_expressions.is_empty() {
                for right_row in &bucket.right_rows {
                    if Self::check_condition(&self.join_condition, left_row, right_row, &self.left_schema, &self.right_schema)? {
                        let mut joined = left_row.clone();
                        joined.extend(right_row.clone());
                        chunk_rows.push(joined);
                        if chunk_rows.len() >= CHUNK_SIZE {
                            let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
                            bucket.current_probe_index += 1;
                            return Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)));
                        }
                    }
                }
                if self.join_kind == HashJoinKind::Left && bucket.right_rows.is_empty() {
                    let mut joined = left_row.clone();
                    joined.extend(vec![Value::Null(crate::core::value::NullType::Null); right_width]);
                    chunk_rows.push(joined);
                    if chunk_rows.len() >= CHUNK_SIZE {
                        let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
                        bucket.current_probe_index += 1;
                        return Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)));
                    }
                }
            } else if let Some(hash) = bucket.right_hash.as_ref() {
                let probe_key = evaluate_join_key(left_row, &self.left_schema, &self.left_key_expressions)?;
                if let Some(matching_indices) = hash.get(&probe_key) {
                    for &right_idx in matching_indices {
                        let right_row = &bucket.right_rows[right_idx];
                        if Self::check_condition(&self.join_condition, left_row, right_row, &self.left_schema, &self.right_schema)? {
                            let mut joined = left_row.clone();
                            joined.extend(right_row.clone());
                            chunk_rows.push(joined);
                            if chunk_rows.len() >= CHUNK_SIZE {
                                let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
                                bucket.current_probe_index += 1;
                                return Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)));
                            }
                        }
                    }
                } else if self.join_kind == HashJoinKind::Left {
                    let mut joined = left_row.clone();
                    joined.extend(vec![Value::Null(crate::core::value::NullType::Null); right_width]);
                    chunk_rows.push(joined);
                    if chunk_rows.len() >= CHUNK_SIZE {
                        let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
                        bucket.current_probe_index += 1;
                        return Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)));
                    }
                }
            } else if self.join_kind == HashJoinKind::Left {
                let mut joined = left_row.clone();
                joined.extend(vec![Value::Null(crate::core::value::NullType::Null); right_width]);
                chunk_rows.push(joined);
                if chunk_rows.len() >= CHUNK_SIZE {
                    let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
                    bucket.current_probe_index += 1;
                    return Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)));
                }
            }

            bucket.current_probe_index += 1;
        }

        if chunk_rows.is_empty() {
            Ok(None)
        } else {
            let layout = build_combined_layout_from_schemas(&self.left_schema, &self.right_schema);
            Ok(Some(DataChunk::new_with_layout(chunk_rows, layout)))
        }
    }

    fn check_condition(
        join_condition: &Option<Expression>,
        left_row: &[Value],
        right_row: &[Value],
        left_schema: &[String],
        right_schema: &[String],
    ) -> Result<bool, QueryError> {
        match join_condition {
            None => Ok(true),
            Some(condition) => evaluate_residual_condition(
                condition,
                left_row,
                right_row,
                left_schema,
                right_schema,
            ),
        }
    }

    pub fn stop(
        &mut self,
        _base: &mut OperatorBase,
        left_trees: &mut [StreamingExecutor],
        right_trees: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        let mut first_error = None;
        for tree in left_trees.iter_mut() {
            if let Err(e) = tree.stop() {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
        for tree in right_trees.iter_mut() {
            if let Err(e) = tree.stop() {
                if first_error.is_none() {
                    first_error = Some(e);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    pub fn close(
        &mut self,
        base: &mut OperatorBase,
        left_trees: &mut [StreamingExecutor],
        right_trees: &mut [StreamingExecutor],
    ) -> Result<(), QueryError> {
        if base.opened {
            self.memory_tracker.reset();
            self.buckets.clear();
            let mut first_error = None;
            for tree in left_trees.iter_mut() {
                if let Err(e) = tree.close() {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
            for tree in right_trees.iter_mut() {
                if let Err(e) = tree.close() {
                    if first_error.is_none() {
                        first_error = Some(e);
                    }
                }
            }
            base.opened = false;
            first_error.map_or(Ok(()), Err)
        } else {
            Ok(())
        }
    }

    pub fn memory_tracker(&self) -> &MemoryTracker {
        &self.memory_tracker
    }
}
