use super::{CsrWithProperties, RowVisibility};
use graphdb_core::types::{EdgeId, INVALID_EDGE_ID};
use graphdb_core::{StorageError, StorageResult, Value};
use std::collections::HashSet;

impl CsrWithProperties {
    pub fn dump(&self) -> Vec<u8> {
        // Current-value snapshot plus per-column identity, encoding choice
        // and statistics. Row version chains stay memory-only by design:
        // load restores plain values holding the latest value with the row
        // creation stamp, then re-applies the recorded encodings.
        let mut buf = Vec::new();
        buf.extend_from_slice(&(self.visibility.len() as u32).to_le_bytes());
        for vis in &self.visibility {
            buf.extend_from_slice(&vis.create_ts.to_le_bytes());
            if let Some(del) = vis.delete_ts {
                buf.push(1);
                buf.extend_from_slice(&del.to_le_bytes());
            } else {
                buf.push(0);
            }
        }
        buf.extend_from_slice(&(self.row_count as u32).to_le_bytes());
        buf.extend_from_slice(&(self.edge_map_len as u32).to_le_bytes());
        for (edge_id, pos) in self.edge_mappings() {
            buf.extend_from_slice(&(edge_id.0).to_le_bytes());
            buf.extend_from_slice(&pos.to_le_bytes());
        }
        buf.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            buf.extend_from_slice(&off.to_le_bytes());
        }
        // Serialize current column values (without version history) keyed by
        // column name, each carrying its stable identifier, its encoding
        // choice and its last refreshed statistics.
        buf.extend_from_slice(&(self.property_columns.len() as u32).to_le_bytes());
        for (idx, col) in self.property_columns.iter().enumerate() {
            // Column name keys the payload to the schema entry on load.
            buf.extend_from_slice(&(col.name.len() as u32).to_le_bytes());
            buf.extend_from_slice(col.name.as_bytes());
            let prop_id = self
                .property_schema
                .get(idx)
                .map(|schema| schema.prop_id)
                .unwrap_or(-1);
            buf.extend_from_slice(&prop_id.to_le_bytes());
            buf.push(col.encoding_type().to_u8());
            let rows = self.visibility.len();
            buf.extend_from_slice(&(rows as u32).to_le_bytes());
            buf.reserve(rows.saturating_mul(16));
            let mut cell_scratch: Vec<u8> = Vec::with_capacity(64);
            for row_idx in 0..rows {
                let val = col.get(row_idx);
                if let Some(v) = val {
                    buf.push(1);
                    cell_scratch.clear();
                    let taken = std::mem::take(&mut cell_scratch);
                    match postcard::to_extend(&v, taken) {
                        Ok(encoded) => {
                            buf.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
                            buf.extend_from_slice(&encoded);
                            cell_scratch = encoded;
                        }
                        Err(_) => {
                            buf.extend_from_slice(&0u32.to_le_bytes());
                            cell_scratch = Vec::with_capacity(64);
                        }
                    }
                } else {
                    buf.push(0);
                }
            }
            match col.stats() {
                Some(stats) => {
                    let mut stats_buf = Vec::new();
                    if stats.serialize_meta(&mut stats_buf).is_ok() {
                        buf.push(1);
                        buf.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                        buf.extend_from_slice(&stats_buf);
                    } else {
                        buf.push(0);
                    }
                }
                None => buf.push(0),
            }
        }
        buf
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        fn need(data: &[u8], offset: usize, len: usize, what: &str) -> StorageResult<()> {
            if data.len().saturating_sub(offset) < len {
                return Err(StorageError::deserialize_error(format!(
                    "properties payload too short for {}",
                    what
                )));
            }
            Ok(())
        }
        if data.is_empty() {
            return Err(StorageError::deserialize_error(
                "properties payload is empty",
            ));
        }
        let mut offset = 0usize;
        need(data, offset, 4, "visibility length")?;
        let vis_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.visibility.clear();
        self.visibility.reserve(vis_len);
        for _ in 0..vis_len {
            need(data, offset, 8, "row creation stamp")?;
            let create = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            need(data, offset, 1, "row deletion flag")?;
            let has_del = data[offset];
            offset += 1;
            let del = if has_del == 1 {
                need(data, offset, 8, "row deletion stamp")?;
                let d = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                Some(d)
            } else if has_del == 0 {
                None
            } else {
                return Err(StorageError::deserialize_error(format!(
                    "invalid row deletion flag: {}",
                    has_del
                )));
            };
            self.visibility.push(RowVisibility {
                create_ts: create,
                delete_ts: del,
            });
        }
        need(data, offset, 4, "row count")?;
        self.row_count = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        need(data, offset, 4, "edge map length")?;
        let map_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.edge_map_segments.clear();
        self.edge_map_len = 0;
        // Wire format is unchanged (entry count plus id/row pairs); the sparse
        // map is rebuilt from the pairs. Rows must land inside the restored
        // visibility window, otherwise the payload is rejected.
        let vis_len = self.visibility.len();
        for _ in 0..map_len {
            need(data, offset, 12, "edge map entry")?;
            let eid = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            offset += 8;
            let pos = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            if eid == INVALID_EDGE_ID.0 || (pos as usize) >= vis_len {
                return Err(StorageError::deserialize_error(
                    "edge map entry outside the restored row window",
                ));
            }
            if self.mapped_row(EdgeId(eid)).is_some() {
                return Err(StorageError::deserialize_error(
                    "duplicate edge map entry in properties payload",
                ));
            }
            self.map_insert(EdgeId(eid), pos as usize)?;
        }
        need(data, offset, 4, "free list length")?;
        let free_len = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        self.free_list.clear();
        for _ in 0..free_len {
            need(data, offset, 4, "free list entry")?;
            let off = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
            offset += 4;
            self.free_list.push(off);
        }
        need(data, offset, 4, "column count")?;
        {
            let col_count =
                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            let mut seen_names = HashSet::new();
            let mut seen_ids = HashSet::new();
            for _ in 0..col_count {
                need(data, offset, 4, "column name length")?;
                let name_len =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                need(data, offset, name_len, "column name")?;
                let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                offset += name_len;
                if !seen_names.insert(name.clone()) {
                    return Err(StorageError::deserialize_error(format!(
                        "duplicate column in properties payload: {}",
                        name
                    )));
                }
                need(data, offset, 4, "column identifier")?;
                let prop_id = i32::from_le_bytes(data[offset..offset + 4].try_into().unwrap());
                offset += 4;
                if !seen_ids.insert(prop_id) {
                    return Err(StorageError::deserialize_error(format!(
                        "duplicate column identifier in properties payload: {}",
                        prop_id
                    )));
                }
                need(data, offset, 1, "column encoding")?;
                let encoding_tag = data[offset];
                offset += 1;
                if encoding_tag > 6 {
                    return Err(StorageError::deserialize_error(format!(
                        "unknown column encoding tag: {}",
                        encoding_tag
                    )));
                }
                let encoding = crate::encoding::EncodingType::from_u8(encoding_tag);
                need(data, offset, 4, "column row count")?;
                let rows =
                    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
                offset += 4;
                if let Some(&col_idx) = self.column_index.get(&name) {
                    self.property_schema[col_idx].prop_id = prop_id;
                    let col = &mut self.property_columns[col_idx];
                    col.col_id = prop_id;
                    if col.len() < rows {
                        col.resize(rows);
                    }
                    for row_idx in 0..rows {
                        need(data, offset, 1, "column cell flag")?;
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            need(data, offset, 4, "column cell length")?;
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            need(data, offset, vlen, "column cell value")?;
                            let vbytes = &data[offset..offset + vlen];
                            offset += vlen;
                            if let Ok(val) = postcard::from_bytes::<Value>(vbytes) {
                                let _ = col.set(row_idx, Some(&val));
                            }
                        } else if has == 0 {
                            let _ = col.set(row_idx, None);
                        } else {
                            return Err(StorageError::deserialize_error(format!(
                                "invalid column cell flag: {}",
                                has
                            )));
                        }
                    }
                    need(data, offset, 1, "column stats flag")?;
                    let has_stats = data[offset];
                    offset += 1;
                    if has_stats == 1 {
                        need(data, offset, 4, "column stats length")?;
                        let stats_len =
                            u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                as usize;
                        offset += 4;
                        need(data, offset, stats_len, "column stats")?;
                        let stats_bytes = &data[offset..offset + stats_len];
                        offset += stats_len;
                        let mut cursor = stats_bytes;
                        let stats =
                            crate::column_stats::ColumnStats::deserialize_meta(&mut cursor)?;
                        if !cursor.is_empty() {
                            return Err(StorageError::deserialize_error(
                                "unexpected trailing data in column stats".to_string(),
                            ));
                        }
                        if encoding != crate::encoding::EncodingType::None {
                            self.apply_encoding_to_column(&name, encoding, 255)?;
                        }
                        let col = &mut self.property_columns[col_idx];
                        col.set_stats(stats);
                    } else if has_stats == 0 {
                        if encoding != crate::encoding::EncodingType::None {
                            self.apply_encoding_to_column(&name, encoding, 255)?;
                        }
                    } else {
                        return Err(StorageError::deserialize_error(format!(
                            "invalid column stats flag: {}",
                            has_stats
                        )));
                    }
                } else {
                    for _ in 0..rows {
                        need(data, offset, 1, "unknown column cell flag")?;
                        let has = data[offset];
                        offset += 1;
                        if has == 1 {
                            need(data, offset, 4, "unknown column cell length")?;
                            let vlen =
                                u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                    as usize;
                            offset += 4;
                            need(data, offset, vlen, "unknown column cell value")?;
                            offset += vlen;
                        } else if has != 0 {
                            return Err(StorageError::deserialize_error(format!(
                                "invalid unknown column cell flag: {}",
                                has
                            )));
                        }
                    }
                    // Unpublished columns keep the abort-on-reload contract:
                    // skip their cells, encoding tag and statistics alike.
                    need(data, offset, 1, "unknown column stats flag")?;
                    let unknown_stats = data[offset];
                    offset += 1;
                    if unknown_stats == 1 {
                        need(data, offset, 4, "unknown column stats length")?;
                        let stats_len =
                            u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
                                as usize;
                        offset += 4;
                        need(data, offset, stats_len, "unknown column stats")?;
                        offset += stats_len;
                    } else if unknown_stats != 0 {
                        return Err(StorageError::deserialize_error(format!(
                            "invalid unknown column stats flag: {}",
                            unknown_stats
                        )));
                    }
                }
            }
        }
        if offset != data.len() {
            return Err(StorageError::deserialize_error(
                "unexpected trailing data in properties payload".to_string(),
            ));
        }
        self.rebuild_aux_indexes();
        self.dirty_columns.clear();
        Ok(())
    }

    fn rebuild_aux_indexes(&mut self) {
        self.row_to_edge.clear();
        self.row_to_edge.resize(self.visibility.len(), None);
        let mappings: Vec<(EdgeId, u32)> = self.edge_mappings().collect();
        for (edge_id, pos) in mappings {
            let idx = pos as usize;
            if idx < self.row_to_edge.len() {
                self.row_to_edge[idx] = Some(edge_id);
            }
        }
        self.rebuild_schema_indexes();
        let max_id = self
            .property_schema
            .iter()
            .map(|s| s.prop_id)
            .max()
            .unwrap_or(-1);
        self.next_prop_id = max_id
            .saturating_add(1)
            .max(self.property_schema.len() as i32);
    }
}
