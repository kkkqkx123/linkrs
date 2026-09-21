//! CSR Variant
//!
//! Enum wrapper for different CSR implementations (mutable).
//! Provides runtime polymorphism without dynamic dispatch (dyn).
//!
//! # CSR Type Selection
//!
//! The `EdgeStrategy` enum determines which CSR implementation to use:
//! - `Multiple`: Standard `MutableCsr` for general multi-edge scenarios
//! - `Single`: `SingleMutableCsr` for one-edge-per-vertex (O(1) access)
//! - `None`: No edges stored
//!
//! # Variants
//!
//! - `CsrVariant::Multiple`: Mutable CSR with dynamic capacity growth
//! - `CsrVariant::Single`: Mutable single-edge CSR
//! - `CsrVariant::Frozen`: Packed immutable CSR, writes rejected until unfrozen
//! - `CsrVariant::Mapped`: Memory-mapped frozen CSR, same read-only semantics
//! - `CsrVariant::None`: Placeholder for relationships with no edges

use graphdb_core::{StorageError, StorageResult};

use super::bundled_csr::BundledCsr;
use super::mutable_csr::VertexEdgesIter;
use super::pure_csr::{PureAllIter, PureRowIter, PureTopologyCsr};
use super::{
    CsrBase, EdgeId, EdgePosition, EdgeStrategy, FragmentationStats, FrozenRowIter, HotNbr,
    ImmutableCsr, ImmutableCsrIterator, MappedFrozen, MappedFrozenIterator, MappedFrozenRowIter,
    MutableCsr, MutableCsrIterator, MutableCsrTrait, Nbr, SingleMutableCsr,
    SingleMutableCsrIterator, Timestamp, VertexId,
};

/// Macro for dispatching method calls to the underlying CSR variant.
///
/// Expands to a match statement with proper None handling. Mutability lives
/// with the receiver, so this single macro serves both mutable and immutable
/// call sites; a second macro would only duplicate the match below.
///
/// # Usage
///
/// - Method with no arguments and return value with default:
///   `dispatch!(self, method() -> default_value)`
/// - Method with arguments and return value with default:
///   `dispatch!(self, method(arg1, arg2) -> default_value)`
///
/// # Examples
///
/// ```ignore
/// let result = dispatch!(self, insert_edge(vid, dst, id, ts) -> false);
/// let result = dispatch!(self, edges_of(vid, ts) -> Vec::new());
/// ```
macro_rules! dispatch {
    // Method with arguments and return value with default for None
    ($self:expr, $method:ident($($arg:expr),+ $(,)?) -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method($($arg),+),
            CsrVariant::Single(csr) => csr.$method($($arg),+),
            CsrVariant::Pure(csr) => csr.$method($($arg),+),
            CsrVariant::Bundled(csr) => csr.$method($($arg),+),
            CsrVariant::Frozen(csr) => csr.$method($($arg),+),
            CsrVariant::Mapped(csr) => csr.$method($($arg),+),
            CsrVariant::None { .. } => $default,
        }
    };

    // Method with no arguments and return value with default for None
    ($self:expr, $method:ident() -> $default:expr) => {
        match $self {
            CsrVariant::Multiple(csr) => csr.$method(),
            CsrVariant::Single(csr) => csr.$method(),
            CsrVariant::Pure(csr) => csr.$method(),
            CsrVariant::Bundled(csr) => csr.$method(),
            CsrVariant::Frozen(csr) => csr.$method(),
            CsrVariant::Mapped(csr) => csr.$method(),
            CsrVariant::None { .. } => $default,
        }
    };
}

/// Polymorphic CSR wrapper supporting multiple implementation strategies.
///
/// Combines mutable CSR implementations into a single enum for runtime
/// selection without generic monomorphization.
///
/// The enum dispatch via `CsrVariant` keeps one implementation per trait
/// method; callers use the trait interface so behavior stays in one place.
///
/// Cross-layer traversal contract: row positions (`EdgePosition` chunk/slot
/// pairs, primary-block offsets, overflow indices) are variant-local and
/// never cross a layer boundary. Every handoff between layers (read to
/// write, scan to point lookup, live table to migration) re-resolves the
/// target through the edge-id key first; a position obtained from one
/// variant is never interpreted by another.
///
/// # Row-view and ordering contract
///
/// All variants promise one shared row-view semantic: a row walk yields the
/// physically stored entries of one vertex, each exactly once, as assembled
/// `Nbr` records. Gap sentinels are excluded by every walk; tombstones are
/// included so reclaim and audit paths observe them. Visibility is decided
/// by the version authority above, never by these walks. Three access shapes
/// serve it with unified naming: the borrowed [`Self::visit_physical`] walk
/// (no allocation, preferred for inline forms and hot scans), the zero-alloc
/// [`Self::fill_physical_into`] caller-buffer fill (preferred for batch
/// scans), and the allocating `physical_edges_of` accessor (test and offline
/// use only). No new traversal dialect may be added per variant; new needs
/// go through these three.
///
/// Ordering is promised per form, never globally: mutable, single, pure and
/// bundled rows are insertion-ordered and promise no order; frozen and
/// mapped rows are packed sorted by `(endpoint, rank, edge_id)` and promise
/// that order plus key-interval bisection. Freeze, compaction, compression
/// and serving rebuilds may change the order; the query layer must never depend on an
/// unpromised order. [`MutableCsr::is_row_sorted`](super::MutableCsr::is_row_sorted)
/// reports the advisory per-row state for plan selection.
///
/// Mapped rows hold their mapping by value (`Arc` inside the iterator), so a
/// row walk stays valid across group replacement; the walk still yields the
/// snapshot it was created from, never the replaced group.
#[derive(Debug, Clone)]
pub enum CsrVariant {
    /// Multi-edge mutable CSR: each vertex can have multiple outgoing edges
    Multiple(Box<MutableCsr>),
    /// Single-edge mutable CSR: each vertex has at most one outgoing edge
    Single(SingleMutableCsr),
    /// Pure topology CSR: 12 bytes/edge, no rank, no timestamps
    Pure(Box<PureTopologyCsr>),
    /// Bundled CSR: 20 bytes/edge, inline single scalar value column
    Bundled(Box<BundledCsr>),
    /// Frozen packed CSR: read-only until explicitly unfrozen
    Frozen(Box<ImmutableCsr>),
    /// Memory-mapped frozen CSR: same read-only content as `Frozen`
    Mapped(Box<MappedFrozen>),
    /// No-edge placeholder: vertices exist but have no outgoing edges
    None { vertex_capacity: usize },
}

