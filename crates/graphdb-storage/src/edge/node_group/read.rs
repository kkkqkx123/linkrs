//! Routed physical read helpers over the shard set.
//!
//! Reads resolve rows through `route`, never creating groups: missing groups
//! read as empty.

use graphdb_core::types::EdgeId;

use super::super::{HotNbr, MutableCsrTrait, Nbr};
use super::CsrShardSet;

impl CsrShardSet {
    /// Whether the primary row of one vertex holds `edge_id`.
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.primary_contains(local, edge_id))
    }

    /// Owner group of one global vertex id without creating groups.
    ///
    /// Shared row-location entry point for point lookups, adjacency batches
    /// and full scans: every read path resolves rows through this routing
    /// instead of duplicating the group arithmetic.
    pub fn group_of(&self, vid: u32) -> Option<usize> {
        self.route(vid).map(|(gid, _)| gid)
    }

    /// Visit every physically stored entry of one vertex without allocating.
    pub fn visit_physical<F>(&self, src_vid: u32, f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            shard.variant.visit_physical(local, f);
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
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            shard.variant.visit_hot(local, f);
        }
    }

    /// Fill a caller buffer with every physically stored entry of one vertex.
    ///
    /// Same content as the allocating trait accessor, without the per-vertex
    /// allocation. Missing groups read as empty and never create groups.
    pub fn fill_physical_into(&self, src_vid: u32, out: &mut Vec<Nbr>) {
        let Some((gid, local)) = self.route(src_vid) else {
            out.clear();
            return;
        };
        match self.shards.get(&gid) {
            Some(shard) => shard.variant.fill_physical_into(local, out),
            None => out.clear(),
        }
    }

    /// Fill one shared buffer with the physical entries of many vertices.
    ///
    /// Records one start offset per vertex plus a trailing end offset, so
    /// `out[offsets[i]..offsets[i + 1]]` is vertex `vids[i]` in order. One
    /// pass over the vertices, no per-vertex allocation; missing groups and
    /// empty rows contribute empty slices. Read paths never create groups.
    pub fn fill_physical_batch_into(
        &self,
        vids: &[u32],
        out: &mut Vec<Nbr>,
        offsets: &mut Vec<usize>,
    ) {
        out.clear();
        offsets.clear();
        offsets.reserve(vids.len() + 1);
        for vid in vids {
            offsets.push(out.len());
            self.visit_physical(*vid, |nbr| {
                out.push(nbr);
                true
            });
        }
        offsets.push(out.len());
    }
}
