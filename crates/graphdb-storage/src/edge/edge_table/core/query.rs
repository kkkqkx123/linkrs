//! Predicate pushdown and per-group segment pruning statistics.

use super::super::stats::GroupSegmentStats;
use super::EdgeStore;
use crate::cursor::ScanPredicate;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::Value;
use std::collections::{HashMap, HashSet};

impl EdgeStore {
    /// Whether one edge matches every pushed predicate at `query_ts`.
    ///
    /// Column-scan pushdown: only predicate columns are read through their
    /// null bitmaps, with no intermediate record materialization. Topology
    /// and attribute checks share the version authority: invisible edges
    /// never match even when their projected row still carries values.
    pub fn matches_pushdown(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        predicates: &[ScanPredicate],
    ) -> bool {
        self.observe_form_read(1);
        if !self.is_visible(edge_id, query_ts) {
            return false;
        }
        if self.is_bundled() {
            // Single inline value evaluated with the shared predicate
            // matcher; a NULL slot yields no properties so nothing matches,
            // mirroring the columnar NULL semantics.
            let props = self.bundled_scan_properties(edge_id, query_ts, None);
            return predicates.iter().all(|p| p.matches(&props));
        }
        self.properties
            .matches_predicates_for_edge(edge_id, query_ts, predicates)
    }

    /// Predicate match on an already-visible edge.
    ///
    /// Counterpart of [`Self::matches_pushdown`] for callers holding a
    /// prior authority verdict (scan iterators, batch filters): the column
    /// check runs without a second visibility lookup.
    pub fn matches_pushdown_assume_visible(
        &self,
        edge_id: EdgeId,
        query_ts: Timestamp,
        predicates: &[ScanPredicate],
    ) -> bool {
        if self.is_bundled() {
            let props = self.bundled_scan_properties_assume_visible(edge_id, query_ts, None);
            return predicates.iter().all(|p| p.matches(&props));
        }
        self.properties
            .matches_predicates_for_edge(edge_id, query_ts, predicates)
    }

    /// Filter edge ids by pushed predicates at the column-scan layer.
    ///
    /// Hits look up topology afterwards; misses never decode a record.
    /// Index first, segment statistics second, full walk last: an
    /// equality-or-range conjunction over lag-free indexed columns resolves to
    /// index candidates (verified back against the columns), otherwise whole
    /// owner groups provably excluding the predicates are skipped before the
    /// property walk. A full walk without either aid logs an observation so
    /// operators know an index would help.
    pub fn filter_edge_ids(
        &self,
        predicates: &[ScanPredicate],
        query_ts: Timestamp,
        candidates: Option<&[EdgeId]>,
    ) -> Vec<EdgeId> {
        self.observe_form_read(candidates.map(|c| c.len() as u64).unwrap_or(1));
        if self.is_bundled() {
            // The stub columnar store holds no rows, so the walk runs over
            // the out-direction topology with per-edge predicate checks.
            // Visibility rides inside `matches_pushdown` via the bundled
            // scan, which returns no properties for invisible edges.
            if let Some(ids) = candidates {
                return ids
                    .iter()
                    .copied()
                    .filter(|edge_id| self.matches_pushdown(*edge_id, query_ts, predicates))
                    .collect();
            }
            // Row-addressed walk: the iterator already yields the owning row,
            // so each edge pays one visibility check plus one inline decode
            // instead of a full-table row scan per edge.
            let mut out = Vec::new();
            for (src_vid, nbr) in self.out_csr.iter_all() {
                if !self.is_visible(nbr.edge_id, query_ts) {
                    continue;
                }
                let Some(row) = src_vid.as_internal_u32() else {
                    continue;
                };
                let props = self.bundled_properties_at_assume_visible(
                    true,
                    row,
                    nbr.edge_id,
                    query_ts,
                    None,
                );
                if predicates.iter().all(|p| p.matches(&props)) {
                    out.push(nbr.edge_id);
                }
            }
            return out;
        }
        if let Some(ids) = candidates {
            let hits =
                self.properties
                    .filter_edge_ids_by_predicates(predicates, query_ts, Some(ids));
            return self.visible_only(hits, query_ts);
        }
        if predicates.is_empty() {
            let hits = self
                .properties
                .filter_edge_ids_by_predicates(predicates, query_ts, None);
            return self.visible_only(hits, query_ts);
        }
        if let Some(indexed) = self.index_candidate_edge_ids(predicates, query_ts) {
            let hits =
                self.properties
                    .filter_edge_ids_by_predicates(predicates, query_ts, Some(&indexed));
            return self.visible_only(hits, query_ts);
        }
        let pruned = self.pruned_owner_groups(predicates);
        if pruned.is_empty() {
            log::debug!(
                "filter_edge_ids: no usable index or segment prune, full property walk over {} rows",
                self.properties.row_count()
            );
            let hits = self
                .properties
                .filter_edge_ids_by_predicates(predicates, query_ts, None);
            return self.visible_only(hits, query_ts);
        }
        let survivors: Vec<EdgeId> = self
            .properties
            .edge_ids()
            .filter(|edge_id| {
                self.edge_owner
                    .get(edge_id)
                    .is_none_or(|owner| !pruned.contains(&owner))
            })
            .collect();
        let hits =
            self.properties
                .filter_edge_ids_by_predicates(predicates, query_ts, Some(&survivors));
        self.visible_only(hits, query_ts)
    }

