//! Manifest commit protocol: metadata file carrying the manifest tail,
//! published last as the snapshot commit point.

use super::super::core::EdgeStore;
use super::super::persistence;
use super::layout::manifest_path;
use crate::edge::node_group::TableShardManifest;
use graphdb_core::{StorageError, StorageResult};
use std::path::Path;

impl EdgeStore {
    pub(crate) fn flush_metadata_file(
        &self,
        dir: &Path,
        page_size: usize,
        level: i32,
        manifest: &TableShardManifest,
    ) -> StorageResult<u64> {
        // Header-only rewrite: timestamps live in per-group shards falling
        // with the same dirt as their owner, so any timestamp change needs
        // only dirty owners' shards. The manifest commit tail is appended so
        // metadata and manifest share one atomic unit: a lone new metadata
        // file without its manifest commit is a torn write.
        let mut meta_payload = Vec::new();
        crate::persistence::write_header_to(
            &mut meta_payload,
            crate::persistence::section::EDGE_META,
        )
        .map_err(|e| StorageError::io_error(format!("Failed to write edge meta header: {}", e)))?;
        persistence::flush_metadata(
            &mut meta_payload,
            self.label,
            self.src_label,
            self.dst_label,
            &self.label_name,
            self.is_open,
            &self.schema,
            self.next_edge_id,
        )?;
        meta_payload.extend_from_slice(&manifest.encode());
        let path = dir.join("meta.bin");
        persistence::write_pages_to_file(&path, &meta_payload, page_size, level, 1)?;
        Ok(super::layout::file_bytes(&path))
    }

    /// Publish the manifest as the snapshot commit point. Must be called
    /// after the metadata file carrying the same manifest tail is durable.
    pub(crate) fn write_manifest(
        &self,
        dir: &Path,
        manifest: &TableShardManifest,
    ) -> StorageResult<()> {
        crate::compression::write_shadow_file(manifest_path(dir), &manifest.encode())
    }

    pub(crate) fn load_metadata_file(
        &mut self,
        dir: &Path,
        _file_manifest: &TableShardManifest,
    ) -> StorageResult<TableShardManifest> {
        use std::io::Read;
        let meta_path = dir.join("meta.bin");
        let (meta_data, _meta_rows) = persistence::read_pages_from_file(&meta_path)?;
        let mut meta_cursor = &meta_data[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        meta_cursor.read_exact(&mut header_buf)?;
        {
            let mut slice = &header_buf[..];
            let (_version, sid) = crate::persistence::read_header(&mut slice)?;
            if sid != crate::persistence::section::EDGE_META {
                return Err(StorageError::deserialize_error(format!(
                    "unexpected section id in edge meta: expected {:#06x}, got {:#06x}",
                    crate::persistence::section::EDGE_META,
                    sid
                )));
            }
        }

        let mut version_bytes = [0u8; 4];
        meta_cursor.read_exact(&mut version_bytes)?;
        let version = u32::from_le_bytes(version_bytes);
        if version == 1 {
            return Err(StorageError::deserialize_error(
                "legacy edge meta version 1 without a manifest commit tail is not supported"
                    .to_string(),
            ));
        }
        if version == 2 {
            return Err(StorageError::deserialize_error(
                "legacy edge meta version 2 with global timestamps is not supported".to_string(),
            ));
        }
        if version != persistence::EDGE_META_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported edge meta version: {}",
                version
            )));
        }

        let meta = persistence::load_metadata(&mut meta_cursor)?;
        let embedded = TableShardManifest::decode(meta_cursor).map_err(|_| {
            StorageError::deserialize_error(
                "edge meta missing manifest commit tail: torn write or legacy file".to_string(),
            )
        })?;
        self.label = meta.label;
        self.src_label = meta.src_label;
        self.dst_label = meta.dst_label;
        self.label_name = meta.label_name;
        self.is_open = meta.is_open;
        self.set_schema(meta.schema);
        self.next_edge_id = meta.next_edge_id;
        self.mvcc.edge_timestamps.clear();
        self.mvcc.min_active_snapshot_ts = graphdb_core::types::Timestamp::MAX;
        self.mvcc.active_snapshots.clear();
        Ok(embedded)
    }
}
