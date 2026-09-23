use super::import_export::{format_csv_value, import_space_impl};
use super::{CatalogStore, GraphStore, StorageAdmin, StorageAuthOps, StoragePersistenceOps};
use crate::stats_reader::ColumnStatsReader;
use crate::{AutoCommitBatchOps, AutoCommitGroupOps, SnapshotHandle};
use graphdb_core::StorageError;
use std::io::Write;

/// Minimal combined capability required by the query crate.
pub trait QueryStorage:
    GraphStore
    + CatalogStore
    + StorageAuthOps
    + StorageAdmin
    + StoragePersistenceOps
    + ColumnStatsReader
    + AutoCommitBatchOps
    + AutoCommitGroupOps
{
    /// Snapshot handle bound to this storage handle, when the handle is
    /// bound to an operation context with a pinned read/write snapshot.
    ///
    /// Always reports the operation context's read timestamp: storage reads
    /// execute at the read timestamp, so reporting it tells the truth about
    /// the observed snapshot. (Write contexts additionally pin a write
    /// timestamp for visibility ordering, but reads never execute at it;
    /// reporting it here made read-committed statements look inconsistent
    /// after a statement refresh moved the read stamp past the write stamp.)
    /// Unbound handles (raw global storage) return `None`. This lets the
    /// query layer observe which snapshot a per-query bound handle reads at,
    /// without reaching into the storage internals.
    ///
    /// Snapshot truth lives in the global tracker, so the handle is always
    /// synthesized from the pinned timestamp (`id = 0`).
    fn snapshot_handle(&self) -> Option<SnapshotHandle> {
        let context = self.operation_context()?;
        Some(SnapshotHandle::new(context.read_timestamp, 0))
    }

    /// Export the given space to CSV files under `path/<space_name>/`.
    ///
    /// Each tag produces a `<tag>.csv` file; each edge type produces a
    /// `<edge_type>.csv` file.  A `schema.json` metadata file records the
    /// space, tags, and edge types with their property schemas.
    fn export_space(&self, space: &str, path: &std::path::Path) -> Result<(), StorageError> {
        let base = path.join(space);
        std::fs::create_dir_all(&base)
            .map_err(|e| StorageError::io_error(format!("Failed to create export dir: {e}")))?;

        // Export schema metadata
        let tags = self.list_tags(space)?;
        let edge_types = self.list_edge_types(space)?;

        let schema_meta = serde_json::json!({
            "space": space,
            "tags": tags.iter().map(|t| serde_json::json!({
                "name": t.tag_name,
                "properties": t.properties.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "type": p.data_type.to_string(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "edge_types": edge_types.iter().map(|e| serde_json::json!({
                "name": e.edge_type_name,
                "src_tag": e.src_tag_name,
                "dst_tag": e.dst_tag_name,
                "properties": e.properties.iter().map(|p| serde_json::json!({
                    "name": p.name,
                    "type": p.data_type.to_string(),
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
        });

        let schema_path = base.join("schema.json");
        let mut schema_file = std::fs::File::create(&schema_path)
            .map_err(|e| StorageError::io_error(format!("Failed to create schema.json: {e}")))?;
        schema_file
            .write_all(
                serde_json::to_string_pretty(&schema_meta)
                    .unwrap_or_default()
                    .as_bytes(),
            )
            .map_err(|e| StorageError::io_error(format!("Failed to write schema.json: {e}")))?;

        // Export vertices by tag
        for tag_info in &tags {
            let vertices = self.scan_vertices_by_tag(space, &tag_info.tag_name)?;
            if vertices.is_empty() {
                continue;
            }

            // Collect all property keys across all vertices from their tags.
            let mut prop_keys: Vec<String> = vertices
                .iter()
                .flat_map(|v| v.tag.properties.keys())
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            prop_keys.sort();

            let csv_path = base.join(format!("{}.csv", tag_info.tag_name));
            let mut file = std::fs::File::create(&csv_path).map_err(|e| {
                StorageError::io_error(format!("Failed to create {}.csv: {e}", tag_info.tag_name))
            })?;

            // Write header
            let mut header = vec!["vid".to_string()];
            header.extend(prop_keys.clone());
            writeln!(file, "{}", header.join(","))
                .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;

            // Write rows
            for vertex in &vertices {
                let mut row = vec![vertex.vid.to_string()];
                for key in &prop_keys {
                    let val = vertex
                        .property_value(key)
                        .map(|value| format_csv_value(&value))
                        .unwrap_or_default();
                    row.push(val);
                }
                writeln!(file, "{}", row.join(","))
                    .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;
            }
        }

        // Export edges by type
        for edge_info in &edge_types {
            let edges = self.scan_edges_by_type(space, &edge_info.edge_type_name)?;
            if edges.is_empty() {
                continue;
            }

            let mut prop_keys: Vec<String> = edges
                .iter()
                .flat_map(|e| e.props.keys())
                .cloned()
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            prop_keys.sort();

            let csv_path = base.join(format!("{}.csv", edge_info.edge_type_name));
            let mut file = std::fs::File::create(&csv_path).map_err(|e| {
                StorageError::io_error(format!(
                    "Failed to create {}.csv: {e}",
                    edge_info.edge_type_name
                ))
            })?;

            let mut header = vec!["src".to_string(), "dst".to_string(), "ranking".to_string()];
            header.extend(prop_keys.clone());
            writeln!(file, "{}", header.join(","))
                .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;

            for edge in &edges {
                let mut row = vec![
                    edge.src.to_string(),
                    edge.dst.to_string(),
                    edge.ranking.to_string(),
                ];
                for key in &prop_keys {
                    let val = edge
                        .props
                        .get(key)
                        .map(format_csv_value)
                        .unwrap_or_default();
                    row.push(val);
                }
                writeln!(file, "{}", row.join(","))
                    .map_err(|e| StorageError::io_error(format!("CSV write error: {e}")))?;
            }
        }

        Ok(())
    }

    /// Import a space from CSV files under `path/<space_name>/`.
    ///
    /// Expects a `schema.json` metadata file and `<tag>.csv` / `<edge_type>.csv` data files.
    fn import_space(&mut self, space: &str, path: &std::path::Path) -> Result<(), StorageError> {
        import_space_impl(self, space, path)
    }
}

impl<T> QueryStorage for T where
    T: GraphStore
        + CatalogStore
        + StorageAuthOps
        + StorageAdmin
        + StoragePersistenceOps
        + ColumnStatsReader
        + AutoCommitBatchOps
        + AutoCommitGroupOps
{
}
