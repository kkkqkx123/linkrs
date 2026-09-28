//! Routed physical read helpers over the shard set.
//!
//! Reads resolve rows through `route`, never creating groups: missing groups
//! read as empty.
//!
//! Batch scans group consecutive same-group vertices into runs so each run
//! borrows its shard once, then walks rows through the `CsrVariant`
//! single-row entries. Dispatch itself stays converged in the variant: these
//! callers never match on the form, so adding a form only touches the
//! variant read entries.

use graphdb_core::types::EdgeId;

use super::super::{HotNbr, MutableCsrTrait, Nbr};
use super::CsrShardSet;

impl CsrShardSet {
    /// Whether the primary row of one vertex holds `edge_id`.
    pub fn primary_contains(&self, src_vid: u32, edge_id: EdgeId) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return false;
        };
        self.shards.get(&gid).is_some_and(|shard| {
            if let Some(mapped) = &shard.mapped {
                return mapped.primary_contains(local, edge_id);
            }
            shard.variant.primary_contains(local, edge_id)
        })
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
    pub fn visit_physical<F>(&self, src_vid: u32, mut f: F)
    where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            let mut visited = 0usize;
            let wrapped = |nbr: Nbr| {
                visited += 1;
                f(nbr)
            };
            if let Some(mapped) = &shard.mapped {
                mapped.visit_physical(local, wrapped);
            } else {
                shard.variant.visit_physical(local, wrapped);
            }
            if visited > super::super::mutable_csr::row::HIGH_DEGREE_LIVE_THRESHOLD {
                log::warn!(
                    "slow topology traversal at row {} (group {} local {}): degree {} exceeds wide threshold",
                    src_vid,
                    gid,
                    local,
                    visited
                );
            }
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
            if let Some(mapped) = &shard.mapped {
                mapped.visit_hot(local, f);
            } else {
                shard.variant.visit_hot(local, f);
            }
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
            Some(shard) => {
                if let Some(mapped) = &shard.mapped {
                    mapped.fill_physical_into(local, out);
                } else {
                    shard.variant.fill_physical_into(local, out);
                }
            }
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
    /// Dispatch stays converged in `CsrVariant`: each run borrows its shard
    /// once (one map lookup per run), then walks rows through the variant
    /// single-row entry. The row loop pays one variant match per row, which
    /// is noise next to the per-row routing plus the row walk itself; in
    /// return there are no per-form arms to keep in sync here, so adding a
    /// form only touches the variant read entries.
    pub fn fill_physical_batch_into(
        &self,
        vids: &[u32],
        out: &mut Vec<Nbr>,
        offsets: &mut Vec<usize>,
    ) {
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
            let variant = &shard.variant;
            let mapped = shard.mapped.clone();
            for vid in &vids[idx..run_end] {
                offsets.push(out.len());
                let (_, local) = self.route(*vid).expect("run shares one group");
                if let Some(mapped) = &mapped {
                    mapped.visit_physical(local, |nbr| {
                        out.push(nbr);
                        true
                    });
                } else {
                    variant.visit_physical(local, |nbr| {
                        out.push(nbr);
                        true
                    });
                }
            }
            idx = run_end;
        }
        offsets.push(out.len());
    }

    /// Fill one shared buffer with paired topology plus inline values.
    ///
    /// Batched counterpart of the single-row paired fill: same offsets
    /// contract as the topology batch above, but each entry carries its
    /// inline value. Non-bundled forms fill with `None` values through the
    /// same walk. Like the topology batch, each run borrows its shard once
    /// and walks rows through the variant paired entry, so there are no
    /// per-form arms to keep in sync here either.
    pub fn fill_physical_with_values_batch_into(
        &self,
        vids: &[u32],
        out: &mut Vec<(Nbr, Option<u64>)>,
        offsets: &mut Vec<usize>,
    ) {
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
            let variant = &shard.variant;
            let mapped = shard.mapped.clone();
            for vid in &vids[idx..run_end] {
                offsets.push(out.len());
                let (_, local) = self.route(*vid).expect("run shares one group");
                if let Some(mapped) = &mapped {
                    mapped.visit_physical_with_values(local, |nbr, value| {
                        out.push((nbr, value));
                        true
                    });
                } else {
                    variant.visit_physical_with_values(local, |nbr, value| {
                        out.push((nbr, value));
                        true
                    });
                }
            }
            idx = run_end;
        }
        offsets.push(out.len());
    }

    /// Whether the live entries of one row arrive in key order.
    ///
    /// Only frozen and single-slot rows promise order and may use bisection;
    /// other variants report an observation that is memory-only, rebuilt on
    /// load and never cached across restarts. Planning paths prefer
    /// `should_use_bisection` below so the promise plus the live observation
    /// stay in one place. Frozen groups serve the same sorted order from the
    /// heap and from a derived mapping alike.
    pub fn is_row_sorted(&self, src_vid: u32) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return true;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.is_row_sorted(local))
    }

    /// Whether a range scan should bisect this row.
    ///
    /// Central plan selection combining the order promise with the live
    /// sorted observation. Missing groups read as sorted empty. The result
    /// is memory-only and must never be cached across restarts.
    pub fn should_use_bisection(&self, src_vid: u32) -> bool {
        let Some((gid, local)) = self.route(src_vid) else {
            return true;
        };
        self.shards
            .get(&gid)
            .is_some_and(|shard| shard.variant.should_use_bisection(local))
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
            if let Some(mapped) = &shard.mapped {
                mapped.visit_threshold(local, lower, upper, f);
            } else {
                shard.variant.visit_threshold(local, lower, upper, f);
            }
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

    /// Visit entries whose endpoint falls in the inclusive endpoint range.
    ///
    /// Endpoint-only counterpart of `visit_threshold` sharing one interval
    /// across forms. Missing groups read as empty and never create groups.
    pub fn visit_threshold_endpoint_only<F>(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        f: F,
    ) where
        F: FnMut(Nbr) -> bool,
    {
        let Some((gid, local)) = self.route(src_vid) else {
            return;
        };
        if let Some(shard) = self.shards.get(&gid) {
            if let Some(mapped) = &shard.mapped {
                mapped.visit_threshold(
                    local,
                    lower.map(|endpoint| (endpoint, i64::MIN)),
                    upper.map(|endpoint| (endpoint, i64::MAX)),
                    f,
                );
            } else {
                shard
                    .variant
                    .visit_threshold_endpoint_only(local, lower, upper, f);
            }
        }
    }

    /// Fill a caller buffer with the same endpoint-range content.
    pub fn fill_threshold_endpoint_only_into(
        &self,
        src_vid: u32,
        lower: Option<u32>,
        upper: Option<u32>,
        out: &mut Vec<Nbr>,
    ) {
        out.clear();
        self.visit_threshold_endpoint_only(src_vid, lower, upper, |nbr| {
            out.push(nbr);
            true
        });
    }
}
