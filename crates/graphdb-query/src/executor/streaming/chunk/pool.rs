//! Row-buffer pool: reuse row `Vec<Value>` allocations across chunks.
//!
//! `DataChunk::clone` remains an explicit deep copy. New code prefers the
//! move-first discipline (`expand_visible_rows`, `take_indices`, `slice`).
//! The pool has two uses:
//! - Multiplicity expansion (`expand_rows_reusing_buffers` in `selection.rs`)
//!   clones repeated rows into recycled buffers instead of fresh allocations
//!   (one pool lock per expansion; the common `multiplicity == 1` path never
//!   touches the pool).
//! - Residual one-by-one rebuilds: callers take drained rows via
//!   [`DataChunk::take_rows_for_reuse`], return them with `release_rows`, and
//!   reacquire them with `acquire_rows` / `acquire_row` instead of allocating
//!   fresh vectors.
//!
//! The pool is process-global, lock-guarded, and bounded: at most
//! [`MAX_POOLED_ROWS`] row buffers are retained, and oversized row buffers
//! (capacity above [`MAX_POOLED_ROW_CAPACITY`]) are dropped instead of
//! retained so a single wide row cannot pin memory.

use graphdb_core::Value;
use parking_lot::Mutex;
use std::sync::LazyLock;

pub const MAX_POOLED_ROWS: usize = 64;
pub const MAX_POOLED_ROW_CAPACITY: usize = 1024;

static ROW_POOL: LazyLock<Mutex<Vec<Vec<Value>>>> = LazyLock::new(|| Mutex::new(Vec::new()));

/// Process-global row-buffer pool.
pub struct RowBufferPool;

impl RowBufferPool {
    /// Take one reusable row buffer, or an empty vector when the pool is dry.
    pub fn acquire_row() -> Vec<Value> {
        ROW_POOL.lock().pop().unwrap_or_default()
    }

    /// Return one row buffer after `clear`. Oversized or surplus buffers are dropped.
    pub fn release_row(row: &mut Vec<Value>) {
        row.clear();
        if row.capacity() > MAX_POOLED_ROW_CAPACITY {
            return;
        }
        let mut pool = ROW_POOL.lock();
        if pool.len() < MAX_POOLED_ROWS {
            pool.push(std::mem::take(row));
        }
    }

    /// Take up to `n` reusable row buffers.
    pub fn acquire_rows(n: usize) -> Vec<Vec<Value>> {
        let mut pool = ROW_POOL.lock();
        let take = n.min(pool.len());
        let mut out = Vec::with_capacity(n);
        for _ in 0..take {
            if let Some(row) = pool.pop() {
                out.push(row);
            }
        }
        out
    }

    /// Return drained rows to the pool, up to the capacity bound.
    pub fn release_rows(rows: &mut Vec<Vec<Value>>) {
        let mut pool = ROW_POOL.lock();
        for row in rows.iter_mut() {
            row.clear();
            if row.capacity() > MAX_POOLED_ROW_CAPACITY {
                continue;
            }
            if pool.len() >= MAX_POOLED_ROWS {
                break;
            }
            pool.push(std::mem::take(row));
        }
        rows.clear();
    }

    /// Current pooled buffer count (observability / tests).
    pub fn pool_len() -> usize {
        ROW_POOL.lock().len()
    }

    /// Drain the pool (tests only).
    #[cfg(test)]
    pub fn clear_for_test() {
        ROW_POOL.lock().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_release_reuses_capacity() {
        RowBufferPool::clear_for_test();
        let mut row = vec![Value::Int(1), Value::Int(2)];
        RowBufferPool::release_row(&mut row);
        assert!(row.is_empty());
        assert_eq!(RowBufferPool::pool_len(), 1);
        let reused = RowBufferPool::acquire_row();
        assert_eq!(RowBufferPool::pool_len(), 0);
        assert!(reused.capacity() >= 2);
    }

    #[test]
    fn pool_drops_beyond_capacity() {
        RowBufferPool::clear_for_test();
        let mut rows: Vec<Vec<Value>> = (0..(MAX_POOLED_ROWS + 16))
            .map(|_| Vec::<Value>::new())
            .collect();
        RowBufferPool::release_rows(&mut rows);
        assert!(rows.is_empty());
        assert_eq!(RowBufferPool::pool_len(), MAX_POOLED_ROWS);
        RowBufferPool::clear_for_test();
    }

    #[test]
    fn oversized_row_is_dropped() {
        RowBufferPool::clear_for_test();
        let mut row = Vec::with_capacity(MAX_POOLED_ROW_CAPACITY + 1);
        row.push(Value::Int(1));
        RowBufferPool::release_row(&mut row);
        assert_eq!(RowBufferPool::pool_len(), 0);
    }

    #[test]
    fn acquire_rows_partial_fill() {
        RowBufferPool::clear_for_test();
        let got = RowBufferPool::acquire_rows(4);
        assert!(got.is_empty());
        let mut rows = vec![vec![Value::Int(1)], vec![Value::Int(2)]];
        RowBufferPool::release_rows(&mut rows);
        let got = RowBufferPool::acquire_rows(4);
        assert_eq!(got.len(), 2);
        RowBufferPool::clear_for_test();
    }
}
