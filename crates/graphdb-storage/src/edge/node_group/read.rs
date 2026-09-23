//! Routed physical read helpers over the shard set.
//!
//! Reads resolve rows through `route`, never creating groups: missing groups
//! read as empty.
//!
//! Dispatch hoisting: single-row entries match the shard variant once per
//! row, which is optimal for point lookups. Batch and full-table scans match
//! once per shard and loop the rows of that shard with the concrete type,
//! so the row loop never pays a per-row enum dispatch. The `CsrVariant`
//! body itself is untouched; only these callers single-morphize.

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
    ///
    /// Dispatch is hoisted to consecutive same-group runs: each run matches
    /// its shard variant once and loops its rows with the concrete type,
    /// instead of matching per row. Consecutive vertex batches (the common
    /// scan order) therefore pay one dispatch per shard, not per row.
    pub fn fill_physical_batch_into(
        &self,
        vids: &[u32],
        out: &mut Vec<Nbr>,
        offsets: &mut Vec<usize>,
    ) {
        use super::super::CsrVariant;
        out.clear();
        offsets.clear();
        offsets.reserve(vids.len() + 1);
        let mut idx = 0usize;
        while idx < vids.len() {
            let Some((gid, _)) = self.route(vids[idx]) else {
                offsets.push(out.len());
                idx += 1;
                continue;
            };
            let mut run_end = idx + 1;
            while run_end < vids.len() {
                match self.route(vids[run_end]) {
                    Some((next_gid, _)) if next_gid == gid => run_end += 1,
                    _ => break,
                }
            }
            let Some(shard) = self.shards.get(&gid) else {
                for _ in idx..run_end {
                    offsets.push(out.len());
                }
                idx = run_end;
                continue;
            };
            match &shard.variant {
                CsrVariant::Multiple(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::Single(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::Pure(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::Bundled(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::Frozen(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::Mapped(csr) => {
                    for vid in &vids[idx..run_end] {
                        offsets.push(out.len());
                        let (_, local) = self.route(*vid).expect("run shares one group");
                        csr.visit_physical(local, |nbr| {
                            out.push(nbr);
                            true
                        });
                    }
                }
                CsrVariant::None { .. } => {
                    for _ in idx..run_end {
                        offsets.push(out.len());
                    }
                }
            }
            idx = run_end;
        }
        offsets.push(out.len());
    }

    /// Visit every physically stored entry across groups without per-entry
    /// enum dispatch.
    ///
    /// Matches once per shard and walks that shard with its concrete
    /// `iter_all`, so a full-table scan pays one dispatch per group instead
    /// of one per entry. The visitor returns false to stop early.
    pub fn visit_all_physical<F>(&self, mut f: F)
    where
        F: FnMut(u32, Nbr) -> bool,
    {
        use super::super::CsrVariant;
        use super::group_base;
        for (gid, shard) in &self.shards {
            let base = group_base(*gid, self.group_bits);
            let keep_going = match &shard.variant {
                CsrVariant::Multiple(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::Single(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::Pure(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::Bundled(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::Frozen(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::Mapped(csr) => {
                    let mut cont = true;
                    for (local, nbr) in csr.iter_all() {
                        let Some(local) = local.as_internal_u32() else {
                            continue;
                        };
                        let global = local.saturating_add(base);
                        if !f(global, nbr) {
                            cont = false;
                            break;
                        }
                    }
                    cont
                }
                CsrVariant::None { .. } => true,
            };
            if !keep_going {
                return;
            }
        }
    }

    /// Whether the live entries of one row arrive in key order.
    ///
    /// Only frozen, mapped and single-slot rows promise order and may use
    /// bisection; other variants report an observation that is memory-only,
    /// rebuilt on load and never cached across restarts.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return true;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.is_row_sorted(local))
    }

    /// Sort one row on the maintenance path. Positions for the row go stale.
    pub fn sort_row(&mut self, src_vid: u32) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards
            .get_mut(&gid)
            .is_some_and(|shard| shard.variant.sort_row(local))
    }

    /// Visit live entries of one row whose key falls in the inclusive range.
    ///
    /// Pure and bundled rows ignore the rank halves by contract: callers
    /// pass endpoint intervals there.
    pub fn visit_threshold<F>(
        &self,
        src_vid: u32,
        lower: Option<(u32, i64)>,
        upper: Option<(u32, i64)>,
        f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            shard.variant.visit_threshold(local, lower, upper, f);
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
}
