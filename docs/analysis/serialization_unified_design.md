# Serialization Unified Design

Analysis date: 2026-07-18

## Design Goal

Establish a single, consistent rule for choosing serialization formats across the codebase. Format choice should be a function of the data's **role**, not historical accident.

---

## Format Taxonomy

| Format | When to Use | Examples |
|---|---|---|
| **postcard** (serde backend, COBS frames, compact binary) | Internal persistence: any file or BLOB that machines write and read. Compact, fast, version-tolerant with serde. | WAL entries, manifests, spill files, sync blobs |
| **postcard + wire version** | Any postcard file that should survive schema evolution. A `format_version: u32` field plus a `validate()` call at load time. | `IndexManifest`, `CheckpointManifest`, WAL sync intents |
| **prost** (generated) | Cross-process / cross-language wire protocol. gRPC service messages only. | `proto/graphdb.proto` service definitions |
| **hand-rolled binary** (LE/BE manual encoding) | Fixed-size headers, magic bytes, or where byte-order matters for comparison (index keys). | WAL file header `[u8; 64]`, storage section headers, `OrderedCodec` |
| **csv** (via `csv` crate) | External data interchange where CSV is the target format. Always use the `csv` crate's `Writer`, never manual escaping. | CLI import/export |
| **serde_json** | Human-inspectable output only: config files, stats dumps, slow-query logs, CLI display formatting. | `config.toml` display, `IndexTraffic` stats, explain plan JSON |
| **toml** | Human-authored configuration files. | `config.toml`, `{cli,connection}.toml` |

---

## Current Format Map

```
├── graphdb-core
│   ├── WAL headers          → hand-rolled binary (fixed [u8; N])
│   ├── WAL sync types       → postcard
│   ├── OrderedCodec (keys)  → hand-rolled BE binary (order-preserving)
│   ├── BloomFilter          → hand-rolled LE binary + murmur_hash3
│   └── Value codec           → hand-rolled type-tagged bytes [REDUNDANT — see §3]
│
├── graphdb-transaction
│   ├── WAL redo entries     → postcard (on outer struct)
│   └── Undo log properties   → value_to_bytes manual codec [REDUNDANT — see §3]
│
├── graphdb-storage
│   ├── Index manifest        → postcard + version ✅ (new)
│   ├── Generation build state → postcard + file ✅ (new)
│   ├── Storage section hdr   → hand-rolled LE binary (magic + version)
│   ├── Vertex column data    → hand-rolled LE binary per type
│   ├── Geography values      → serde_json ❌ (should be postcard)
│   ├── Sync commit state     → postcard + CRC ✅
│   └── Query spill rows      → postcard ✅
│
├── graphdb-sync
│   ├── IndexMutation (SQL BLOB) → postcard ✅
│   ├── CheckpointManifest    → postcard + version ✅
│   ├── Receiver commit state → serde_json ❌ (should be postcard)
│   └── Outbox intent          → postcard ✅
│
├── graphdb-query
│   ├── Spill rows            → postcard ✅
│   └── gRPC responses        → prost ✅
│
└── graphdb-cli
    ├── Config files          → toml ✅
    ├── Import                → csv crate ✅
    └── Export                → csv crate (planned) ✅
```

---

## Target State

### §1 — Geography in column_store

**Current**: `serde_json::to_vec(geo)` / `serde_json::from_slice::<Geography>(bytes)`

**Target**: `postcard::to_allocvec(geo)` / `postcard::from_bytes::<Geography>(bytes)`

**Rationale**: `Geography` already derives `Serialize`/`Deserialize`. Postcard produces ~3-5x smaller output than JSON for coordinate data. No human reads the column store directly — it is a machine-only binary file.

**Migration**: Column stores are rebuilt on every flush (immutable data). No backward-compat concern during dev.

### §2 — Checksum error handling

**Current**: `postcard::to_allocvec(&clone).unwrap_or_default()`

**Target**: `postcard::to_allocvec(&clone)?` (propagate error)

**Rationale**: Serialization of a derived struct should not fail. If it does, the caller should know rather than silently computing a checksum over empty bytes.

### §3 — Unify Value serialization

**Current**: Two codecs — `value_to_bytes()` (manual) for undo properties, postcard for WAL redo outer struct.

**Target**: Single postcard codec for all `Value` data.

**Approach**:
1. Change `UpdateVertexPropRedo.value` from `Vec<u8>` to `Value` (directly).
2. Change `InsertVertexRedo.properties` from `Vec<(String, Vec<u8>)>` to `Vec<(String, Value)>`.
3. Let postcard handle the entire struct uniformly.
4. Remove `value_to_bytes()` / `bytes_to_value()` from `codec.rs`.

**Rationale**: 
- Eliminates the maintenance burden of two parallel codecs.
- New Value variants (Geography, List, Map, etc.) work automatically.
- The manual codec silently encodes unknown variants as Null — a data-loss bug.
- Postcard is already the project standard for binary persistence.

**Risk**: WAL replay must handle old-format undo entries. Since the project is in dev (no backward-compat constraint), a clean cutover is acceptable. If a transition period is needed, a `wal_format_version` field can gate the decoder.

### §4 — Sync receiver state

**Current**: `serde_json::to_string(state)` → `*_receiver_state.json`

**Target**: `postcard::to_allocvec(state)` → `*_receiver_state.bin`

**Rationale**: Consistency with the rest of the sync subsystem. The state struct is small and machine-only. JSON provides no value here — no human inspects the receiver state file.

### §5 — CSV export

**Target**: Use `csv::Writer` from the `csv` crate (already a dependency).

**Rationale**: The crate handles quoting, escaping, and edge cases (embedded newlines, delimiters, quotes). Manual `writeln!` is guaranteed to produce corrupt output for non-trivial data.

---

## Format Selection Decision Tree

```
Is it a cross-process / cross-language protocol?
├── YES → prost (gRPC)
└── NO → Is it human-authored configuration?
    ├── YES → toml
    └── NO → Is it human-consumable output (stats, logs, display)?
        ├── YES → serde_json
        └── NO → Is it a fixed-size header or magic-bytes structure?
            ├── YES → hand-rolled binary (LE/BE as needed)
            └── NO → Is it a variable-length key where byte-order matters?
                ├── YES → OrderedCodec (BE, order-preserving)
                └── NO → postcard (default for all internal binary)
```

---

## Summary of Required Changes

| Change | Files | Effort |
|---|---|---|
| Geography → postcard | `column_store.rs` (write + read paths) | ~10 lines |
| Checksum error propagation | `checkpoint_manifest.rs` (2 lines) | ~2 lines |
| Unify Value codec | `codec.rs`, `redo.rs`, `writer.rs`, `ops.rs`, `recovery.rs` | ~100 lines |
| Sync state → postcard | `receiver.rs` (load + persist) | ~10 lines |
| CSV export → csv crate | `graphdb-cli` (future) | ~20 lines |

All changes are independent and can be applied incrementally. Items 1 and 2 are trivial and can be done immediately. Item 3 is the highest-value change (eliminates a correctness risk) but requires the most care.
