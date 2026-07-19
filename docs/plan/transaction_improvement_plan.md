# Transaction Module Improvement Plan

## 1. Existing Problems Summary

### P1: Update Transaction Blocks All Reads [High]

**Location**: `mvcc.rs:444-493` `acquire_update_timestamp_with_timeout`

Update transactions subtract `thread_num` from `pending_reqs` (making it negative), then wait until all read/insert transactions drain. This blocks **all** reads and inserts — even unrelated graph traversals — until the update finishes.

**Impact**: Long-running reads (analytics queries, graph traversals) block writes entirely. Write starvation under read-heavy workloads.

**Observed in code**: `pending_reqs.fetch_sub(thread_num)` + wait loop at line 469-487.

---

### P2: WAL Group Commit Missing [High]

**Location**: `wal/writer/local.rs` (`LocalWalWriter`), `wal/writer/sync.rs`

Each transaction performs `fsync` independently via `SyncPolicy::EveryWrite`. No cross-transaction coordination exists. The `issue.md` mentions a planned `group_commit.rs` but the file was never created.

**Impact**: Under high-concurrent commit load, disk I/O becomes the bottleneck. Throughput is limited by `fsync` latency × transaction count.

---

### P3: WriteSet Conflict Detection Too Coarse [Medium]

**Location**: `types.rs:520-541` `WriteSet::has_conflict_with`

The conflict rule "edges sharing a source or destination vertex also conflict" is overly conservative. Two transactions inserting edges `(u1, u2)` and `(u1, u3)` would be flagged as conflicting despite no actual data conflict.

**Impact**: False conflict rate increases with graph density. Concurrent edge insertions from the same source vertex are serialized unnecessarily.

**Additionally**: The `has_conflict_with` edge-sharing check is O(n*m) — nested loop over edge sets.

---

### P4: Undo Log Memory Pressure [Medium]

**Location**: `undo_log.rs` (`UndoLogManager`)

15 `UndoLogEntry` variants stored in `Vec<UndoLogEntry>` in memory. Bulk imports (millions of edges) generate proportional Undo entries. Even after successful commit, memory is held until `clear_logs()`.

**Impact**: Large transactions can consume hundreds of MBs of memory for Undo entries that will only be used on abort.

---

### P5: WAL Poison Mechanism Missing [High]

**Location**: `wal/writer/local.rs`, `wal/writer/mod.rs`

After an I/O error, `LocalWalWriter` returns `WalError::IoError` but continues to accept writes. No global poisoned state prevents further operations. Subsequent transactions may append to a corrupt WAL.

**Impact**: Data inconsistency can propagate silently during partial I/O failures. Crash recovery may replay a corrupted log.

---

### P6: Ring Buffer Capacity Bottleneck [Medium]

**Location**: `mvcc.rs:25` `RING_BUF_SIZE = 1024 * 1024`

Fixed 1M-entry ring buffer. At 50% utilization a warning is logged; at capacity, new transactions block entirely. Long-running snapshot reads (backups, analytics) accumulate and fill the buffer.

**Impact**: Total system halt when buffer fills. No dynamic sizing or graceful degradation.

---

### P7: Commit/Abort Error Handling Asymmetry [Medium]

**Location**: `manager.rs:428-485` vs `manager.rs:569-613`

- `commit_transaction`: `sync_manager.commit_transaction_sync()` failure is **logged but ignored** — local commit proceeds.
- `abort_transaction_internal`: `sync_manager.rollback_transaction_sync()` failure causes immediate **abort failure** with timestamp rollback.

**Impact**: If sync_manager is temporarily unavailable during abort, the transaction enters an unrecoverable state — timestamps are rolled back but sync cleanup hasn't run.

---

### P8: Committing State Not Retryable [Medium]

**Location**: `manager.rs:452` `context.transition_to(TransactionState::Committing)`

Once in `Committing` state, the only transitions are to `Committed` or `Aborted`. If `commit_sink.commit_transaction()` fails transiently, the entire transaction must be aborted and the business logic retried.

**Impact**: Latency spikes under transient I/O pressure. Application-layer retry complexity increases.

---

### P9: Undo Rollback Before State Transition [Medium]

**Location**: `manager.rs:549-556` `abort_transaction_with_undo`

Undo entries are executed **before** `transition_to(Aborting)`. If Undo execution panics or returns an error, the transaction is still in `Active` state but storage has been partially rolled back.

**Impact**: Inconsistent transaction state — storage layer thinks the operation was reverted, but the transaction manager still considers it active.

---

### P10: No Dedicated Checkpoint Transaction Type [Low]

**Location**: `mvcc.rs`, `wal/checkpoint.rs`

Checkpoint operations share the same `VersionManager` as regular transactions. No mechanism to pause new writes during checkpoint, ensuring a consistent snapshot.