impl CsrVariant {
    pub fn from_strategy_with_overflow(
        strategy: EdgeStrategy,
        vertex_capacity: usize,
        edge_capacity: usize,
        overflow_chunk_edges: usize,
    ) -> StorageResult<Self> {
        match strategy {
            EdgeStrategy::Multiple => Ok(CsrVariant::Multiple(Box::new(
                MutableCsr::with_overflow_chunk_edges(
                    vertex_capacity,
                    edge_capacity,
                    overflow_chunk_edges,
                ),
            ))),
            EdgeStrategy::Single => Ok(CsrVariant::Single(SingleMutableCsr::with_capacity(
                vertex_capacity,
            ))),
            EdgeStrategy::None => Ok(CsrVariant::None { vertex_capacity }),
        }
    }

    /// Clear all edges
    pub fn clear(&mut self) {
        // Clearing drops the serving view: the mapping is outside the heap
        // and cannot be emptied, so the group falls back to the placeholder.
        if let CsrVariant::Mapped(csr) = self {
            let vertex_capacity = csr.vertex_capacity();
            *self = CsrVariant::None { vertex_capacity };
            return;
        }
        match self {
            CsrVariant::Multiple(csr) => csr.clear(),
            CsrVariant::Single(csr) => csr.clear(),
            CsrVariant::Pure(csr) => csr.clear(),
            CsrVariant::Bundled(csr) => csr.clear(),
            CsrVariant::Frozen(csr) => csr.clear(),
            CsrVariant::Mapped(_) => unreachable!("mapped view replaced above"),
            CsrVariant::None { .. } => {}
        }
    }

    /// Refresh the hot-path tombstone reuse cutoff. Only the multi-edge
    /// store reuses primary tombstones; single-slot rows overwrite in place
    /// already and hold no overflow, so other variants ignore the hint.
    ///
    /// Freshness follows the single contract on
    /// [`MutableCsr::set_tombstone_reuse_cutoff`](super::MutableCsr::set_tombstone_reuse_cutoff):
    /// only watermark-derived bounds refresh, the sentinel disables, and a
    /// stale value only narrows reuse.
    pub fn set_tombstone_reuse_cutoff(&mut self, cutoff: Timestamp) {
        if let CsrVariant::Multiple(csr) = self {
            csr.set_tombstone_reuse_cutoff(cutoff);
        }
    }

    /// Fresh watermark refresh allowing widening. Only call with a freshly
    /// captured global watermark bound.
    pub fn refresh_tombstone_reuse_cutoff(&mut self, fresh: Timestamp) {
        if let CsrVariant::Multiple(csr) = self {
            csr.refresh_tombstone_reuse_cutoff(fresh);
        }
    }

    /// Drop the reuse hint back to the disabled sentinel on every group.
    ///
    /// Stale path of the same contract: no fresh watermark means no reuse.
    pub fn clear_tombstone_reuse_cutoff(&mut self) {
        if let CsrVariant::Multiple(csr) = self {
            csr.clear_tombstone_reuse_cutoff();
        }
    }

    /// Current reuse cutoff for observability and tests. Non-multi-edge
    /// variants never reuse, so they report the disabled sentinel.
    pub fn tombstone_reuse_cutoff(&self) -> Timestamp {
        if let CsrVariant::Multiple(csr) = self {
            csr.tombstone_reuse_cutoff()
        } else {
            Timestamp::MAX
        }
    }

    /// Get fragmentation ratio for diagnostics
    ///
    /// Returns:
    /// - `Multiple(ratio)`: Fragmentation ratio of the CSR
    /// - `Single/None`: 0.0 (no fragmentation)
    pub fn fragmentation_ratio(&self) -> f32 {
        match self {
            CsrVariant::Multiple(csr) => csr.fragmentation_ratio(),
            _ => 0.0,
        }
    }