    fn visible_only(&self, hits: Vec<EdgeId>, query_ts: Timestamp) -> Vec<EdgeId> {
        hits.into_iter()
            .filter(|edge_id| self.is_visible(*edge_id, query_ts))
            .collect()
    }

    /// Owner groups provably excluding the pushed predicates.
    ///
    /// Dirty groups never prune: their uncheckpointed writes are not covered
    /// by the flushed statistics, mirroring `segment_may_contain`.
    fn pruned_owner_groups(&self, predicates: &[ScanPredicate]) -> HashSet<u32> {
        let mut pruned = HashSet::new();
        for gid in self.owner_group_ids() {
            if !self.segment_may_contain(gid, predicates) {
                pruned.insert(gid);
            }
        }
        pruned
    }

    /// Whether one owner group may contain rows matching the predicates.
    ///
    /// Dirty groups always scan: their uncheckpointed writes are not covered
    /// by the flushed statistics. Clean groups prune only on provable
    /// exclusion from the widened bounds, so the pre-filter never changes
    /// results.
    pub fn segment_may_contain(&self, group: u32, predicates: &[ScanPredicate]) -> bool {
        if predicates.is_empty() {
            return true;
        }
        let gid = group as usize;
        if self.out_csr.needs_checkpoint(gid) || self.in_csr.needs_checkpoint(gid) {
            return true;
        }
        let Some(stats) = self.segment_stats.get(&group) else {
            return true;
        };
        stats.may_contain(predicates)
    }

    /// Snapshot of the in-memory segment statistics.
    pub fn segment_stats_snapshot(&self) -> HashMap<u32, GroupSegmentStats> {
        self.segment_stats.clone()
    }

    /// Restore segment statistics decoded from a checkpoint.
    pub(crate) fn restore_segment_stats(&mut self, stats: HashMap<u32, GroupSegmentStats>) {
        self.segment_stats = stats;
    }

