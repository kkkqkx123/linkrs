use super::ShardedVertexTable;

/// Maximum shard count per vertex table. Lifted from 16 to 256 so a single
/// vertex label's write concurrency is no longer pinned to 16 on large
/// machines; the interleaved ID encoding supports any power-of-two count up
/// to this value within the u32 ID space (~16.7M vertices per shard).
pub(super) const MAX_SHARDS: usize = 256;

/// Default segment slot width (2^14 slots per segment). Kept as the layout
/// default for newly created tables; opened tables use the width pinned in
/// their manifest instead of this constant.
pub const DEFAULT_SEGMENT_SLOTS_BITS: u32 = 14;

/// Versioned shard layout pinning the parameters of the global internal ID
/// encoding. Persisted in the table manifest and loaded at open; the
/// encode/decode arithmetic takes its parameters from here, never from
/// globals, so a layout change is always an explicit offline redistribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardLayout {
    /// Power-of-two shard count of the table.
    pub num_shards: usize,
    /// Base-2 logarithm of the slots per ID segment.
    pub segment_slots_bits: u32,
}

impl ShardLayout {
    /// Layout for a new table: shard count clamped to the supported range
    /// and rounded to a power of two, default segment width.
    pub fn for_new_table(num_shards: usize) -> Self {
        Self {
            num_shards: num_shards.clamp(1, MAX_SHARDS).next_power_of_two(),
            segment_slots_bits: DEFAULT_SEGMENT_SLOTS_BITS,
        }
    }

    /// Slots per ID segment under this layout.
    pub fn segment_slots(&self) -> u32 {
        1 << self.segment_slots_bits
    }

    /// Slot mask within one segment.
    pub fn segment_slots_mask(&self) -> u32 {
        self.segment_slots() - 1
    }
}

/// Adapt the default shard count to the available CPU parallelism, clamped to
/// `MAX_SHARDS` and rounded down to a power of two (required by the shard-ID
/// bit encoding). Fallback to 1 if the platform cannot report parallelism.
pub(super) fn default_num_shards() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, MAX_SHARDS)
        .next_power_of_two()
}
// Internal ID layout: `internal_id = (segment << K) | slot`.
// A shard's i-th segment is `shard + i * num_shards`, so segments are
// interleaved across shards and unique per shard, and the shard is recovered
// as `(id >> K) % num_shards` — pure bit arithmetic, no mapping table.
// Decoding the slot requires knowing which segment ordinal it belongs to:
// `local_id = (segment / num_shards) * 2^K + slot`, which keeps the
// shard-local ids dense (0..n) exactly as the underlying VertexTable rows.
//
// Because each shard's i-th segment is fixed at `shard + i * num_shards`,
// IDs stay proportional to the vertex count: with K = 14 and balanced shards,
// N vertices occupy IDs up to ~N + num_shards * 16384, versus the 16x blowup
// of the previous shard-in-low-bits encoding. K=14 (16384 slots) trades a
// slightly larger ID tail for fewer segments and lower fragmentation rate;
// combined with lazy ID recycling, the effective reclaimed space stays high.
pub(super) const SEGMENT_SLOTS_BITS: u32 = 14;
pub(super) const SEGMENT_SLOTS: u32 = 1 << SEGMENT_SLOTS_BITS;
pub(super) const SEGMENT_SLOTS_MASK: u32 = SEGMENT_SLOTS - 1;

pub(super) fn encode_id(shard: usize, local_id: u32, layout: ShardLayout) -> u32 {
    debug_assert!(shard < layout.num_shards);
    debug_assert!(local_id <= u32::MAX / layout.num_shards as u32);
    let segment =
        shard as u32 + (local_id >> layout.segment_slots_bits) * layout.num_shards as u32;
    (segment << layout.segment_slots_bits) | (local_id & layout.segment_slots_mask())
}

pub(super) fn decode_id(global_id: u32, layout: ShardLayout) -> (usize, u32) {
    let segment = global_id >> layout.segment_slots_bits;
    let shard = (segment % layout.num_shards as u32) as usize;
    let local_id = (segment / layout.num_shards as u32) << layout.segment_slots_bits
        | (global_id & layout.segment_slots_mask());
    (shard, local_id)
}

pub(super) fn fxhash(s: &str) -> u64 {
    let mut hash: u64 = 0;
    for byte in s.bytes() {
        hash = hash.wrapping_mul(0x517cc1b727220a95);
        hash ^= byte as u64;
    }
    hash
}

pub(super) fn fxhash_i64(n: i64) -> u64 {
    let mut hash: u64 = 0x517cc1b727220a95;
    hash ^= n as u64;
    hash
}

impl ShardedVertexTable {
    pub(super) fn shard_index_by_str(&self, external_id: &str) -> usize {
        let hash = fxhash(external_id);
        (hash as usize) & (self.layout.num_shards - 1)
    }

    pub(super) fn shard_index_by_i64(&self, external_id: i64) -> usize {
        let hash = fxhash_i64(external_id);
        (hash as usize) & (self.layout.num_shards - 1)
    }

    /// Record an allocation of `local_id` in shard `idx` and encode it as a
    /// global id.
    pub(super) fn record_allocation(&self, idx: usize, local_id: u32) -> u32 {
        encode_id(idx, local_id, self.layout)
    }

    pub(super) fn decode_id(&self, global_id: u32) -> (usize, u32) {
        decode_id(global_id, self.layout)
    }

    pub(super) fn encode_id(&self, shard: usize, local_id: u32) -> u32 {
        encode_id(shard, local_id, self.layout)
    }

    pub fn num_shards(&self) -> usize {
        self.layout.num_shards
    }

    /// Versioned shard layout this table encodes global ids with.
    pub fn layout(&self) -> ShardLayout {
        self.layout
    }
}
