//! Frozen group mmap serving file.
//!
//! Derived read-only cache beside the authoritative checkpoint: while the
//! checkpoint stays the source of truth, a frozen group can also serve
//! queries straight from a memory-mapped flat column file, paging entries in
//! on demand instead of decoding the whole group into the heap at open.
//!
//! File layout, everything little-endian:
//! - magic (u32)
//! - rows (u64), entries (u64), live edge count (u64)
//! - five `(offset u64, length u64)` column descriptors: degrees, endpoints,
//!   ranks, edge ids, delete stamps
//! - degrees: `rows` u32 values
//! - endpoints: `entries` u32 values
//! - ranks: `entries` i64 values
//! - edge ids, delete stamps: `entries` u64 values each
//!
//! Column widths are fixed, so any slot is addressable by index with one
//! little-endian decode and no full-file decode. Row offsets are rebuilt in
//! memory on open and never persisted, mirroring the heap frozen form.
//!
//! The serving payload carries a trailing CRC32 covering every preceding
//! byte, using the same checksum pattern as the heap checkpoint dumps. Open
//! rejects structural mismatches (magic, out-of-range descriptors,
//! length disagreements, trailing bytes) and CRC mismatches alike, and the
//! caller falls back to the authoritative checkpoint, optionally
//! regenerating the serving file. A bad cache is discarded and rebuilt. Writers use a sibling temp file
//! plus atomic rename, so readers only ever observe complete files.
//!
//! The mapped handle is reference-counted (`MappedFrozen` clones share one
//! mapping), so readers holding a view keep the file alive across serving
//! file replacement. Single-writer discipline applies: the checkpoint flush
//! syncs base file and serving file together, and flushing a non-frozen
//! group removes its serving file. On Linux the mapping carries a transparent
//! huge page hint; a rejected hint falls back to base pages without failing
//! the open.
//!
//! # Serving cache state machine
//!
//! The serving file is a derived cache, never the truth. States and
//! transitions, all covered by the checkpoint tests:
//!
//! - Absent: no sidecar beside the base. Frozen or mapped bases backfill one
//!   on flush when missing; mutable bases stay absent.
//! - Valid: sidecar parses (magic, descriptors, lengths, CRC) and
//!   loads validate it against the base. Loads map it directly and skip the
//!   authoritative decode.
//! - Stale: the base was rewritten as mutable, or the group is gone. The
//!   flush removes the sidecar; readers never see it.
//! - Expired: the sidecar fails validation (corrupt bytes or
//!   a pending append delta the read-only view cannot absorb). Loads fall
//!   back to the authoritative base and regenerate the cache; the failure is
//!   counted in debug logs, never silent, and never fails the load.
//!
//! Generation, deletion, expiry and fallback are all decided in the
//! checkpoint serving helpers and the group load path; no other module
//! creates or removes sidecars.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use graphdb_core::{StorageError, StorageResult};

use super::csr_shared::decode_endpoint_pair;
use super::mutable_csr::serialization::{
    encode_topology_i64_column, encode_topology_u32_column, encode_topology_u64_column,
};
use super::{ColdStamps, EdgePosition, HotNbr, ImmutableCsr, Nbr, INVALID_EDGE_ID};
use graphdb_core::types::{EdgeId, Timestamp, VertexId};

pub(crate) mod columns;
pub(crate) mod format;
pub(crate) mod iter;
pub(crate) mod open;
pub(crate) mod persistence;
pub(crate) mod read;
pub(crate) mod trait_impl;

#[cfg(test)]
mod tests;

use format::ServingColumns;
pub use format::{serving_path_for, write_serving_file};
pub(crate) use format::{SERVING_CRC_LEN, SERVING_HEADER_LEN, SERVING_MAGIC};
pub use iter::{MappedFrozenIterator, MappedFrozenRowIter};

/// Memory-mapped read view of one frozen group serving file.
///
/// Clones share one mapping through the reference count, so snapshotting the
/// handle for a reader is cheap and the mapping outlives serving file
/// replacement while readers hold it. The query surface mirrors
/// [`ImmutableCsr`] so variant dispatch treats both identically; writes are
/// rejected the same way.
#[derive(Debug, Clone)]
pub struct MappedFrozen {
    map: Arc<memmap2::Mmap>,
    columns: ServingColumns,
    offsets: Arc<Vec<u32>>,
    rows: usize,
    entries: usize,
    edge_count: u64,
}
