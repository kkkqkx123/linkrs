# Transaction Module Refactor Plan

> Source analysis: `docs/analysis/transaction-module-analysis.md`
> Target crate: `crates/graphdb-transaction`

---

## Phase 1: Simplify State Machine & Recovery Model

**Goal**: Collapse 7 states to 4, eliminate `RecoveryRequired` as a zombie state.

### 1.1 Reduce `TransactionState` variants

Current: `Active | Committing | CommitRetry | Committed | Aborting | Aborted | RecoveryRequired`

Target: `Active | Committing | Aborting | Aborted`

- **`CommitRetry`)** → Internal retry loop inside `commit_transaction()`. The state remains `Committing` during retries; no external state needed.
- **`Committed`)** → Removed from the enum. Success is terminal: context is removed from `active_transactions`. The caller gets `Ok(())`.
- **`RecoveryRequired`)** → Replaced by a dedicated `RecoveryTask` struct (see 1.2).

**File**: `crates/graphdb-transaction/src/transaction/types.rs`

```rust
pub enum TransactionState {
    Active,
    Committing,
    Aborting,
    Aborted,
}
```

Update predicates:
- `can_commit()` → `Active`
- `can_abort()` → `Active | Committing | Aborting`
- `is_terminal()` → `Aborted` only

**File**: `crates/graphdb-transaction/src/transaction/context.rs`

Rewrite `transition_to()` CAS matrix:
- `Active → Committing | Aborting`
- `Committing → Aborting | Aborted`
- `Aborting → Aborted`

This eliminates all retry/recovery transitions. The state machine becomes acyclic (DAG-shaped) and trivially verifiable.

### 1.2 Recovery as a failed-commit cleanup path, not a state

When commit fails after the sink has persisted data (WAL durable, finalization failed), the context must be cleaned up. Instead of marking it `RecoveryRequired` and leaving it in `active_transactions`:

**New approach**: On finalize/undo-log cleanup failure:
1. Log the error with full context (txn_id, write_ts, commit_lsn).
2. Remove from `active_transactions`.
3. Transition to `Aborted`.
4. Emit a `TransactionEvent::CommitDurableButUnfinalized { txn_id, lsn}` (see Phase 3) so storage-layer components can complete cleanup asynchronously.

This eliminates `retry_recovery()`, `force_cleanup()`, and the `RecoveryRequired` branch in `shutdown()`.

**File**: `crates/graphdb-transaction/src/transaction/manager.rs`

- `commit_transaction()`: Replace the two `RecoveryRequired` transitions (lines 884, 894) with immediate abort + event emission.
- Delete `retry_recovery()` (line 1209).
- Delete `force_cleanup()` (line 1260).
- Simplify `shutdown()` — no special handling for `RecoveryRequired`.

### 1.3 Update error types

Remove `TransactionErrorKind::RecoveryRequired`. Add `CommitDurableButUnfinalized` for the new event path.

**File**: `crates/graphdb-transaction/src/transaction/error.rs`

---

## Phase 2: Introduce Lifecycle Event System & Commit/Rollback Callbacks

**Goal**: Decouple statistics, catalog versioning, sequence tracking, and storage notifications from the core commit/abort path.

### 2.1 Define `TransactionEvent` enum

**File**: `crates/graphdb-transaction/src/transaction/types.rs` (new types)

```rust
#[derive(Debug, Clone)]
pub enum TransactionEvent {
    Committed {
        txn_id: TransactionId,
        write_timestamp: u32,
        write_set: WriteSet,
        schema_catalog_version: u64,
    },
    Aborted {
        txn_id: TransactionId,
        write_timestamp: u32,
    },
    CommitDurableButUnfinalized {
        txn_id: TransactionId,
        write_timestamp: u32,
        commit_lsn: CommitLsn,
    },
}
```

### 2.2 Callback registration

**File**: `crates/graphdb-transaction/src/transaction/types.rs` or new `callbacks.rs`

```rust
pub type CommitCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
pub type RollbackCallback = Arc<dyn Fn(&TransactionEvent) + Send + Sync>;
```

Add to `TransactionManager`:

