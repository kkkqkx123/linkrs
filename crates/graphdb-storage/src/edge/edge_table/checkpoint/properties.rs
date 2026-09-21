//! Per-group property shard persistence and dirty tracking.

use super::super::core::EdgeStore;
use super::layout::props_group_path;
use graphdb_core::StorageResult;
use std::collections::{HashMap, HashSet};
use std::path::Path;

impl EdgeStore {
    /// Mark property columns dirty. Every operation mutating property state
    /// calls this so checkpoints can skip the property file when clean.
    pub(crate) fn mark_properties_dirty(&mut self) {
        self.properties_dirty = true;
    }

    /// Mark property columns dirty and trace the owning groups.
    ///
    /// Property-only writes leave topology files untouched, so the group
    /// trace records column dirt only and never forces a topology rewrite.
    /// The trace is precise: every write marks its owning group, and the
    /// table-level flag remains only as a correctness insurance.
    pub(crate) fn mark_properties_dirty_for_edge(&mut self, src: u32, dst: u32) {
        self.properties_dirty = true;
        self.out_csr.mark_column_updated_for(src);
        self.in_csr.mark_column_updated_for(dst);
    }

    /// Mark named property columns dirty for the edge `(src, dst)`.
    ///
    /// Column-precise counterpart of [`Self::mark_properties_dirty_for_edge`]:
    /// besides the table flag and the group trace it records which columns
    /// the owning group touched, so the incremental patch rewrites only
    /// those columns instead of the whole row. Unknown or empty column lists
    /// degrade to the traced-group behavior.
    pub(crate) fn mark_property_columns_dirty_for_edge(
        &mut self,
        src: u32,
        dst: u32,
        columns: &[String],
    ) {
        self.mark_properties_dirty_for_edge(src, dst);
        if columns.is_empty() {
            return;
        }
        let owner = self.owner_gid_for(src, dst);
        self.property_column_dirt
            .entry(owner)
            .or_default()
            .extend(columns.iter().cloned());
    }