**Impact**: Checkpoint may capture data from transactions that started before the checkpoint but commit after it, creating potential inconsistency between WAL and checkpoint state.

---

### P11: Unsafe Panic on Snapshot Tracking Failure [Medium]

**Location**: `mvcc.rs:238`, `mvcc.rs:337`

`acquire_read_timestamp()` and `acquire_insert_timestamp()` (non-timeout variants) call `panic!` when `SnapshotTracker::add_snapshot()` fails. The timeout variants correctly return `None`.

**Impact**: A DashMap error (e.g., memory pressure) crashes the entire process instead of failing the transaction gracefully.

---

## 2. Design Decisions

| ID | Problem | Decision | Rationale |
|----|---------|----------|-----------|
| D1 | P1: Update blocks all reads | **Replace update-exclusive model with insert-level conflict detection only** | There is no fundamental need for update-only transactions on a single node. Treat all writes as inserts with write-set conflict detection. Eliminates the "update" category entirely, replacing it with: readers (snapshot) + writers (insert, multi-concurrent with conflict detection). This matches the actual concurrency requirement: only conflicting writes need serialization. |
| D2 | P2: No group commit | **Implement GroupCommitCoordinator with dual-sequence numbering** (`appended_commit_seq`, `durable_commit_seq`) + `Condvar` coordination. One thread performs `fsync`, others wait. | Proven pattern (Ladybug, PostgreSQL). Dual sequence numbers ensure durability guarantees are maintained even when multiple transactions share one fsync. |
| D3 | P3: Coarse conflict detection | **Remove shared-endpoint rule; keep vertex-level and edge-level conflict only**. Optimize edge overlap check with a HashSet of vertex IDs used as edge endpoints. | The shared-endpoint rule produces false conflicts for disjoint edge insertions. True conflict = same vertex modified OR same edge modified. If property-level conflict is needed in the future, add a `modified_properties: HashSet<(VertexId, ColumnId)>` field instead. |
| D4 | P4: Undo log memory | **Keep Undo log as-is but add memory-bounded disk overflow**. When Undo entries exceed a threshold (e.g., 10k), serialize older entries to a temp file. On abort, replay from file. On commit, delete temp file. | Complete replacement with MVCC version marks (Ladybug approach) requires deep changes to storage layer. Disk overflow is less invasive and preserves the existing UndoTarget trait. |
| D5 | P5: No WAL poison | **Add `AtomicBool poisoned` + `poison_reason: Mutex<Option<String>>>` to `LocalWalWriter`**. On any I/O error, set poisoned. All subsequent append/sync calls check poisoned first and return `WalError::Poisoned`. | Clean fail-fast pattern. Matches Ladybug's `throwIfPoisonedNoLock()`. Prevents silent corruption propagation. |
| D6 | P6: Ring buffer size | **Replace fixed-size ring buffer with a sparse bitmap (BTreeMap-based) tracking only active snapshots**. Remove `RING_BUF_SIZE` entirely. Use `SnapshotTracker`'s ordered BTreeMap directly for GC-safe timestamp computation. | The ring buffer exists only to track which timestamps have been released for GC advancement. A sparse structure is more memory-efficient and has no capacity limit. The `SnapshotTracker` already maintains an ordered BTreeMap for this purpose. |
| D7 | P7/P8: Commit/abort asymmetry + non-retryable committing | **Unify error handling**: both commit and abort treat sync_manager failures as retryable. Add `CommitRetry` state between `Active` and `Committing`. | Single error-handling policy reduces complexity. `CommitRetry` allows transient failures to be retried without aborting the entire transaction. |
| D8 | P9: Undo before state transition | **Transition to `Aborting` first, then execute Undo logs**. If Undo fails, transition to `Aborted` (best-effort) and return the error. | Ensures the transaction is never in `Active` state after storage mutations have been reversed. |
| D9 | P10: No checkpoint txn type | **Defer**: current checkpoint mechanism via `PersistenceCoordinator` is sufficient for single-node deployment. Document this as a known limitation for now. | Single-node graph DB doesn't need the complexity of checkpoint transactions. Can be revisited if multi-version checkpoint isolation becomes a requirement. |
| D10 | P11: Panic on snapshot failure | **Replace `panic!` with `Err(VersionManagerError::SnapshotTrackingFailed)`** in non-timeout variants, matching the timeout variants. | Consistency: all public APIs should return errors, not panic. The timeout variants already handle this correctly. |

---

## 3. Phased Implementation Plan

### Phase 1: Safety & Correctness Fixes (Low Risk, Immediate)

These are bugfixes that don't change architecture but fix correctness issues.