    /// Estimate wasted bytes due to fragmentation (only for Multiple strategy).
    pub fn wasted_bytes_estimate(&self) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.wasted_bytes_estimate(),
            _ => 0,
        }
    }

    /// Get fragmentation statistics for this CSR variant.
    ///
    /// Returns `Some(stats)` for MutableCsr variant, `None` for others.
    pub fn fragmentation_stats(&self) -> Option<super::FragmentationStats> {
        match self {
            CsrVariant::Multiple(csr) => {
                let stats = csr.get_fragmentation_stats();
                Some(FragmentationStats::with_dead_info(
                    stats.total_capacity,
                    stats.reachable_edges,
                    stats.dead_entries,
                    stats.wasted_capacity,
                ))
            }
            _ => None,
        }
    }

    /// Average bytes per edge based on actual memory usage.
    ///
    /// Computed as `used_memory_size() / edge_count()` with no fallback.
    /// Empty tables report zero; a degenerate zero measurement on a
    /// non-empty table reports zero with a debug line. Callers handle zero
    /// explicitly instead of relying on a structural estimate.
    pub fn bytes_per_edge(&self) -> usize {
        super::FragmentationStats::measured_bytes_per_edge(
            self.used_memory_size(),
            self.edge_count(),
        )
    }
}

impl CsrBase for CsrVariant {
    fn vertex_capacity(&self) -> usize {
        match self {
            CsrVariant::None { vertex_capacity } => *vertex_capacity,
            CsrVariant::Multiple(csr) => csr.vertex_capacity(),
            CsrVariant::Single(csr) => csr.vertex_capacity(),
            CsrVariant::Pure(csr) => csr.vertex_capacity(),
            CsrVariant::Bundled(csr) => csr.vertex_capacity(),
            CsrVariant::Frozen(csr) => csr.vertex_capacity(),
            CsrVariant::Mapped(csr) => csr.vertex_capacity(),
        }
    }

    fn edge_count(&self) -> u64 {
        dispatch!(self, edge_count() -> 0)
    }

