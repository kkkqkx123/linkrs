# WAL Parser I/O Model Analysis

> Status: 2026-07. Evaluates whether WAL recovery should switch from read-all-then-parse to
> streaming (record-by-record) I/O.

## Current I/O Model

```rust
let file_size = metadata.len() as usize;
let mut buffer = Vec::with_capacity(file_size);    // ① full file allocation
file.read_to_end(&mut buffer)?;                     // ② full file read
// ... then parse sequentially from the buffer ...
let payload = buffer[payload_start..payload_end].to_vec();  // ③ per-record copy
```

Both `LocalWalParser` and `ParallelWalParser` follow the same pattern: load the entire file
into a single `Vec<u8>` before parsing any records.

## WAL File Size Configuration

| Parameter       | Default | Description                                  |
|-----------------|---------|----------------------------------------------|
| `max_file_size` | 16 MB   | Rotation threshold — triggers `rotate()`     |
| `max_total_size`| 256 MB  | Total disk budget for all WAL files           |
| `truncate_size` | 4 MB    | Pre-allocation chunk size                    |

Under default settings, at most ~16 concurrent WAL files exist before cleanup deletes old ones.

## Peak Memory Usage

At crash recovery time with default config:

| Component             | Single file (16 MB) | All files (256 MB) |
|-----------------------|---------------------|--------------------|
| Raw file buffer       | 16 MB               | 256 MB             |
| ParsedWalEntry (+ payload) | ~17 MB (16k × 1KB) | ~280 MB        |
| **Peak total**        | **~33 MB**          | **~536 MB**        |

The parser is invoked only at startup (recovery, outbox projection catch-up, index generation),
never during normal runtime. The raw buffer is dropped after parsing completes.

## Two Consumers — Different Constraints

| Consumer | Access pattern | Can stream? |
|----------|---------------|-------------|
| `recover_with_applier` | Sequential replay of all entries | ✅ Yes |
| `collect_committed_transactions` | Scan forward for commit markers, then backtrack to find batch boundaries | ❌ No — requires random access |

`collect_committed_transactions` is used by outbox projection recovery and index catch-up.
It inherently needs two passes with random access to the entry list, making pure streaming
impossible without a full rewrite of the recovery protocol.

## Compression Interaction

WAL supports `WalCompression::Zstd` (writer side optional). For compressed WAL files:
- Only the header is uncompressed (fixed size, readable without decompression)
- The payload must be decompressed before use, which requires an output buffer of the
  full decompressed size
- Streaming would only save the raw file buffer (~16 MB per file), not the per-record
  `to_vec` copy — the decompressed payload still ends up heap-allocated

## Options

| Option | Memory saved | Complexity | Notes |
|--------|-------------|------------|-------|
| ① BufReader + record-by-record | ~16 MB raw buffer | Medium | Only `recover_with_applier` benefits; `collect_committed_transactions` needs second pass |
| ② mmap + on-demand paging | ~16 MB raw buffer | High | Requires mmap dependency; lifetime-parameterized `ParsedWalEntry` for zero-copy payloads |
| ③ Keep current approach | 0 | None | Correct, safe, and adequate for a single-node graph database |

## Conclusion

Streaming is **not needed**. Rationale in priority order:

1. **Hard file size cap** — 16 MB per file is well within modern system memory
2. **`collect_committed_transactions` requires random access** — a streaming-only approach
   either cannot support it or needs a two-pass redesign
3. **Parser runs once at startup** — not a hot path; low optimization priority
4. **Default peak of ~536 MB is acceptable** — for a single-node graph database; under
   typical workloads it is much lower (~33 MB per rotation)

If memory pressure at startup ever becomes a real bottleneck, the preferred improvement
path (in order of decreasing ROI) is:

1. Reduce `max_file_size` / `max_total_size` — config change, zero code
2. Switch to mmap for file reads — eliminates the raw buffer allocation, enables
   zero-copy for uncompressed payloads
3. BufReader streaming — only benefits `recover_with_applier`; requires `collect_committed_transactions` to buffer past entries or switch to a two-pass protocol

None of these are functional gaps or security issues — the current implementation is
correct and sufficient.
