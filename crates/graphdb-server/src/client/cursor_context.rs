//! Session-scoped forward-only cursors over streaming results.
//!
//! A cursor holds one [`StreamingQueryResult`] execution handle plus a small
//! row buffer. Fetching pulls chunks until the requested page is full (or
//! the execution is exhausted); the frontend never buffers the full result.
//! Explicit close drops the handle promptly. Query deregistration runs
//! through the handle's drop callback, the single path shared with streaming
//! queries, which also covers idle-timeout sweeps, fetch-failure removals,
//! and session drops.

use parking_lot::RwLock;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::query::executor::streaming::StreamingQueryResult;
use graphdb_core::Value;

/// Cap on concurrently open cursors per session.
pub const MAX_CURSORS_PER_SESSION: usize = 16;
/// Idle cursors are reclaimed on the next cursor operation.
pub const CURSOR_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Largest page a single fetch returns.
pub const MAX_CURSOR_PAGE_SIZE: usize = 10_000;
/// Smallest page a single fetch returns.
pub const MIN_CURSOR_PAGE_SIZE: usize = 1;

/// One open cursor: an execution handle plus buffered rows.
pub struct CursorState {
    query: String,
    columns: Vec<String>,
    result: StreamingQueryResult,
    buffered: VecDeque<Vec<Value>>,
    exhausted: bool,
    last_active: Instant,
}

impl fmt::Debug for CursorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CursorState")
            .field("query", &self.query)
            .field("columns", &self.columns)
            .field("buffered", &self.buffered.len())
            .field("exhausted", &self.exhausted)
            .finish()
    }
}

/// One fetched page: column names plus row-major values.
pub struct CursorPage {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub has_more: bool,
}

/// Session-owned cursor registry, keyed by server-assigned cursor id.
#[derive(Debug)]
pub struct CursorContext {
    cursors: RwLock<HashMap<u64, CursorState>>,
    next_id: AtomicU64,
}

impl Default for CursorContext {
    fn default() -> Self {
        Self::new()
    }
}

impl CursorContext {
    pub fn new() -> Self {
        Self {
            cursors: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Register a new cursor over an already-started streaming result.
    pub fn open(
        &self,
        query: String,
        result: StreamingQueryResult,
        columns: Vec<String>,
    ) -> Result<u64, String> {
        self.sweep_expired();
        let mut cursors = self.cursors.write();
        if cursors.len() >= MAX_CURSORS_PER_SESSION {
            return Err(format!(
                "Too many open cursors (limit {MAX_CURSORS_PER_SESSION}); close idle ones first"
            ));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        cursors.insert(
            id,
            CursorState {
                query,
                columns,
                result,
                buffered: VecDeque::new(),
                exhausted: false,
                last_active: Instant::now(),
            },
        );
        Ok(id)
    }

    /// Pull chunks until `page_size` rows are ready or the execution ends.
    /// Blocking: callers run this off the async runtime.
    pub fn fetch(&self, cursor_id: u64, page_size: usize) -> Result<CursorPage, String> {
        self.sweep_expired();
        let page_size = page_size.clamp(MIN_CURSOR_PAGE_SIZE, MAX_CURSOR_PAGE_SIZE);
        // Snapshot the handle without holding the lock across blocking pulls.
        let (result, must_pull) = {
            let cursors = self.cursors.read();
            let cursor = cursors
                .get(&cursor_id)
                .ok_or_else(|| format!("Unknown cursor: {cursor_id}"))?;
            (
                cursor.result.clone(),
                !cursor.exhausted && cursor.buffered.len() < page_size,
            )
        };

        let mut pulled: Vec<(Vec<String>, Vec<Vec<Value>>)> = Vec::new();
        let mut pulled_rows: usize = 0;
        let mut now_exhausted = false;
        if must_pull {
            // Seed from the current buffer fill, read without a lock held:
            // the merge below re-checks existence, so a concurrent close
            // simply discards the pulled rows.
            loop {
                match result.next_chunk() {
                    Ok(Some(chunk)) => {
                        pulled_rows += chunk.rows.len();
                        pulled.push((chunk.col_names(), chunk.rows));
                        if pulled_rows >= page_size {
                            break;
                        }
                    }
                    Ok(None) => {
                        now_exhausted = true;
                        break;
                    }
                    Err(e) => {
                        self.cursors.write().remove(&cursor_id);
                        return Err(format!("Cursor query failed: {e}"));
                    }
                }
            }
        }

        // Merge under the write lock; a concurrent close wins over the fetch.
        let mut cursors = self.cursors.write();
        let cursor = cursors
            .get_mut(&cursor_id)
            .ok_or_else(|| format!("Unknown cursor: {cursor_id}"))?;
        for (names, rows) in pulled {
            if cursor.columns.is_empty() {
                cursor.columns = names;
            }
            cursor.buffered.extend(rows);
        }
        cursor.exhausted = cursor.exhausted || now_exhausted;
        let mut rows = Vec::with_capacity(page_size.min(cursor.buffered.len()));
        while rows.len() < page_size {
            match cursor.buffered.pop_front() {
                Some(row) => rows.push(row),
                None => break,
            }
        }
        cursor.last_active = Instant::now();
        let has_more = !(cursor.exhausted && cursor.buffered.is_empty());
        Ok(CursorPage {
            columns: cursor.columns.clone(),
            rows,
            has_more,
        })
    }

    /// Release a cursor; dropping the handle runs query deregistration.
    pub fn close(&self, cursor_id: u64) -> bool {
        self.sweep_expired();
        self.cursors.write().remove(&cursor_id).is_some()
    }

    /// Release every cursor; dropping each handle runs query deregistration.
    pub fn close_all(&self) {
        self.cursors.write().clear();
    }

    /// Number of currently open cursors.
    pub fn active_count(&self) -> usize {
        self.cursors.read().len()
    }

    /// Drop cursors idle past the timeout.
    fn sweep_expired(&self) {
        let mut cursors = self.cursors.write();
        cursors.retain(|_, cursor| cursor.last_active.elapsed() < CURSOR_IDLE_TIMEOUT);
    }
}