| Task | Files | Description |
|------|-------|-------------|
| 1.1 Replace snapshot panics with errors | `mvcc.rs:238,337` | Change `panic!` → return `Err(VersionManagerError::SnapshotTrackingFailed)` |
| 1.2 Fix abort ordering: state transition before undo | `manager.rs:549-556` | Move `transition_to(Aborting)` before `UndoLogRollback::execute_rollback()` |
| 1.3 Add WAL poison mechanism | `wal/writer/local.rs` | Add `poisoned: AtomicBool`, check in all write paths |
| 1.4 Remove shared-endpoint conflict rule | `types.rs:531-538` | Delete the O(n*m) edge-sharing loop in `has_conflict_with` |
| 1.5 Simplify conflict check with endpoint HashSet | `types.rs:520-541` | Add `edge_endpoints: HashSet<VertexId>` to `WriteSet` for O(1) lookup if needed later |

**Estimated effort**: 1-2 days  
**Risk**: Low (bugfixes + behavior change that reduces false conflicts)  
**Tests**: Unit tests for each fix; update conflict_integration_test

---

### Phase 2: Concurrency Model Refactoring (Medium Risk)

Core architecture changes to eliminate write starvation.

| Task | Files | Description |
|------|-------|-------------|
| 2.1 Remove `acquire_update_timestamp` / update transaction type | `mvcc.rs` | Delete `acquire_update_timestamp_with_timeout`, `release_update_timestamp`, `revert_update_timestamp`, `UpdateTimestampGuard` |
| 2.2 Remove `begin_update_transaction` from manager | `manager.rs:298-335` | Delete method; all writes use insert transaction |
| 2.3 Simplify `pending_reqs` to single-writer-counter model | `mvcc.rs` | Remove `pending_update_reqs`, `thread_num`; keep `pending_reqs` as simple active-count |
| 2.4 Simplify `release_*` timestamp logic | `mvcc.rs:402-510` | Merge `release_insert_timestamp` and `release_update_timestamp` into single `release_write_timestamp` |
| 2.5 Update all callers of removed APIs | `manager.rs`, `context.rs`, tests | Replace `UpdateTimestampGuard` with `InsertTimestampGuard` |
| 2.6 Remove `has_active_write_transaction` check | `manager.rs` | No longer needed — multiple write transactions allowed |

**Estimated effort**: 3-5 days  
**Risk**: Medium (removes an API surface, changes concurrency semantics)  
**Tests**: All existing tests must pass; add concurrent write tests verifying no starvation

**Key invariant after Phase 2**: All write transactions are "insert" transactions. Multiple writers run concurrently; conflicts detected by WriteSet at commit time. No write transaction ever blocks readers.

---

### Phase 3: GC & Timestamp Infrastructure (Medium Risk)

Replace the ring buffer with a more scalable design.

| Task | Files | Description |
|------|-------|-------------|
| 3.1 Remove `BitSet` ring buffer | `mvcc.rs:49-92, 26-27` | Delete `RING_BUF_SIZE`, `RING_INDEX_MASK`, `BitSet` struct |
| 3.2 Use `SnapshotTracker` for get_safe_gc_timestamp | `mvcc.rs:537-544` | Return `snapshot_tracker.min_active_snapshot()` directly |
| 3.3 Simplify `release_write_timestamp` | `mvcc.rs` | No ring buffer to update; just release snapshot + decrement count |
| 3.4 Remove ring-buffer-related fields from VersionManager | `mvcc.rs:151-162` | Remove `buffer: BitSet` |
| 3.5 Advance read_ts from SnapshotTracker's released set | `mvcc.rs` | On release, advance `read_ts` to the new minimum active snapshot timestamp (if monotonic) |

**Estimated effort**: 2-3 days  
**Risk**: Medium (changes timestamp lifecycle, affects GC behavior)  
**Tests**: Existing MVCC tests; add long-running test with thousands of timestamps

---

### Phase 4: Group Commit WAL (Medium Risk)

Implement cross-transaction fsync coordination.

| Task | Files | Description |
|------|-------|-------------|
| 4.1 Create `GroupCommitCoordinator` struct | New: `wal/writer/group_commit.rs` | Fields: `appended_seq`, `durable_seq`, `sync_in_progress: AtomicBool`, `commit_condvar: Condvar`, `commit_mutex: Mutex<()>` |
| 4.2 Implement `append_and_wait()` protocol | `group_commit.rs` | Thread acquires mutex → checks if sync already in progress → if yes, wait on condvar → if no, set `sync_in_progress=true` → perform fsync → update `durable_seq` → `notify_all()` → clear `sync_in_progress` |
| 4.3 Integrate into `LocalWalWriter::sync_data` | `local.rs` | Replace direct `file.sync_all()` with `coordinator.append_and_wait()` |
| 4.4 Expose durability API on `WalWriter` trait | `core/wal/traits.rs` | Add `fn wait_for_durable(&self, appended_seq: u64) -> WalResult<()>` to trait |
| 4.5 Wire `DurabilityLevel::Sync` to group commit | `local.rs` | `Sync` mode calls `wait_for_durable`; `Async` mode skips |

