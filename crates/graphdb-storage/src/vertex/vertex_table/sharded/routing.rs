use super::ShardedVertexTable;

/// Maximum shard count per vertex table. Lifted from 16 to 256 so a single
/// vertex label's write concurrency is no longer pinned to 16 on large
/// machines; the interleaved ID encoding supports any power-of-two count up
/// to this value within the u32 ID space (~16.7M vertices per shard).
pub(super) const MAX_SHARDS: usize = 256;

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

pub(super) fn encode_id(shard: usize, local_id: u32, num_shards: usize) -> u32 {
    debug_assert!(shard < num_shards);
    debug_assert!(local_id <= u32::MAX / num_shards as u32);
    let segment = shard as u32 + (local_id >> SEGMENT_SLOTS_BITS) * num_shards as u32;
    (segment << SEGMENT_SLOTS_BITS) | (local_id & SEGMENT_SLOTS_MASK)
}

pub(super) fn decode_id(global_id: u32, num_shards: usize) -> (usize, u32) {
    let segment = global_id >> SEGMENT_SLOTS_BITS;
    let shard = (segment % num_shards as u32) as usize;
    let local_id =
        (segment / num_shards as u32) << SEGMENT_SLOTS_BITS | (global_id & SEGMENT_SLOTS_MASK);
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
        (hash as usize) & (self.num_shards - 1)
    }

    pub(super) fn shard_index_by_i64(&self, external_id: i64) -> usize {
        let hash = fxhash_i64(external_id);
        (hash as usize) & (self.num_shards - 1)
    }

    /// Record an allocation of `local_id` in shard `idx` and encode it as a
    /// global id.
    pub(super) fn record_allocation(&self, idx: usize, local_id: u32) -> u32 {
        encode_id(idx, local_id, self.num_shards)
    }

    pub(super) fn decode_id(&self, global_id: u32) -> (usize, u32) {
        decode_id(global_id, self.num_shards)
    }

    pub(super) fn encode_id(&self, shard: usize, local_id: u32) -> u32 {
        encode_id(shard, local_id, self.num_shards)
    }

    pub fn num_shards(&self) -> usize {
        self.num_shards
    }
}