```rust
commit_callbacks: RwLock<Vec<CommitCallback>>,
rollback_callbacks: RwLock<Vec<RollbackCallback>>,
```

Public API:

```rust
pub fn register_commit_callback(&self, cb: CommitCallback);
pub fn register_rollback_callback(&self, cb: RollbackCallback);
```

### 2.3 Emit events in commit/abort paths

In `commit_transaction()` (after `active_transactions.remove`):
```rust
let event = TransactionEvent::Committed { ... };
for cb in self.commit_callbacks.read().iter() {
    cb(&event);
}
```

In `abort_transaction_internal()` (after `active_transactions.remove`):
```rust
let event = TransactionEvent::Aborted { ... };
for cb in self.rollback_callbacks.read().iter() {
    cb(&event);
}
```

### 2.4 Migrate stats collection to callbacks

`TransactionStats` should no longer be called directly from `commit_transaction()` / `abort_transaction_internal()`. Instead, register a stats-collection callback at `TransactionManager` construction time:

```rust
// In TransactionManager::new():
let stats_callbacks = self.stats.clone();
self.register_commit_callback(Arc::new(move |event| {
    if let TransactionEvent::Committed { .. } = event {
        stats_callbacks.record_txn_commit();
    }
}));
```

This removes all `self.stats.record_txn_*()` calls from the commit/abort hot path in `manager.rs`.

---

## Phase 3: Add CHECKPOINT as First-Class Transaction Type

**Goal**: Unify checkpoint and read/write transactions under a single type system.

### 3.1 Define `TransactionType` enum

**File**: `crates/graphdb-transaction/src/transaction/context.rs` (add field to `TransactionContext`)

```rust
pub enum TransactionType {
    ReadOnly,
    Write,
    Checkpoint,
}
```

Add `txn_type: TransactionType` to `TransactionContext`. The `TransactionOptions` already has `read_only: bool`; checkpoint transactions are constructed via a dedicated `begin_checkpoint_transaction()` which sets `txn_type = Checkpoint`.

### 3.2 Integrate `CheckpointTransaction` into the event system

Currently `CheckpointTransaction` is a standalone RAII struct in `manager.rs`. Refactor it to:
- Register as a `Checkpoint`-typed context in `active_transactions`.
- Participate in commit/abort event emission (so a checkpoint commit triggers storage-layer callbacks).
- Respond to `get_type()` queries for monitoring.

### 3.3 Expose `TransactionType` in `TransactionInfo`

Add a `txn_type: TransactionType` field to `TransactionInfo` so monitoring and admin tooling can distinguish checkpoint transactions.

---

## Phase 4: AUTO_COMMIT Mode

**Goal**: Support automatic single-statement transactions for simple queries.

### 4.1 Add `auto_commit` to `TransactionConfig`

```rust
pub struct TransactionConfig {
    // ... existing fields ...
    pub auto_commit: bool,  // default: false (explicit mode is current behavior)
}
```

### 4.2 Add `auto_commit_guard()` convenience method

```rust
impl TransactionManager {
    /// Begin a write transaction that auto-commits after `f` returns Ok,
    /// or auto-rolls-back on Err.
    pub fn auto_commit<F, T, E>(
        &self,
        f: F,
    ) -> Result<T, TransactionError>
    where
        F: FnOnce(&TransactionContext) -> Result<T, E>,
        E: Into<TransactionError>,
    {
        let ctx = self begin_insert_transaction(TransactionOptions::default())?;
        match f(&ctx) {
            Ok(result) => {
                self.commit_transaction(ctx.id)?;
                Ok(result)
            }
            Err(e) => {
                let _ = self.abort_transaction(ctx.id);
                Err(e.into())
            }
        }
    }
}
```

### 4.3 Execution-layer integration

Add `auto_commit: bool` to `TransactionExecution`. When true, each `Statement` commit auto-finalizes the underlying transaction.

---

## Phase 5: Conflict Detection Optimization

**Goal**: Reduce overhead of `check_write_set_conflict` for write-intensive workloads.

### 5.1 Add concurrency mode selection to `TransactionConfig`

