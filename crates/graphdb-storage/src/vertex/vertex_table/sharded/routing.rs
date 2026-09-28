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

/// Width in bits of the global internal ID space.
const GLOBAL_ID_BITS: u32 = 32;

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
    /// Total segments addressable by global IDs under this layout: the ID
    /// bits above the slot field. Persisted (not re-derived) so future
    /// layouts can evolve the ID width without silently re-deriving it.
    pub total_segments: u32,
}

impl ShardLayout {
    /// Layout for a new table: shard count clamped to the supported range
    /// and rounded to a power of two, default segment width.
    pub fn for_new_table(num_shards: usize) -> Self {
        let segment_slots_bits = DEFAULT_SEGMENT_SLOTS_BITS;
        Self {
            num_shards: num_shards.clamp(1, MAX_SHARDS).next_power_of_two(),
            segment_slots_bits,
            total_segments: 1u32 << (GLOBAL_ID_BITS - segment_slots_bits),
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

    /// Whether the persisted parameters describe one coherent ID encoding:
    /// a slot width inside the ID space and a segment total matching the
    /// bits the encoding actually addresses.
    pub fn is_consistent(&self) -> bool {
        self.segment_slots_bits >= 1
            && self.segment_slots_bits < GLOBAL_ID_BITS
            && self.total_segments == 1u32 << (GLOBAL_ID_BITS - self.segment_slots_bits)
            && self.num_shards.is_power_of_two()
            && self.num_shards <= MAX_SHARDS
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
pub(super) fn encode_id(shard: usize, local_id: u32, layout: ShardLayout) -> u32 {
    try_encode_id(shard, local_id, layout).expect("shard id encoding out of range")
}

pub(super) fn try_encode_id(
    shard: usize,
    local_id: u32,
    layout: ShardLayout,
) -> Result<u32, String> {
    if !layout.is_consistent() {
        return Err("shard layout is inconsistent".to_string());
    }
    if shard >= layout.num_shards {
        return Err(format!(
            "shard {} out of range for {} shards",
            shard, layout.num_shards
        ));
    }
    let segment = shard as u32 + (local_id >> layout.segment_slots_bits) * layout.num_shards as u32;
    if segment >= layout.total_segments {
        return Err(format!(
            "local_id {} in shard {} exceeds the segment address space",
            local_id, shard
        ));
    }
    Ok((segment << layout.segment_slots_bits) | (local_id & layout.segment_slots_mask()))
}

pub(super) fn decode_id(global_id: u32, layout: ShardLayout) -> (usize, u32) {
    try_decode_id(global_id, layout).expect("global id decoding out of range")
}

pub(super) fn try_decode_id(
    global_id: u32,
    layout: ShardLayout,
) -> Result<(usize, u32), String> {
    if !layout.is_consistent() {
        return Err("shard layout is inconsistent".to_string());
    }
    let segment = global_id >> layout.segment_slots_bits;
    if segment >= layout.total_segments {
        return Err(format!(
            "global id {} segment {} out of range",
            global_id, segment
        ));
    }
    let shard = (segment % layout.num_shards as u32) as usize;
    let local_id = (segment / layout.num_shards as u32) << layout.segment_slots_bits
        | (global_id & layout.segment_slots_mask());
    Ok((shard, local_id))
}

/// External-key routing scheme version pinned in the table manifest.
///
/// Locked at 1 through the development phase: there is no old data in
/// the wild, so routing improvements land in place with no version change
/// and no migration path.
pub(super) const ROUTER_VERSION: u8 = 1;

/// Splitmix64 avalanche finalizer: diffuses input entropy across all output
/// bits so prefix-similar keys and sequential integers spread evenly over
/// the shard mask instead of clustering.
fn finalize(mut hash: u64) -> u64 {
    hash = hash.wrapping_add(0x9e3779b97f4a7c15);
    hash = (hash ^ (hash >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    hash = (hash ^ (hash >> 27)).wrapping_mul(0x94d049bb133111eb);
    hash ^ (hash >> 31)
}

/// External-key shard hash: stability contract.
///
/// The shard index routes both writes and reads, and persisted global IDs
/// embed the routing shard, so this function must return the same value
/// for the same key for the lifetime of the manifest format. The FNV-1a
/// fold plus finalizer spreads prefix-similar string keys that the bare
/// multiply-xor fold clustered.
pub(super) fn fxhash(s: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    finalize(hash)
}

pub(super) fn fxhash_i64(n: i64) -> u64 {
    finalize(n as u64)
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
        self.try_record_allocation(idx, local_id)
            .expect("shard id encoding out of range")
    }

    pub(super) fn decode_id(&self, global_id: u32) -> (usize, u32) {
        decode_id(global_id, self.layout)
    }

    pub(super) fn encode_id(&self, shard: usize, local_id: u32) -> u32 {
        encode_id(shard, local_id, self.layout)
    }

    /// Fallible allocation encoding for recovery and diagnostic paths that
    /// must not panic on out-of-range ids.
    pub(super) fn try_record_allocation(&self, idx: usize, local_id: u32) -> Result<u32, String> {
        try_encode_id(idx, local_id, self.layout)
    }

    /// Fallible global-id decoding for batched scans that must skip malformed
    /// ids instead of panicking. See `group_by_shard`.
    pub(super) fn try_decode_global_id(
        &self,
        global_id: u32,
    ) -> Result<(usize, u32), String> {
        try_decode_id(global_id, self.layout)
    }

    pub fn num_shards(&self) -> usize {
        self.layout.num_shards
    }

    /// Versioned shard layout this table encodes global ids with.
    pub fn layout(&self) -> ShardLayout {
        self.layout
    }

    /// Redistribution generation of this table lineage.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
