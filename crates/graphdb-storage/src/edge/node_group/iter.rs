//! Cross-group scan iterator and its constructors.
//!
//! Chains every existing group in group order, translating local rows back
//! to global vertex ids. Missing groups are absent and never visited.

use graphdb_core::types::{Timestamp, VertexId};
use std::collections::BTreeMap;

use super::super::csr_variant::{CsrIterator, CsrRowIter};
use super::super::Nbr;
use super::{group_base, CsrShardSet, Shard};

/// Iterator chaining every existing group in group order, translating local
/// rows to global vertex ids. Missing groups are absent and never visited.
pub struct ShardCsrIterator<'a> {
    shards: &'a BTreeMap<usize, Shard>,
    order: Vec<usize>,
    group_bits: u32,
    group_pos: usize,
    base: u32,
    inner: CsrIterator<'a>,
    include_deleted: bool,
    ts: Timestamp,
}

impl<'a> ShardCsrIterator<'a> {
    fn new(
        shards: &'a BTreeMap<usize, Shard>,
        group_bits: u32,
        ts: Timestamp,
        include_deleted: bool,
    ) -> Self {
        Self {
            shards,
            order: shards.keys().copied().collect(),
            group_bits,
            group_pos: 0,
            base: 0,
            inner: CsrIterator::None,
            include_deleted,
            ts,
        }
    }

    fn advance_group(&mut self) -> bool {
        if self.group_pos >= self.order.len() {
            return false;
        }
        let gid = self.order[self.group_pos];
        self.group_pos += 1;
        self.base = group_base(gid, self.group_bits);
        let Some(shard) = self.shards.get(&gid) else {
            return self.advance_group();
        };
        let variant = &shard.variant;
        self.inner = if self.include_deleted {
            variant.iter_all()
        } else {
            variant.iter(self.ts)
        };
        true
    }
}

impl<'a> Iterator for ShardCsrIterator<'a> {
    type Item = (VertexId, Nbr);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some((local, nbr)) = self.inner.next() {
                let global = local.as_int64().unwrap_or(0) + self.base as i64;
                return Some((VertexId::from_int64(global), nbr));
            }
            if !self.advance_group() {
                return None;
            }
        }
    }
}

impl CsrShardSet {
    /// Iterate edges of a vertex without allocating, for every strategy.
    /// Test-only row-stamp filtered iterator; production scans go through the version authority.
    pub fn iter_edges_of(&self, src_vid: u32, ts: Timestamp) -> Option<CsrRowIter<'_>> {
        let (gid, local) = self.route(src_vid)?;
        self.shards.get(&gid)?.variant.iter_edges_of(local, ts)
    }

    /// Iterate all live edges across groups in group order.
    pub fn iter(&self, ts: Timestamp) -> ShardCsrIterator<'_> {
        ShardCsrIterator::new(&self.shards, self.group_bits, ts, false)
    }

    /// Iterate every physically present entry across groups, including
    /// tombstoned ones, in group order.
    pub fn iter_all(&self) -> ShardCsrIterator<'_> {
        ShardCsrIterator::new(&self.shards, self.group_bits, 0, true)
    }
}