    fn dump(&self) -> Vec<u8> {
        match self {
            CsrVariant::None { vertex_capacity } => {
                let mut result = vec![0u8];
                result.extend((*vertex_capacity as u64).to_le_bytes());
                result
            }
            CsrVariant::Multiple(csr) => {
                let mut result = vec![1u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Single(csr) => {
                let mut result = vec![2u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Frozen(csr) => {
                let mut result = vec![3u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Mapped(csr) => {
                let mut result = vec![3u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Pure(csr) => {
                let mut result = vec![4u8];
                result.extend(csr.dump());
                result
            }
            CsrVariant::Bundled(csr) => {
                let mut result = vec![5u8];
                result.extend(csr.dump());
                result
            }
        }
    }

    fn dump_into(&self, out: &mut Vec<u8>) {
        match self {
            CsrVariant::None { vertex_capacity } => {
                out.push(0u8);
                out.extend((*vertex_capacity as u64).to_le_bytes());
            }
            CsrVariant::Multiple(csr) => {
                out.push(1u8);
                csr.dump_into(out);
            }
            CsrVariant::Single(csr) => {
                out.push(2u8);
                csr.dump_into(out);
            }
            CsrVariant::Frozen(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Mapped(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Pure(csr) => {
                out.push(4u8);
                csr.dump_into(out);
            }
            CsrVariant::Bundled(csr) => {
                out.push(5u8);
                csr.dump_into(out);
            }
        }
    }

    fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.is_empty() {
            return Err(graphdb_core::StorageError::deserialize_error(
                "Cannot load CSR variant: empty data",
            ));
        }

        match data[0] {
            0 => {
                if data.len() < 9 {
                    return Err(graphdb_core::StorageError::deserialize_error(
                        "Cannot load None CSR variant: data too short",
                    ));
                }
                let vertex_capacity = u64::from_le_bytes([
                    data[1], data[2], data[3], data[4], data[5], data[6], data[7], data[8],
                ]) as usize;
                *self = CsrVariant::None { vertex_capacity };
                Ok(())
            }
            1 => {
                let mut csr = MutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Multiple(Box::new(csr));
                Ok(())
            }
            2 => {
                let mut csr = SingleMutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Single(csr);
                Ok(())
            }
            3 => {
                let mut csr = ImmutableCsr::new();
                csr.load(&data[1..])?;
                *self = CsrVariant::Frozen(Box::new(csr));
                Ok(())
            }
            4 => {
                let mut csr = PureTopologyCsr::default();
                csr.load(&data[1..])?;
                *self = CsrVariant::Pure(Box::new(csr));
                Ok(())
            }
            5 => {
                let mut csr = BundledCsr::default();
                csr.load(&data[1..])?;
                *self = CsrVariant::Bundled(Box::new(csr));
                Ok(())
            }
            _ => Err(graphdb_core::StorageError::deserialize_error(
                "Invalid CSR variant tag in serialized data",
            )),
        }
    }
}

impl CsrVariant {
    /// Borrow-based dump reusing caller-owned column buffers.
    ///
    /// Same bytes as `dump_into` through the base trait; a checkpoint over
    /// many groups pays one allocation per column instead of one per group.
    pub fn dump_into_with_scratch(
        &self,
        out: &mut Vec<u8>,
        scratch: &mut super::mutable_csr::persistence::CsrDumpScratch,
    ) {
        match self {
            CsrVariant::None { vertex_capacity } => {
                out.push(0u8);
                out.extend((*vertex_capacity as u64).to_le_bytes());
            }
            CsrVariant::Multiple(csr) => {
                out.push(1u8);
                csr.dump_into_with_scratch(out, scratch);
            }
            CsrVariant::Single(csr) => {
                out.push(2u8);
                csr.dump_into(out);
            }
            CsrVariant::Frozen(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Mapped(csr) => {
                out.push(3u8);
                csr.dump_into(out);
            }
            CsrVariant::Pure(csr) => {
                out.push(4u8);
                csr.dump_into(out);
            }
            CsrVariant::Bundled(csr) => {
                out.push(5u8);
                csr.dump_into(out);
            }
        }
    }
}

impl MutableCsrTrait for CsrVariant {
    fn insert_edge(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<()> {
        match self {
            CsrVariant::Multiple(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Single(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Pure(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Bundled(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Frozen(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::Mapped(csr) => csr.insert_edge(src_vid, dst, edge_id, ts),
            CsrVariant::None { .. } => Err(StorageError::invalid_operation(
                "no edges stored for this edge type".to_string(),
            )),
        }
    }

    fn delete_edge(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> StorageResult<bool> {
        dispatch!(self, delete_edge(src_vid, edge_id, ts) -> Err(StorageError::invalid_operation(
            "no edges stored for this edge type".to_string()
        )))
    }

    fn delete_edge_by_dst(&mut self, src_vid: u32, dst: VertexId, ts: Timestamp) -> usize {
        dispatch!(self, delete_edge_by_dst(src_vid, dst, ts) -> 0)
    }

    fn delete_edge_by_dst_reporting(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Single(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Pure(csr) => csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted),
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_by_dst_reporting(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn delete_edge_by_dst_reporting_positioned(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        ts: Timestamp,
        on_deleted: &mut dyn FnMut(EdgeId, Option<EdgePosition>),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge_by_dst_reporting_positioned(
                src_vid,
                dst,
                ts,
                &mut |edge_id, position| on_deleted(edge_id, Some(position)),
            ),
            CsrVariant::Pure(csr) => {
                csr.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, on_deleted)
            }
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_by_dst_reporting_positioned(src_vid, dst, ts, on_deleted)
            }
            _ => self.delete_edge_by_dst_reporting(src_vid, dst, ts, &mut |edge_id| {
                on_deleted(edge_id, None)
            }),
        }
    }

    fn locate_edge(&self, src_vid: u32, edge_id: EdgeId) -> Option<(EdgePosition, Nbr)> {
        match self {
            CsrVariant::Multiple(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Pure(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Bundled(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Frozen(csr) => csr.locate_edge(src_vid, edge_id),
            CsrVariant::Mapped(csr) => csr.locate_edge(src_vid, edge_id),
            _ => None,
        }
    }

    fn delete_edge_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Pure(csr) => csr.delete_edge_at_position(src_vid, position, expected, ts),
            CsrVariant::Bundled(csr) => {
                csr.delete_edge_at_position(src_vid, position, expected, ts)
            }
            _ => Err(StorageError::invalid_operation(
                "row position must not cross variants; re-resolve by edge id".to_string(),
            )),
        }
    }

    fn revert_delete_at_position(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
    ) -> bool {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.revert_delete_at_position(src_vid, position, expected, ts)
            }
            CsrVariant::Pure(csr) => csr.revert_delete_at_position(src_vid, position, expected, ts),
            CsrVariant::Bundled(csr) => {
                csr.revert_delete_at_position(src_vid, position, expected, ts)
            }
            _ => {
                debug_assert!(
                    false,
                    "row position must not cross variants; re-resolve by edge id"
                );
                false
            }
        }
    }

    fn delete_edge_by_offset(
        &mut self,
        src_vid: u32,
        offset: i32,
        ts: Timestamp,
    ) -> StorageResult<bool> {
        match self {
            CsrVariant::Multiple(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Single(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Pure(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Bundled(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Frozen(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::Mapped(csr) => csr.delete_edge_by_offset(src_vid, offset, ts),
            CsrVariant::None { .. } => Ok(false),
        }
    }

    fn revert_delete_by_offset(&mut self, src_vid: u32, offset: i32, ts: Timestamp) -> bool {
        dispatch!(self, revert_delete_by_offset(src_vid, offset, ts) -> false)
    }

    fn nbr_at_offset(&self, src_vid: u32, offset: i32) -> Option<Nbr> {
        dispatch!(self, nbr_at_offset(src_vid, offset) -> None)
    }

    fn get_edge_physical(&self, src_vid: u32, dst: VertexId) -> Option<Nbr> {
        dispatch!(self, get_edge_physical(src_vid, dst) -> None)
    }

    fn physical_edges_of(&self, src_vid: u32) -> Vec<Nbr> {
        dispatch!(self, physical_edges_of(src_vid) -> Vec::new())
    }

    fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        match self {
            CsrVariant::Multiple(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Single(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Pure(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Bundled(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Frozen(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Mapped(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::None { .. } => out.clear(),
        }
    }

    fn has_physical_entries(&self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.has_physical_entries(vid),
            CsrVariant::Single(csr) => csr.has_physical_entries(vid),
            CsrVariant::Pure(csr) => csr.has_physical_entries(vid),
            CsrVariant::Bundled(csr) => csr.has_physical_entries(vid),
            CsrVariant::Frozen(csr) => csr.has_physical_entries(vid),
            CsrVariant::Mapped(csr) => csr.has_physical_entries(vid),
            CsrVariant::None { .. } => false,
        }
    }

    fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Single(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Pure(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Bundled(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Frozen(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::Mapped(csr) => csr.primary_contains(src_vid, edge_id),
            CsrVariant::None { .. } => false,
        }
    }

    fn rollback_insert(&mut self, src_vid: u32, edge_id: EdgeId) -> bool {
        dispatch!(self, rollback_insert(src_vid, edge_id) -> false)
    }

    fn revert_delete_by_edge_id(&mut self, src_vid: u32, edge_id: EdgeId, ts: Timestamp) -> bool {
        dispatch!(self, revert_delete_by_edge_id(src_vid, edge_id, ts) -> false)
    }

    fn get_edge(&self, src_vid: u32, dst: VertexId, ts: Timestamp) -> Option<Nbr> {
        dispatch!(self, get_edge(src_vid, dst, ts) -> None)
    }

    fn edges_of(&self, src_vid: u32, ts: Timestamp) -> Vec<Nbr> {
        dispatch!(self, edges_of(src_vid, ts) -> Vec::new())
    }

    fn compact_vertex_with_reporting(
        &mut self,
        vid: u32,
        cutoff: Timestamp,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Single(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Pure(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Bundled(csr) => {
                csr.compact_vertex_with_reporting(vid, cutoff, on_edge_removed)
            }
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn reclaimable_count(&self, vid: u32, cutoff: Timestamp) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Single(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Pure(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Bundled(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Frozen(csr) => csr.reclaimable_count(vid, cutoff),
            CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    fn vertex_needs_compact(&self, vid: u32, cutoff: Timestamp) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.vertex_needs_compact(vid, cutoff),
            CsrVariant::Single(csr) => csr.reclaimable_count(vid, cutoff) > 0,
            CsrVariant::Pure(_) | CsrVariant::Bundled(_) => false,
            CsrVariant::Frozen(csr) => csr.reclaimable_count(vid, cutoff) > 0,
            CsrVariant::Mapped(_) => false,
            CsrVariant::None { .. } => false,
        }
    }

    fn vertex_census(&self, vid: u32) -> (usize, usize, usize) {
        match self {
            CsrVariant::Multiple(csr) => csr.vertex_census(vid),
            CsrVariant::Single(csr) => csr.vertex_census(vid),
            CsrVariant::Pure(csr) => csr.vertex_census(vid),
            CsrVariant::Bundled(csr) => csr.vertex_census(vid),
            CsrVariant::Frozen(csr) => csr.vertex_census(vid),
            CsrVariant::Mapped(csr) => csr.vertex_census(vid),
            CsrVariant::None { .. } => (0, 0, 0),
        }
    }

    fn vertex_reclaim_probe(&self, vid: u32, cutoff: Timestamp) -> (usize, usize) {
        match self {
            CsrVariant::Multiple(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Single(csr) => csr.vertex_reclaim_probe(vid, cutoff),
            CsrVariant::Pure(_) | CsrVariant::Bundled(_) => (0, 0),
            CsrVariant::Frozen(_) | CsrVariant::Mapped(_) => (0, 0),
            CsrVariant::None { .. } => (0, 0),
        }
    }

    fn row_gap(&self, vid: u32) -> usize {
        match self {
            CsrVariant::Multiple(csr) => csr.row_gap(vid),
            CsrVariant::Pure(csr) => csr.row_gap(vid),
            CsrVariant::Bundled(csr) => csr.row_gap(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => 0,
        }
    }

    fn row_density(&self, vid: u32) -> f32 {
        match self {
            CsrVariant::Multiple(csr) => csr.row_density(vid),
            CsrVariant::Pure(csr) => csr.row_density(vid),
            CsrVariant::Bundled(csr) => csr.row_density(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => 1.0,
        }
    }

    fn rebalance_row(&mut self, vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.rebalance_row(vid),
            CsrVariant::Pure(csr) => csr.rebalance_row(vid),
            CsrVariant::Bundled(csr) => csr.rebalance_row(vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => true,
        }
    }

    fn used_memory_size(&self) -> usize {
        match self {
            CsrVariant::None { .. } => std::mem::size_of::<Self>(),
            CsrVariant::Multiple(csr) => csr.used_memory_size(),
            CsrVariant::Single(csr) => csr.used_memory_size(),
            CsrVariant::Pure(csr) => csr.used_memory_size(),
            CsrVariant::Bundled(csr) => csr.used_memory_size(),
            CsrVariant::Frozen(csr) => csr.used_memory_size(),
            CsrVariant::Mapped(csr) => csr.used_memory_size(),
        }
    }
}

impl CsrVariant {
    /// Iterate edges of a vertex without allocating, for every strategy.
    ///
    /// Test-only row-stamp filtered iterator; production scans go through
    /// the version authority. Missing groups and the `None` strategy yield
    /// no iterator.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> Option<CsrRowIter<'_>> {
        // Pure rows store no timestamps, so the timestamp carries no
        // information there; the borrowed walk yields live entries directly.
        let _ = ts;
        match self {
            CsrVariant::Multiple(csr) => Some(CsrRowIter::Multiple(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::Single(csr) => Some(CsrRowIter::Single(
                csr.iter_edges_of(src_vid, ts).into_iter(),
            )),
            CsrVariant::Pure(csr) => Some(CsrRowIter::Pure(csr.iter_row(src_vid))),
            CsrVariant::Bundled(csr) => Some(CsrRowIter::Bundled(csr.iter_row(src_vid))),
            CsrVariant::Frozen(csr) => Some(CsrRowIter::Frozen(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::Mapped(csr) => Some(CsrRowIter::Mapped(csr.iter_edges_of(src_vid, ts))),
            CsrVariant::None { .. } => None,
        }
    }

    /// Iterate over all physically present entries, including tombstoned ones.
    ///
    /// Used when rebuilding the CSR (e.g. vertex ID remapping) so entries
    /// marked as deleted survive the rebuild. Pure and bundled holes carry
    /// the unassignable sentinel instead of an edge, so they are skipped:
    /// no rebuild may resurrect a hole as a live edge.
    pub fn iter_all(&self) -> CsrIterator<'_> {
        match self {
            CsrVariant::Multiple(csr) => CsrIterator::Multiple(csr.iter_all()),
            CsrVariant::Single(csr) => CsrIterator::Single(csr.iter_all()),
            CsrVariant::Pure(csr) => CsrIterator::Pure(csr.iter_all()),
            CsrVariant::Bundled(csr) => CsrIterator::Bundled(csr.iter_all()),
            CsrVariant::Frozen(csr) => CsrIterator::Frozen(csr.iter_all()),
            CsrVariant::Mapped(csr) => CsrIterator::Mapped(csr.iter_all()),
            CsrVariant::None { .. } => CsrIterator::None,
        }
    }

    /// Compact with per-edge removal reporting.
    ///
    /// Both retained strategies report reclaimed tombstones; the placeholder
    /// keeps the no-op semantics. Frozen groups reclaim tombstones in place
    /// with no reserve; mapped views need a serving-file rebuild and stay
    /// no-op here.
    pub fn compact_with_ts_reporting(
        &mut self,
        cutoff: Timestamp,
        reserve_ratio: f32,
        on_edge_removed: &mut dyn FnMut(EdgeId, Timestamp),
    ) -> usize {
        match self {
            CsrVariant::Multiple(csr) => {
                csr.compact_with_ts_reporting(cutoff, reserve_ratio, on_edge_removed)
            }
            CsrVariant::Single(csr) => csr.compact_with_ts_reporting(cutoff, on_edge_removed),
            CsrVariant::Pure(_) | CsrVariant::Bundled(_) => 0,
            CsrVariant::Frozen(csr) => csr.compact_with_cutoff(cutoff, on_edge_removed),
            CsrVariant::Mapped(_) => 0,
            CsrVariant::None { .. } => 0,
        }
    }

    /// Visit every physically stored entry of one vertex without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Single(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Pure(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Bundled(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Frozen(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::Mapped(csr) => csr.visit_physical(src_vid, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Visit every physically stored hot half of one vertex without
    /// allocating and without touching the stamp lines.
    ///
    /// Hot-only counterpart of [`Self::visit_physical`] for traversals that
    /// resolve visibility through the version authority by `edge_id`.
    pub fn visit_hot<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(HotNbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Single(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Pure(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Bundled(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Frozen(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::Mapped(csr) => csr.visit_hot(src_vid, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Fill a caller buffer with every physically stored entry of one vertex.
    ///
    /// Same content as the allocating trait accessor, without the per-vertex
    /// allocation. Batch scans reuse one buffer across vertices. Behavior is
    /// uniform across variants: the buffer is cleared first, then the row is
    /// appended in the variant's promised order (insertion order for mutable
    /// forms, sorted order for frozen forms; see the enum-level contract).
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        match self {
            CsrVariant::Multiple(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Single(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Pure(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Bundled(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Frozen(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::Mapped(csr) => csr.fill_physical_into(src_vid, out),
            CsrVariant::None { .. } => out.clear(),
        }
    }

    /// Whether the live entries of one row arrive in key order.
    ///
    /// Query routing consults this before choosing a bisection over a
    /// linear walk. Frozen, mapped and single-slot rows are always ordered;
    /// mutable rows report their insertion-order observation.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Single(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Pure(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Bundled(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Frozen(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::Mapped(csr) => csr.is_row_sorted(src_vid),
            CsrVariant::None { .. } => true,
        }
    }

    /// Sort one primary row into key order on the maintenance path.
    ///
    /// Same invalidation as the underlying stores: positions go stale and
    /// the caller relocates through the edge-id key. Frozen, mapped,
    /// single-slot and empty forms are already ordered and report false.
    pub fn sort_row(&mut self, src_vid: u32) -> bool {
        match self {
            CsrVariant::Multiple(csr) => csr.sort_row(src_vid),
            CsrVariant::Pure(csr) => csr.sort_row(src_vid),
            CsrVariant::Bundled(csr) => csr.sort_row(src_vid),
            CsrVariant::Single(_)
            | CsrVariant::Frozen(_)
            | CsrVariant::Mapped(_)
            | CsrVariant::None { .. } => false,
        }
    }

    /// Visit live entries whose `(endpoint, rank)` key falls in the inclusive
    /// `[lower, upper]` range. Pure and bundled rows carry no rank, so the
    /// rank halves of the bounds are ignored there.
    pub fn visit_threshold<F>(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        match self {
            CsrVariant::Multiple(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Single(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Pure(csr) => {
                csr.visit_threshold(src_vid, lower.map(|(ep, _)| ep), upper.map(|(ep, _)| ep), f)
            }
            CsrVariant::Bundled(csr) => {
                csr.visit_threshold(src_vid, lower.map(|(ep, _)| ep), upper.map(|(ep, _)| ep), f)
            }
            CsrVariant::Frozen(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::Mapped(csr) => csr.visit_threshold(src_vid, lower, upper, f),
            CsrVariant::None { .. } => {}
        }
    }

    /// Fill a caller buffer with the same range content as `visit_threshold`.
    pub fn fill_threshold_into(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }

    /// Whether this group stores its single scalar inline.
    pub fn is_bundled(&self) -> bool {
        matches!(self, CsrVariant::Bundled(_))
    }

    /// Insert one edge carrying its inline value.
    ///
    /// Only the bundled form accepts a value; every other form rejects the
    /// call instead of silently dropping the property.
    pub fn insert_edge_with_value(
        &mut self,
        src_vid: u32,
        dst: VertexId,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> StorageResult<()> {
        match self {
            CsrVariant::Bundled(csr) => csr.insert_edge_with_value(src_vid, dst, edge_id, value),
            _ => Err(StorageError::invalid_operation(
                "inline values require the bundled record form".to_string(),
            )),
        }
    }

    /// Read one edge's inline value within its source row.
    pub fn bundled_value_by_edge_id(&self, src_vid: u32, edge_id: EdgeId) -> Option<(u64, bool)> {
        match self {
            CsrVariant::Bundled(csr) => csr.value_by_edge_id(src_vid, edge_id),
            _ => None,
        }
    }

    /// Overwrite one edge's inline value within its source row.
    pub fn bundled_set_value_by_edge_id(
        &mut self,
        src_vid: u32,
        edge_id: EdgeId,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.set_value_by_edge_id(src_vid, edge_id, value),
            _ => false,
        }
    }

    /// Read the inline value of the live edge for one endpoint.
    pub fn bundled_value_by_endpoint(&self, src_vid: u32, endpoint: u32) -> Option<(u64, bool)> {
        match self {
            CsrVariant::Bundled(csr) => csr.value_by_endpoint(src_vid, endpoint),
            _ => None,
        }
    }

    /// Overwrite the inline value of the live edge for one endpoint.
    pub fn bundled_set_value_by_endpoint(
        &mut self,
        src_vid: u32,
        endpoint: u32,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.set_value_by_endpoint(src_vid, endpoint, value),
            _ => false,
        }
    }

    /// Revert a deletion restoring the caller's value alongside the topology.
    pub fn bundled_revert_with_value(
        &mut self,
        src_vid: u32,
        position: EdgePosition,
        expected: EdgeId,
        ts: Timestamp,
        value: Option<u64>,
    ) -> bool {
        match self {
            CsrVariant::Bundled(csr) => {
                csr.revert_delete_at_position_with_value(src_vid, position, expected, ts, value)
            }
            _ => false,
        }
    }

    /// Visit every physically stored entry of one vertex with its inline
    /// value (`None` for NULL slots). Non-bundled forms visit with `None`.
    pub fn visit_physical_with_values<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr, Option<u64>) -> bool,
    {
        match self {
            CsrVariant::Bundled(csr) => csr.visit_physical_with_values(src_vid, f),
            _ => self.visit_physical(src_vid, |nbr| f(nbr, None)),
        }
    }

    /// Whether any slot holds a valid inline value.
    pub fn bundled_has_valid_values(&self) -> bool {
        match self {
            CsrVariant::Bundled(csr) => csr.any_valid_values(),
            _ => false,
        }
    }
}

/// Iterator over CSR edges, supporting multiple implementation types
pub enum CsrIterator<'a> {
    /// Iterator over multi-edge CSR
    Multiple(MutableCsrIterator<'a>),
    /// Iterator over single-edge CSR
    Single(SingleMutableCsrIterator<'a>),
    /// Iterator over pure topology CSR
    Pure(PureAllIter<'a>),
    /// Iterator over bundled CSR
    Bundled(PureAllIter<'a>),
    /// Iterator over frozen packed CSR
    Frozen(ImmutableCsrIterator<'a>),
    /// Iterator over memory-mapped frozen CSR (owns its mapping handle)
    Mapped(MappedFrozenIterator),
    /// Empty iterator
    None,
}

impl<'a> Iterator for CsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CsrIterator::Multiple(iter) => iter.next(),
            CsrIterator::Single(iter) => iter.next(),
            CsrIterator::Pure(iter) => iter.next(),
            CsrIterator::Bundled(iter) => iter.next(),
            CsrIterator::Frozen(iter) => iter.next(),
            CsrIterator::Mapped(iter) => iter.next(),
            CsrIterator::None => None,
        }
    }
}

/// Borrowed per-vertex edge iterator covering every CSR strategy.
///
/// Allocation-free counterpart of `edges_of` at the variant level: each arm
/// wraps the strategy's own iterator, so callers above the variant (shard
/// sets, table scans) iterate one row without materializing a vector,
/// regardless of the underlying layout. Records are assembled by value
/// because the split halves live in separate slices.
pub enum CsrRowIter<'a> {
    /// Multi-edge row: primary block plus overflow chain walk.
    Multiple(VertexEdgesIter<'a>),
    /// Single-edge row: at most one assembled slot.
    Single(std::option::IntoIter<Nbr>),
    /// Pure topology row: borrowed primary plus overflow walk.
    Pure(PureRowIter<'a>),
    /// Bundled row: borrowed topology walk, values resolved separately.
    Bundled(PureRowIter<'a>),
    /// Frozen row: filtered packed-slice walk.
    Frozen(FrozenRowIter<'a>),
    /// Mapped frozen row: on-demand decode walk owning its mapping handle.
    Mapped(MappedFrozenRowIter),
}

impl<'a> Iterator for CsrRowIter<'a> {
    type Item = Nbr;

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            CsrRowIter::Multiple(iter) => iter.next(),
            CsrRowIter::Single(iter) => iter.next(),
            CsrRowIter::Pure(iter) => iter.next(),
            CsrRowIter::Bundled(iter) => iter.next(),
            CsrRowIter::Frozen(iter) => iter.next(),
            CsrRowIter::Mapped(iter) => iter.next(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multiple_csr_variant() {
        let mut csr =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_single_csr_variant() {
        let mut csr =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Single, 10, 100, 4096).unwrap();

        csr.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        assert_eq!(csr.edge_count(), 1);
    }

    #[test]
    fn test_frozen_variant_dump_load_roundtrip() {
        let mut inner = MutableCsr::with_capacity(8, 64);
        inner
            .insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();
        let frozen = CsrVariant::Frozen(Box::new(super::ImmutableCsr::pack_from_mutable(&inner)));
        assert_eq!(frozen.edge_count(), 1);
        let bytes = frozen.dump();
        assert_eq!(bytes[0], 3u8);

        let mut loaded =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 8, 64, 4096).unwrap();
        loaded.load(&bytes).unwrap();
        assert_eq!(loaded.edge_count(), 1);
        assert_eq!(loaded.edges_of(0, 1), frozen.edges_of(0, 1));
        assert!(loaded
            .insert_edge(1u32, VertexId::from_int64(2), EdgeId(101), 1)
            .is_err());
    }

    #[test]
    fn test_removed_variant_tags_are_rejected() {
        for tag in [6u8, 7u8] {
            let mut payload = vec![tag];
            payload.extend_from_slice(&[0u8; 8]);
            let mut csr =
                CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096)
                    .unwrap();
            assert!(csr.load(&payload).is_err());
        }
    }

    #[test]
    fn test_none_csr_variant() {
        let mut csr =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();

        // None variant should return the configured vertex capacity
        assert_eq!(csr.vertex_capacity(), 10);
        assert_eq!(csr.edge_count(), 0);
        assert!(csr.edges_of(0, 1).is_empty());

        // None variant should reject all insertions
        assert!(csr
            .insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .is_err());
        assert_eq!(csr.edge_count(), 0);

        // None variant should reject all deletions
        assert!(csr.delete_edge(0, EdgeId(100), 1).is_err());
        assert_eq!(csr.delete_edge_by_dst(0, VertexId::from_int64(1), 1), 0);
        assert!(!csr.revert_delete_by_offset(0, 0, 1));

        // None variant should return None for get_edge
        assert!(csr.get_edge(0, VertexId::from_int64(1), 1).is_none());

        // Clear should be a no-op
        csr.clear();
        assert_eq!(csr.edge_count(), 0);
    }

    #[test]
    fn test_none_csr_iter() {
        let csr =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
        let mut iter = csr.iter_all();

        // Iterator should produce no items
        assert!(iter.next().is_none());
    }

    #[test]
    fn test_none_csr_dump_load() {
        let csr1 =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
        let data = csr1.dump();

        // Data should start with variant tag (0 for None)
        assert!(!data.is_empty());
        assert_eq!(data[0], 0u8);

        let mut csr2 =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
        csr2.load(&data).unwrap();

        // After loading, should be None variant
        assert_eq!(csr2.edge_count(), 0);
        assert!(csr2
            .insert_edge(0, VertexId::from_int64(1), EdgeId(100), 1)
            .is_err());
    }

    #[test]
    fn test_clone() {
        let mut csr1 =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::Multiple, 10, 100, 4096).unwrap();
        csr1.insert_edge(0u32, VertexId::from_int64(1), EdgeId(100), 1)
            .unwrap();

        let csr2 = csr1.clone();
        assert_eq!(csr2.edge_count(), 1);
    }

    #[test]
    fn test_clone_none() {
        let csr1 =
            CsrVariant::from_strategy_with_overflow(EdgeStrategy::None, 10, 100, 4096).unwrap();
        let mut csr2 = csr1.clone();

        assert_eq!(csr2.edge_count(), 0);
        assert!(csr2
            .insert_edge(0, VertexId::from_int64(1), EdgeId(100), 1)
            .is_err());
    }

    #[test]
    fn pure_row_iter_matches_allocating_read() {
        let mut inner = PureTopologyCsr::with_capacity(8, 16);
        for dst in [1u32, 2, 3] {
            inner
                .insert_edge(
                    0,
                    VertexId::edge_endpoint_key(dst, 0),
                    EdgeId(dst as u64),
                    0,
                )
                .unwrap();
        }
        let variant = CsrVariant::Pure(Box::new(inner));
        let via_iter: Vec<Nbr> = variant.iter_edges_of(0, 1).unwrap().collect();
        assert_eq!(via_iter, variant.edges_of(0, 1));
        assert_eq!(via_iter.len(), 3);
    }

    #[test]
    fn bundled_row_iter_matches_allocating_read() {
        let mut inner = BundledCsr::with_capacity(8, 16);
        for dst in [5u32, 6] {
            inner
                .insert_edge(
                    0,
                    VertexId::edge_endpoint_key(dst, 0),
                    EdgeId(dst as u64),
                    0,
                )
                .unwrap();
        }
        let variant = CsrVariant::Bundled(Box::new(inner));
        let via_iter: Vec<Nbr> = variant.iter_edges_of(0, 1).unwrap().collect();
        assert_eq!(via_iter, variant.edges_of(0, 1));
        assert_eq!(via_iter.len(), 2);
    }
}