    /// Collect fresh per-group segment statistics and widen the in-memory
    /// snapshot. Only groups touched since the last checkpoint are
    /// recollected; clean groups keep their previous snapshot so checkpoint
    /// cost follows dirty groups rather than table size. Bounds widen
    /// monotonically so pruning stays conservative; counts are exact-current.
    pub(crate) fn refresh_segment_stats(&mut self) {
        use std::collections::{HashMap, HashSet};
        let use_out = self.schema.oe_strategy != super::super::super::EdgeStrategy::None;
        let existing: Vec<u32> = if use_out {
            self.out_csr.existing_group_ids()
        } else {
            self.in_csr.existing_group_ids()
        }
        .into_iter()
        .map(|gid| gid as u32)
        .collect();
        let live_set: HashSet<u32> = existing.iter().copied().collect();
        self.segment_stats
            .retain(|group, _| live_set.contains(group));
        let mut dirty: HashSet<u32> = HashSet::new();
        for gid in self.out_csr.dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.in_csr.dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.out_csr.column_dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in self.in_csr.column_dirty_group_ids() {
            dirty.insert(gid as u32);
        }
        for gid in &existing {
            if !self.segment_stats.contains_key(gid) {
                dirty.insert(*gid);
            }
        }
        dirty.retain(|gid| live_set.contains(gid));
        if dirty.is_empty() {
            return;
        }
        let group_ids: Vec<u32> = dirty.into_iter().collect();
        let group_size = if use_out {
            self.out_csr.group_size()
        } else {
            self.in_csr.group_size()
        } as u64;
        let column_names: Vec<String> = self
            .schema
            .properties
            .iter()
            .map(|prop| prop.name.clone())
            .collect();
        let mut encodings: HashMap<String, crate::encoding::EncodingType> = HashMap::new();
        for name in &column_names {
            if let Some(encoding) = self.properties.column_encoding_type(name) {
                encodings.insert(name.clone(), encoding);
            }
        }
        let mut by_owner: HashMap<u32, Vec<EdgeId>> = HashMap::new();
        for group in &group_ids {
            by_owner.insert(*group, Vec::new());
        }
        // Group only the dirty owners: iterate the authority owner map once
        // and file each edge into its dirty group slot, so the grouping pass
        // touches one map entry per edge with no second lookup and decodes
        // properties for dirty groups only below.
        for (edge_id, owner) in self.edge_owner.iter() {
            if let Some(slot) = by_owner.get_mut(&owner) {
                slot.push(edge_id);
            }
        }
        for group in group_ids {
            let gid = group as usize;
            let live = if use_out {
                self.out_csr.group_live_count(gid)
            } else {
                self.in_csr.group_live_count(gid)
            };
            let owned = by_owner.get(&group);
            let mut endpoints: Vec<u32> = Vec::new();
            if let Some(variant) = if use_out {
                self.out_csr.group_variant(gid)
            } else {
                self.in_csr.group_variant(gid)
            } {
                for (_, nbr) in variant.iter_all() {
                    endpoints.push(nbr.endpoint);
                }
            }
            let mut column_values: HashMap<String, Vec<Option<Value>>> = HashMap::new();
            for name in &column_names {
                column_values.insert(name.clone(), Vec::new());
            }
            if let Some(edges) = owned {
                for edge_id in edges {
                    if let Some(cells) = self.properties.get_projected_physical_by_edge_id(
                        *edge_id,
                        graphdb_core::types::MAX_TIMESTAMP,
                        None,
                    ) {
                        let cell_map: HashMap<&String, &Option<Value>> =
                            cells.iter().map(|(name, value)| (name, value)).collect();
                        for name in &column_names {
                            let value = cell_map.get(name).and_then(|cell| (*cell).clone());
                            if let Some(slot) = column_values.get_mut(name) {
                                slot.push(value);
                            }
                        }
                    }
                }
            }
            let fresh = GroupSegmentStats::collect(
                group,
                group_size,
                live,
                &endpoints,
                &column_values,
                &encodings,
            );
            match self.segment_stats.get_mut(&group) {
                Some(current) => current.widen_with(&fresh),
                None => {
                    self.segment_stats.insert(group, fresh);
                }
            }
        }
    }

    /// Encoding report for the persisted topology columns of both
    /// directions. Neighbor and edge-id columns come first, offset and
    /// length columns follow; all use the integer column path only.
    pub fn topology_encoding_report(
        &self,
    ) -> Vec<(
        String,
        crate::edge::mutable_csr::serialization::TopologyColumnEncoding,
        usize,
        usize,
    )> {
        let mut report = Vec::new();
        for gid in self.out_csr.existing_group_ids() {
            if let Some(variant) = self.out_csr.group_variant(gid) {
                match variant {
                    super::super::super::CsrVariant::Multiple(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("out_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    super::super::super::CsrVariant::Frozen(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("out_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    super::super::super::CsrVariant::Mapped(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("out_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        for gid in self.in_csr.existing_group_ids() {
            if let Some(variant) = self.in_csr.group_variant(gid) {
                match variant {
                    super::super::super::CsrVariant::Multiple(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("in_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    super::super::super::CsrVariant::Frozen(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("in_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    super::super::super::CsrVariant::Mapped(csr) => {
                        for (name, encoding, plain, encoded) in csr.topology_encoding_report() {
                            report.push((
                                format!("in_g{}:{}", gid, name),
                                encoding,
                                plain,
                                encoded,
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }
        report
    }
}
