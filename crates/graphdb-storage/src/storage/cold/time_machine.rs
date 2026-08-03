//! Multi-version cold snapshot registry (time travel).
//!
//! Holds an immutable snapshot shelf per edge label, keyed by snapshot
//! timestamp. Querying `snapshot_at(label, ts)` routes to the most recent
//! snapshot not newer than `ts`, which is the basis for historical reads:
//! a label's cold history is a chain of full snapshots at increasing
//! timestamps (optionally compressed into delta chains, see `delta`).
//!
//! On-disk layout (loaded by [`ColdSnapshotTimeMachine::load_from_dir`]):
//! ```text
//! {dir}/
//!   edges/
//!     {label_name}/
//!       1000.lkcs    # snapshot at ts=1000
//!       2000.lkcs    # snapshot at ts=2000
//! ```
//! Flat `.lkcs` files directly under `{dir}` are also accepted; label names
//! are resolved from the file metadata, not the directory path.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::core::types::{LabelId, Timestamp};
use crate::core::StorageResult;

use super::ColdSnapshot;

/// Timestamp-sorted immutable snapshot shelves, one per edge label.
#[derive(Debug, Clone, Default)]
pub struct ColdSnapshotTimeMachine {
    shelves: HashMap<LabelId, BTreeMap<Timestamp, Arc<ColdSnapshot>>>,
}

impl ColdSnapshotTimeMachine {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build shelves from a set of snapshots. Later timestamps win when two
    /// snapshots of the same label share a timestamp.
    pub fn with_snapshots<I: IntoIterator<Item = ColdSnapshot>>(snapshots: I) -> Self {
        let mut machine = Self::new();
        for snapshot in snapshots {
            machine.insert(snapshot);
        }
        machine
    }

    /// Register a snapshot on its label's shelf. A snapshot with the same
    /// timestamp replaces the previous one.
    pub fn insert(&mut self, snapshot: ColdSnapshot) {
        self.insert_arc(Arc::new(snapshot));
    }

    pub fn insert_arc(&mut self, snapshot: Arc<ColdSnapshot>) {
        let ts = snapshot.snapshot_ts();
        self.shelves
            .entry(snapshot.label())
            .or_default()
            .insert(ts, snapshot);
    }

    /// Select the most recent snapshot of `label` not newer than `ts`
    /// (`range(..=ts).next_back()`).
    pub fn snapshot_at(&self, label: LabelId, ts: Timestamp) -> Option<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)?
            .range(..=ts)
            .next_back()
            .map(|(_, v)| v.clone())
    }

    /// Newest snapshot of `label`, regardless of timestamp.
    pub fn latest(&self, label: LabelId) -> Option<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)?
            .iter()
            .next_back()
            .map(|(_, v)| v.clone())
    }

    /// Oldest snapshot of `label`.
    pub fn earliest(&self, label: LabelId) -> Option<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)?
            .iter()
            .next()
            .map(|(_, v)| v.clone())
    }

    /// All snapshots of `label` in ascending timestamp order.
    pub fn versions(&self, label: LabelId) -> Vec<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)
            .map(|shelf| shelf.values().cloned().collect())
            .unwrap_or_default()
    }

    /// Snapshots of `label` with `from <= ts <= to`, ascending.
    pub fn versions_between(
        &self,
        label: LabelId,
        from: Timestamp,
        to: Timestamp,
    ) -> Vec<Arc<ColdSnapshot>> {
        self.shelves
            .get(&label)
            .map(|shelf| shelf.range(from..=to).map(|(_, v)| v.clone()).collect())
            .unwrap_or_default()
    }

    /// Drop the whole shelf of `label`, returning the removed snapshots
    /// (ascending timestamp order).
    pub fn remove(&mut self, label: LabelId) -> Option<Vec<Arc<ColdSnapshot>>> {
        self.shelves
            .remove(&label)
            .map(|shelf| shelf.into_values().collect())
    }

    pub fn labels(&self) -> Vec<LabelId> {
        self.shelves.keys().copied().collect()
    }

    pub fn version_count(&self, label: LabelId) -> usize {
        self.shelves
            .get(&label)
            .map(|shelf| shelf.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.shelves.is_empty()
    }

    /// Scan a snapshot directory (recursively, accepting both the
    /// `edges/{label}/{ts}.lkcs` layout and flat files) and register every
    /// readable `.lkcs` snapshot.
    pub fn load_from_dir(&mut self, dir: &Path) -> StorageResult<usize> {
        let mut count = 0usize;
        let mut paths = Vec::new();
        collect_lkcs_into(dir, &mut paths);
        for path in paths {
            match ColdSnapshot::open(&path) {
                Ok(snapshot) => {
                    self.insert(snapshot);
                    count += 1;
                }
                Err(err) => {
                    log::warn!(
                        "skipping unreadable cold snapshot {}: {}",
                        path.display(),
                        err
                    );
                }
            }
        }
        Ok(count)
    }
}