    /// Mark property dirt for one owner group without column detail.
    ///
    /// Used when the touched columns are unknown (authority revives,
    /// property-slot reclaims): the group joins the dirty set through its
    /// trace and the flush falls back to the table-wide column set for it,
    /// so the write is never missed.
    pub(crate) fn mark_properties_dirty_for_owner(&mut self, owner: u32) {
        self.properties_dirty = true;
        if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
            self.out_csr.mark_column_updated_for_group(owner as usize);
        } else if self.schema.ie_strategy != crate::edge::EdgeStrategy::None {
            self.in_csr.mark_column_updated_for_group(owner as usize);
        }
    }

    /// Dirty columns recorded for one owner group, if any.
    ///
    /// `None` means the group carries no column-precise trace and the caller
    /// must fall back to the table-wide dirty set.
    fn property_dirty_columns_for_group(&self, gid: u32) -> Option<Vec<String>> {
        self.property_column_dirt.get(&gid).map(|set| {
            let mut out: Vec<String> = set.iter().cloned().collect();
            out.sort();
            out
        })
    }

    /// Drop per-group column traces for groups a flush persisted.
    fn clear_property_column_dirt_for_groups(&mut self, gids: &[u32]) {
        for gid in gids {
            self.property_column_dirt.remove(gid);
        }
    }

    /// Trace every existing owner group for `columns`.
    ///
    /// Schema fills, drops and renames touch rows in all owners at once, so
    /// they cannot rely on the per-edge trace left by point writes. Tracing
    /// every owner keeps those groups in the dirty set even when no other
    /// write marked them, and records the column scope for the incremental
    /// patch. Dropped columns are removed from the traces instead.
    pub(crate) fn trace_all_owner_groups_for_columns(&mut self, columns: &[String]) {
        self.properties_dirty = true;
        let owners = self.owner_group_ids();
        let use_out = self.schema.oe_strategy != crate::edge::EdgeStrategy::None;
        for gid in &owners {
            if use_out {
                self.out_csr.mark_column_updated_for_group(*gid as usize);
            } else {
                self.in_csr.mark_column_updated_for_group(*gid as usize);
            }
            if !columns.is_empty() {
                self.property_column_dirt
                    .entry(*gid)
                    .or_default()
                    .extend(columns.iter().cloned());
            }
        }
    }

    /// Forget one column in every per-group trace after a drop publish.
    pub(crate) fn forget_property_column_in_dirt(&mut self, name: &str) {
        for set in self.property_column_dirt.values_mut() {
            set.remove(name);
        }
    }

    /// Rename one column inside every per-group trace after a rename publish.
    pub(crate) fn rename_property_column_in_dirt(&mut self, old_name: &str, new_name: &str) {
        for set in self.property_column_dirt.values_mut() {
            if set.remove(old_name) {
                set.insert(new_name.to_string());
            }
        }
    }

    /// Owner groups whose timestamp or property shards must be rewritten.
    /// Topology dirt always covers inserts and deletes; precise column
    /// traces cover property-only writes. When the table flag reports
    /// property dirt but no group trace exists, every owner rewrites as a
    /// correctness insurance that the regular path never needs. Each
    /// insurance rewrite increments the fallback counter for observability.
    fn property_dirty_owners(&mut self) -> Vec<u32> {
        let (dirty, column_traced, existing) =
            if self.schema.oe_strategy != crate::edge::EdgeStrategy::None {
                (
                    self.out_csr.dirty_group_ids(),
                    self.out_csr.column_dirty_group_ids(),
                    self.out_csr.existing_group_ids(),
                )
            } else if self.schema.ie_strategy != crate::edge::EdgeStrategy::None {
                (
                    self.in_csr.dirty_group_ids(),
                    self.in_csr.column_dirty_group_ids(),
                    self.in_csr.existing_group_ids(),
                )
            } else {
                return Vec::new();
            };
        let mut set: HashSet<u32> = HashSet::new();
        for gid in dirty.into_iter().chain(column_traced) {
            set.insert(gid as u32);
        }
        if set.is_empty() && self.properties_dirty {
            for gid in existing {
                set.insert(gid as u32);
            }
            self.property_fallback_rewrites += 1;
        }
        let mut out: Vec<u32> = set.into_iter().collect();
        out.sort_unstable();
        out
    }

    pub(crate) fn flush_property_shards(
        &mut self,
        dir: &Path,
        page_size: usize,
        level: i32,
    ) -> StorageResult<u64> {
        use crate::edge::property_schema::PropertySchema;
        let mut dirty_set: HashSet<u32> = if self.properties_dirty {
            self.property_dirty_owners().into_iter().collect()
        } else {
            HashSet::new()
        };
        // Same fresh-directory completeness rule as timestamp shards:
        // a new checkpoint must carry property rows even when the
        // preceding save already cleared the dirty flags.
        for gid in self.owner_group_ids() {
            if !props_group_path(dir, gid).exists() {
                dirty_set.insert(gid);
            }
        }
        if dirty_set.is_empty() {
            return Ok(0);
        }
        let mut dirty: Vec<u32> = dirty_set.into_iter().collect();
        dirty.sort_unstable();
        if self.config.auto_encode_on_checkpoint {
            let dirty_names: Vec<String> = self.properties.dirty_column_names();
            let scope: Option<Vec<String>> = (!dirty_names.is_empty()).then_some(dirty_names);
            let adapted = self
                .properties
                .adapt_encodings_for_checkpoint(scope.as_deref(), self.config.auto_encode_min_rows);
            if adapted > 0 {
                log::debug!(
                    "flush_property_shards: adapted {} column encodings on checkpoint",
                    adapted
                );
            }
        }
        self.properties.refresh_column_stats();
        let owners = self.owner_group_ids();
        let fallback = owners.first().copied();
        let live: HashSet<u32> = owners.into_iter().collect();
        // Group dirty owners only: the single owner-map pass files each row
        // into its dirty shard slot, so grouping memory follows the dirty set
        // rather than the table size. Clean groups reuse their files untouched.
        let wanted: HashSet<u32> = dirty.iter().copied().collect();
        let mut by_owner: HashMap<u32, Vec<graphdb_core::types::EdgeId>> = HashMap::new();
        let mut fallback_hits = 0usize;
        for edge_id in self.properties.edge_ids() {
            let (gid, fell_back) =
                Self::resolve_owner_gid(&edge_id, &self.edge_owner, &live, fallback);
            fallback_hits += usize::from(fell_back);
            if wanted.contains(&gid) {
                by_owner.entry(gid).or_default().push(edge_id);
            }
        }
        if fallback_hits > 0 {
            log::debug!(
                "flush_property_shards: {} property rows fell back to smallest owner",
                fallback_hits
            );
        }
        let schema: Vec<PropertySchema> = self.properties.property_schema().to_vec();
        let dirty_columns: Vec<String> = self.properties.dirty_column_names();
        // Per-group patch scopes, snapshotted before the write loop: groups
        // with a precise trace patch only their own columns, groups without
        // one fall back to the table-wide set so no write is ever missed.
        let mut group_scopes: HashMap<u32, Vec<String>> = HashMap::new();
        for gid in &dirty {
            if let Some(scoped) = self.property_dirty_columns_for_group(*gid) {
                if !scoped.is_empty() {
                    group_scopes.insert(*gid, scoped);
                }
            }
        }
        let topology_dirty: HashSet<u32> = self
            .out_csr
            .dirty_group_ids()
            .into_iter()
            .map(|gid| gid as u32)
            .chain(
                self.in_csr
                    .dirty_group_ids()
                    .into_iter()
                    .map(|gid| gid as u32),
            )
            .collect();
        let mut written = 0u64;
        for gid in &dirty {
            let gid = *gid;
            let path = props_group_path(dir, gid);
            let edges = by_owner.get(&gid).cloned().unwrap_or_default();
            if edges.is_empty() {
                if path.exists() {
                    let _ = std::fs::remove_file(&path);
                }
                continue;
            }
            let scope: &[String] = group_scopes
                .get(&gid)
                .map(Vec::as_slice)
                .unwrap_or(&dirty_columns);
            if !topology_dirty.contains(&gid) && path.exists() && !scope.is_empty() {
                let incremental = Self::flush_property_shard_incremental(
                    &mut self.properties,
                    &path,
                    &schema,
                    &edges,
                    scope,
                )
                .unwrap_or(None);
                if let Some(bytes) = incremental {
                    super::super::persistence::write_pages_to_file(
                        &path,
                        &bytes,
                        page_size,
                        level,
                        edges.len() as u32,
                    )?;
                    written += super::layout::file_bytes(&path);
                    continue;
                }
            }
            let mut shard = crate::edge::CsrWithProperties::new(schema.clone());
            for edge_id in edges {
                if let Some((create_ts, delete_ts, values)) = self.properties.export_row(edge_id) {
                    let _ = shard.import_row(edge_id, create_ts, delete_ts, &values);
                }
            }
            for column in schema.iter().map(|s| s.name.clone()).collect::<Vec<_>>() {
                if let Some(enc) = self.properties.column_encoding_type(&column) {
                    if enc != crate::encoding::EncodingType::None {
                        let _ = shard.apply_encoding_to_column(&column, enc, 255);
                    }
                }
            }
            shard.refresh_column_stats();
            let mut payload = Vec::new();
            super::super::persistence::serialize_property_shard(
                &shard,
                crate::persistence::section::EDGE_PROPS_SHARD,
                &mut payload,
            )?;
            super::super::persistence::write_pages_to_file(
                &path,
                &payload,
                page_size,
                level,
                shard.row_count() as u32,
            )?;
            written += super::layout::file_bytes(&path);
        }
        self.properties.clear_dirty_columns();
        self.clear_property_column_dirt_for_groups(&dirty);
        Ok(written)
    }

    /// Incremental property-shard rewrite for property-only dirt.
    ///
    /// Loads the last flushed shard, patches only `dirty_columns` from the
    /// live property store, re-encodes only those columns and refreshes only
    /// their statistics. Clean columns reuse the last flushed bytes without
    /// re-export or re-encode, so checkpoint work follows dirty columns
    /// rather than the full row. Returns `Ok(None)` when the row set or
    /// schema changed, signalling the caller to take the full rewrite path
    /// rather than risking a missed write.
    fn flush_property_shard_incremental(
        live: &mut crate::edge::CsrWithProperties,
        path: &Path,
        schema: &[crate::edge::property_schema::PropertySchema],
        edges: &[graphdb_core::types::EdgeId],
        dirty_columns: &[String],
    ) -> StorageResult<Option<Vec<u8>>> {
        use std::io::Read as _;
        let Ok((raw, _)) = super::super::persistence::read_pages_from_file(path) else {
            return Ok(None);
        };
        let mut cursor = &raw[..];
        let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
        if cursor.read_exact(&mut header_buf).is_err() {
            return Ok(None);
        }
        {
            let mut slice = &header_buf[..];
            let Ok(sid) = crate::persistence::read_header(&mut slice) else {
                return Ok(None);
            };
            if sid != crate::persistence::section::EDGE_PROPS_SHARD {
                return Ok(None);
            }
        }
        let mut len_bytes = [0u8; 8];
        if cursor.read_exact(&mut len_bytes).is_err() {
            return Ok(None);
        }
        let len = u64::from_le_bytes(len_bytes) as usize;
        let mut data = vec![0u8; len];
        if cursor.read_exact(&mut data).is_err() || !cursor.is_empty() {
            return Ok(None);
        }
        let mut shard = crate::edge::CsrWithProperties::new(schema.to_vec());
        if shard.load(&data).is_err() {
            return Ok(None);
        }
        let shard_edges: HashSet<graphdb_core::types::EdgeId> = shard.edge_ids().collect();
        let live_set: HashSet<graphdb_core::types::EdgeId> = edges.iter().copied().collect();
        if shard_edges != live_set {
            return Ok(None);
        }
        let shard_cols: HashSet<String> = shard
            .property_schema()
            .iter()
            .map(|s| s.name.clone())
            .collect();
        for name in dirty_columns {
            if !shard_cols.contains(name) {
                return Ok(None);
            }
        }
        for edge_id in edges {
            let Some((_, _, values)) = live.export_row(*edge_id) else {
                return Ok(None);
            };
            let value_map: HashMap<&String, &Option<graphdb_core::Value>> =
                values.iter().map(|(name, value)| (name, value)).collect();
            for name in dirty_columns {
                let value = value_map.get(name).and_then(|cell| (*cell).clone());
                if shard
                    .set_property_for_edge(
                        *edge_id,
                        name,
                        value,
                        graphdb_core::types::MAX_TIMESTAMP,
                    )
                    .is_err()
                {
                    return Ok(None);
                }
            }
        }
        for name in dirty_columns {
            if let Some(enc) = live.column_encoding_type(name) {
                if enc != crate::encoding::EncodingType::None {
                    let _ = shard.apply_encoding_to_column(name, enc, 255);
                }
            }
            shard.refresh_column_stats_for(name);
        }
        let mut payload = Vec::new();
        super::super::persistence::serialize_property_shard(
            &shard,
            crate::persistence::section::EDGE_PROPS_SHARD,
            &mut payload,
        )?;
        Ok(Some(payload))
    }

    pub(crate) fn load_property_shards(
        &mut self,
        dir: &Path,
        manifest: &crate::edge::node_group::TableShardManifest,
    ) -> StorageResult<()> {
        use crate::edge::property_schema::PropertySchema;
        use graphdb_core::StorageError;
        use std::io::Read as _;
        let prop_schemas: Vec<PropertySchema> = self
            .schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
                    .with_default_value(p.default_value.clone())
            })
            .collect();
        // Inline forms keep no columnar rows: values live in the CSR value
        // column persisted with the group files, so the stub is restored and
        // no shard files are read or merged.
        if matches!(
            self.schema.record_form,
            crate::edge::RecordForm::Pure | crate::edge::RecordForm::Bundled
        ) {
            self.properties = crate::edge::CsrWithProperties::inline_stub(prop_schemas);
            return Ok(());
        }
        self.properties = crate::edge::CsrWithProperties::new(prop_schemas.clone());
        let owners = self.owner_list_for_load(manifest);
        let mut encodings: HashMap<String, crate::encoding::EncodingType> = HashMap::new();
        let mut prop_ids: HashMap<String, i32> = HashMap::new();
        for gid in &owners {
            let path = props_group_path(dir, *gid);
            if !path.exists() {
                continue;
            }
            let (raw, _) = super::super::persistence::read_pages_from_file(&path)?;
            let mut cursor = &raw[..];
            let mut header_buf = [0u8; crate::persistence::HEADER_SIZE];
            cursor.read_exact(&mut header_buf)?;
            {
                let mut slice = &header_buf[..];
                let sid = crate::persistence::read_header(&mut slice)?;
                if sid != crate::persistence::section::EDGE_PROPS_SHARD {
                    return Err(StorageError::deserialize_error(format!(
                        "unexpected section id in props shard: expected {:#06x}, got {:#06x}",
                        crate::persistence::section::EDGE_PROPS_SHARD,
                        sid
                    )));
                }
            }
            let mut len_bytes = [0u8; 8];
            cursor.read_exact(&mut len_bytes)?;
            let len = u64::from_le_bytes(len_bytes) as usize;
            let mut data = vec![0u8; len];
            cursor.read_exact(&mut data)?;
            if !cursor.is_empty() {
                return Err(StorageError::deserialize_error(
                    "unexpected trailing data in props shard".to_string(),
                ));
            }
            let mut shard = crate::edge::CsrWithProperties::new(prop_schemas.clone());
            shard.load(&data)?;
            for column in shard
                .property_schema()
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
            {
                if let Some(enc) = shard.column_encoding_type(&column) {
                    if enc != crate::encoding::EncodingType::None {
                        encodings.entry(column.clone()).or_insert(enc);
                    }
                }
                if let Some(id) = shard.prop_id_of(&column) {
                    prop_ids.entry(column).or_insert(id);
                }
            }
            for edge_id in shard.edge_ids().collect::<Vec<_>>() {
                if let Some((create_ts, delete_ts, values)) = shard.export_row(edge_id) {
                    // Cross-shard repeats must agree byte for byte: a repeat
                    // with different stamps or values is damage and fails the
                    // load instead of silently letting the first shard win.
                    if let Some((prev_create, prev_delete, prev_values)) =
                        self.properties.export_row(edge_id)
                    {
                        if prev_create != create_ts
                            || prev_delete != delete_ts
                            || prev_values != values
                        {
                            return Err(StorageError::deserialize_error(format!(
                                "duplicate property shard entry for edge {:?}",
                                edge_id
                            )));
                        }
                        continue;
                    }
                    self.properties
                        .import_row(edge_id, create_ts, delete_ts, &values)?;
                    self.edge_owner.or_insert(edge_id, *gid);
                }
            }
        }
        for (column, enc) in encodings {
            if self.properties.has_property(&column) {
                let _ = self.properties.apply_encoding_to_column(&column, enc, 255);
            }
        }
        self.properties.restore_prop_ids(&prop_ids);
        self.properties.refresh_column_stats();
        self.properties.clear_dirty_columns();
        Ok(())
    }
}
