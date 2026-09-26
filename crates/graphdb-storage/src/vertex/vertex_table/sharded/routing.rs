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
    debug_assert!(shard < layout.num_shards);
    let segment = shard as u32 + (local_id >> layout.segment_slots_bits) * layout.num_shards as u32;
    debug_assert!(
        segment < layout.total_segments,
        "local_id {local_id} in shard {shard} exceeds the segment address space"
    );
    (segment << layout.segment_slots_bits) | (local_id & layout.segment_slots_mask())
}

pub(super) fn decode_id(global_id: u32, layout: ShardLayout) -> (usize, u32) {
    let segment = global_id >> layout.segment_slots_bits;
    let shard = (segment % layout.num_shards as u32) as usize;
    let local_id = (segment / layout.num_shards as u32) << layout.segment_slots_bits
        | (global_id & layout.segment_slots_mask());
    (shard, local_id)
}

/// External-key routing scheme version pinned in the table manifest.
///
/// Version 1 is the `fxhash` mask scheme below. The version travels with
/// the persisted data (not the binary): a future routing change takes
/// effect only in a new redistribution generation, and old generations
/// keep decoding with their pinned version. Unknown versions refuse the
/// open with a rebuild directive instead of misrouting.
pub(super) const ROUTER_VERSION: u8 = 1;

/// External-key shard hash: stability contract.
///
/// The shard index routes both writes and reads, and persisted global IDs
/// embed the routing shard, so this function must return the same value
/// for the same key for the lifetime of the manifest format. A stronger
/// mixer would reduce skew on adversarial keys, but changing the output
/// would silently misroute every persisted row. Any future hash change
/// must therefore ride the offline `reshard_to` rebuild (which re-routes
/// by external key) under a new manifest marker, never as an in-place
/// edit here.
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

    /// Redistribution generation of this table lineage.
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