```rust
pub enum ConcurrencyMode {
    Optimistic,    // current default — WriteSet conflict detection
    Pessimistic,   // acquire exclusive write lock at begin time
}

pub struct TransactionConfig {
    // ...
    pub concurrency_mode: ConcurrencyMode,
}
```

### 5.2 Implement pessimistic mode

Add a `write_exclusion: Mutex<()>` (or `RwLock`) to `TransactionManager`.

- `begin_insert_transaction()` with `ConcurrencyMode::Pessimistic` acquires `write_exclusion.lock()`.
- The guard is stored in `TransactionContext` and released on commit/abort.
- Conflict detection is skipped (`check_write_set_conflict` returns `Ok(())` immediately) because serialization is guaranteed by the mutex.

### 5.3 Optimize optimistic mode

Replace the linear scan in `check_write_set_conflict()` with a spatial index:
- Build a `Mutex<HashMap<VertexId, Vec<TransactionId>>>` of committed writes per vertex.
- Conflict check becomes: for each vertex/edge in the write set, look up concurrent transactions in O(1).
- This reduces conflict detection from O(N*M) to O(K) where K is the write-set size.

---

## Phase 6: Catalog Version & Sequence Integration

**Goal**: Ensure `schema_catalog_version` is correctly propagated and sequence changes are undoable.

### 6.1 Commit-event-driven catalog version increment

Register a commit callback on the catalog manager:

```rust
txn_mgr.register_commit_callback(Arc::new(|event| {
    if let TransactionEvent::Committed { schema_catalog_version, .. } = event {
        if schema_catalog_version > catalog.current_version() {
            catalog.increment_version();
        }
    }
}));
```

This replaces the current `schema_catalog_version` field that is stored but never read by an upstream consumer.

### 6.2 Sequence change undo support

Add new `UndoLogEntry` variants:

```rust
pub enum UndoLogEntry {
    // ... existing variants ...
    SequenceIncrement { sequence_name: String, previous_value: i64 },
    SequenceCreate { sequence_name: String },
}
```

Implement `undo()` for each:
- `SequenceIncrement`: restore `previous_value` to the sequence.
- `SequenceCreate`: drop the sequence.

Extend `TransactionMutationRecorder` with `record_sequence_change()`.

---

## Phase 7: Resource Budget Soft Limits & Monitoring Decoupling

**Goal**: Replace hard budget failures with soft-limit warnings; complete stats decoupling.

### 7.1 Soft-limit alerting

Add `budget_warning_threshold: f64` (0.0–1.0, default 0.8) to `TransactionConfig`.

In `record_mutation()` (context.rs line 685):
- If `mutation_count > max_mutation_count * threshold` → emit `TransactionEvent::BudgetWarning { ... }`.
- Only hard-fail at `mutation_count > max_mutation_count`.

Same for WAL bytes and undo bytes.

### 7.2 Complete stats decoupling

With Phase 2.4 done, `TransactionStats` no longer needs direct coupling. Remove `StatsManager` from `TransactionStats` and make all reporting go through the event system:

```rust
// Stats are collected purely via:
txn_mgr.register_commit_callback(stats_commit_handler);
txn_mgr.register_rollback_callback(stats_rollback_handler);
```

The `TransactionMonitor` becomes a thin snapshot reader over `active_transactions`, not a stats aggregator.

---

## Dependency Between Phases

```
Phase 1 (state machine) ──→ Phase 2 (events/callbacks) ──→ Phase 3 (checkpoint type)
                                          │
                                          ├──→ Phase 4 (auto_commit)
                                          │
                                          ├──→ Phase 6 (catalog/sequence)
                                          │
                                          └──→ Phase 7 (soft limits)
                                          
Phase 5 (concurrency mode)  ← independent → can run in parallel with Phase 2-4
```

---

## Migration & Compatibility Notes

- **Within this project** (no backward compat required per AGENTS.md): all changes are internal API changes.
- The public API surface of `TransactionManager` gains methods (`register_commit_callback`, `auto_commit`) but does not remove any until fully migrated.
- Phase 1 is breaking for any code that pattern-matches on `TransactionState::RecoveryRequired` or `CommitRetry` — but these are internal variants not exposed to user code outside the crate.
- Phases 2–7 are purely additive on the public API.