**Estimated effort**: 3-4 days  
**Risk**: Medium (adds shared synchronization state to WAL layer)  
**Tests**: Concurrent commit throughput test; crash durability test; poison interaction test

---

### Phase 5: Undo Log Disk Overflow (Low-Medium Risk)

| Task | Files | Description |
|------|-------|-------------|
| 5.1 Add `UndoLogConfig` with overflow threshold | `undo_log.rs` | `pub struct UndoLogConfig { pub memory_overflow_threshold: usize }` |
| 5.2 Implement `FileBackedUndoLog` wrapper | New: `undo_log/file_backed.rs` | Stores recent entries in memory, spills older entries to a temp file (postcard-serialized) |
| 5.3 Update `UndoLogManager` to use file-backed storage | `undo_log.rs` | `UndoLogManager` holds `FileBackedUndoLog` instead of `Vec<UndoLogEntry>` |
| 5.4 Implement LIFO replay from mixed memory+file source | `undo_log/file_backed.rs` | `execute_undo()` replays memory entries first (LIFO), then reads file entries in reverse |
| 5.5 Clean up temp files on commit/abort | `undo_log.rs` | Drop `FileBackedUndoLog` → delete temp file |

**Estimated effort**: 2-3 days  
**Risk**: Low-Medium (isolated to UndoLog module, existing trait preserved)  
**Tests**: Large-transaction memory usage test; abort-after-overflow correctness test

---

### Phase 6: Commit Retry & Error Handling Polish (Low Risk)

| Task | Files | Description |
|------|-------|-------------|
| 6.1 Add `CommitRetry` state to `TransactionState` | `types.rs:17-23` | `Active → Committing → CommitRetry → Committed/Aborted` |
| 6.2 Implement retry logic in `commit_transaction` | `manager.rs:428-485` | On retryable error, transition to `CommitRetry`, attempt up to N retries with backoff |
| 6.3 Unify sync_manager error handling in abort | `manager.rs:569-613` | On `rollback_transaction_sync()` failure, retry instead of immediate failure |
| 6.4 Add `commit_retry_attempts` to `TransactionManagerConfig` | `types.rs` | Configurable retry count (default: 3) |
| 6.5 Add `abort_retry_attempts` to `TransactionManagerConfig` | `types.rs` | Configurable retry count (default: 3) |

**Estimated effort**: 1-2 days  
**Risk**: Low (new state, backward-compatible behavior)  
**Tests**: Transient failure injection test with mock sync_manager

---

## 4. Dependencies & Execution Order

```
Phase 1 (Safety) ──→ Phase 2 (Concurrency) ──→ Phase 3 (GC/Timestamp)
                                            │
                                            └──→ Phase 4 (Group Commit)
                                            │
                                            └──→ Phase 5 (Undo Overflow)
                                            │
                                            └──→ Phase 6 (Retry/Errors)
```

- Phase 1 is independent, can start immediately
- Phases 2 and 3 can proceed in parallel after Phase 1
- Phase 4 depends on Phase 3 (needs simplified timestamp lifecycle)
- Phases 5 and 6 are independent of each other, can start after Phase 2

---

## 5. Files Touched Summary

| Phase | Primary Files | New Files |
|-------|--------------|-----------|
| 1 | `mvcc.rs`, `manager.rs`, `types.rs`, `wal/writer/local.rs` | — |
| 2 | `mvcc.rs`, `manager.rs`, `context.rs`, tests | — |
| 3 | `mvcc.rs`, `snapshot_tracker.rs` | — |
| 4 | `wal/writer/local.rs`, `core/wal/traits.rs` | `wal/writer/group_commit.rs` |
| 5 | `undo_log.rs` | `undo_log/file_backed.rs` |
| 6 | `types.rs`, `manager.rs` | — |

---

## 6. Success Metrics

| Metric | Before | After (Target) |
|--------|--------|----------------|
| Concurrent write throughput (txn/s with fsync) | ~500-1k (per-fsync bottleneck) | ~5-10k (batched fsync) |
| Write starvation under read load | Writes blocked completely | No blocking — writes proceed concurrently |
| False conflict rate (edge insertions) | High (shared endpoint) | Zero (exact entity matching only) |
| Memory per 1M operations (Undo) | ~200-400MB in-memory | ~20MB memory + disk spill |
| Recovery from WAL I/O error | Silent corruption propagation | Poison stops all writes cleanly |
| Transaction outage (transient commit sink failure) | Full abort + retry | Up to N retries from CommitRetry state |