fn collect_lkcs_into(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_lkcs_into(&path, out);
        } else if path.extension().is_some_and(|e| e == "lkcs") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::Value;
    use crate::storage::edge::edge_table::core::{EdgeTableConfig, TimeTravelEdgeStore};
    use crate::storage::edge::{EdgeSchema, EdgeStrategy};
    use crate::storage::types::StoragePropertyDef;

    fn make_snapshot(ts: Timestamp, edges: &[(u32, u32)]) -> ColdSnapshot {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };
        let mut table =
            TimeTravelEdgeStore::with_config(schema, EdgeTableConfig::default()).unwrap();
        for (src, dst) in edges {
            table
                .insert_edge(
                    *src,
                    *dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    ts,
                )
                .unwrap();
        }
        let exported = table.export_snapshot(ts).unwrap();
        let dir = tempfile::tempdir().unwrap();
        ColdSnapshot::create(&exported, dir.path().join(format!("{}.lkcs", ts))).unwrap()
    }

    #[test]
    fn test_snapshot_at_selects_most_recent_not_newer_than_ts() {
        let mut machine = ColdSnapshotTimeMachine::new();
        let s1 = make_snapshot(100, &[(0, 1)]);
        let s2 = make_snapshot(200, &[(0, 1), (0, 2)]);
        let s3 = make_snapshot(300, &[(0, 1), (0, 2), (0, 3)]);
        machine.insert(s1);
        machine.insert(s2);
        machine.insert(s3);

        assert_eq!(machine.version_count(0), 3);
        assert!(machine.snapshot_at(0, 0).is_none());
        assert_eq!(machine.snapshot_at(0, 100).unwrap().edge_count(), 1);
        assert_eq!(machine.snapshot_at(0, 150).unwrap().edge_count(), 1);
        assert_eq!(machine.snapshot_at(0, 200).unwrap().edge_count(), 2);
        assert_eq!(machine.snapshot_at(0, 999).unwrap().edge_count(), 3);
        assert!(machine.snapshot_at(42, 100).is_none());

        assert_eq!(machine.latest(0).unwrap().edge_count(), 3);
        assert_eq!(machine.earliest(0).unwrap().edge_count(), 1);
        assert_eq!(machine.versions_between(0, 150, 250).len(), 1);
        assert_eq!(machine.versions(0).len(), 3);

        let removed = machine.remove(0).unwrap();
        assert_eq!(removed.len(), 3);
        assert!(machine.is_empty());
    }

    #[test]
    fn test_time_machine_load_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        let edges_dir = dir.path().join("edges").join("knows");
        std::fs::create_dir_all(&edges_dir).unwrap();

        let schema = EdgeSchema {
            label_id: 0,
            label_name: "knows".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef::new(
                "weight".to_string(),
                crate::core::types::DataType::Double,
            )],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
        };

        // Two snapshots written directly into the nested layout.
        for (ts, src, dst) in [(100u64, 0u32, 1u32), (200u64, 0u32, 2u32)] {
            let mut table =
                TimeTravelEdgeStore::with_config(schema.clone(), EdgeTableConfig::default())
                    .unwrap();
            table
                .insert_edge(
                    src,
                    dst,
                    0,
                    &[("weight".to_string(), Value::Double(1.0))],
                    ts,
                )
                .unwrap();
            let exported = table.export_snapshot(ts).unwrap();
            ColdSnapshot::create(&exported, edges_dir.join(format!("{}.lkcs", ts))).unwrap();
        }

        let mut machine = ColdSnapshotTimeMachine::new();
        let count = machine.load_from_dir(dir.path()).unwrap();
        assert_eq!(count, 2);
        assert_eq!(machine.version_count(0), 2);
        assert_eq!(machine.latest(0).unwrap().snapshot_ts(), 200);
    }
}
