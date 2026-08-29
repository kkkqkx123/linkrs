//! Serialization: on-disk dump/load (column-only).

use super::*;

impl PropertyTable {
    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();
        write_header(&mut result, section::PROPERTY_TABLE);
        let checksum_pos = result.len();
        result.extend_from_slice(&[0u8; 4]);
        result.push(PROPERTY_TABLE_VERSION);
        result.extend_from_slice(&(self.schema.len() as u32).to_le_bytes());
        for prop in &self.schema {
            let name_bytes = prop.name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            result.extend_from_slice(&prop.prop_id.to_le_bytes());
            result.push(prop.data_type.as_u8());
            if prop.data_type.as_u8() >= 64
                || matches!(
                    prop.data_type,
                    DataType::List(_) | DataType::Map(_) | DataType::Set(_)
                )
            {
                let info = match &prop.data_type {
                    DataType::List(e) => TypeInfo::List(e.clone()),
                    DataType::Map(v) => TypeInfo::Map(v.clone()),
                    DataType::Set(e) => TypeInfo::Set(e.clone()),
                    DataType::Struct(s) => TypeInfo::Struct(s.as_ref().clone()),
                    DataType::Array(a) => TypeInfo::Array(a.as_ref().clone()),
                    _ => unreachable!("parameterized data type without TypeInfo"),
                };
                let bytes = postcard::to_allocvec(&info)
                    .expect("TypeInfo encoding cannot fail for schema-valid input");
                result.extend_from_slice(&bytes);
            }
            result.push(if prop.nullable { 1 } else { 0 });
            result.push(prop.encoding_type.to_u8());
        }

        // Row metadata: create_ts, delete_ts, row_count, free_list
        result.extend_from_slice(&(self.row_create_ts.len() as u32).to_le_bytes());
        for &ts in &self.row_create_ts {
            result.extend_from_slice(&ts.to_le_bytes());
        }
        for opt in &self.row_delete_ts {
            match opt {
                Some(ts) => {
                    result.push(1);
                    result.extend_from_slice(&ts.to_le_bytes());
                }
                None => result.push(0),
            }
        }
        result.extend_from_slice(&(self.row_count as u32).to_le_bytes());
        result.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            encode_varint(off, &mut result);
        }

        // Column store: per column raw data + MVCC chains
        result.extend_from_slice(&(self.column_store.column_count() as u32).to_le_bytes());
        for col in self.column_store.columns() {
            let name_bytes = col.name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            // Column data
            let (data, offsets, bitmap) = col.get_flush_data();
            result.extend_from_slice(&(data.len() as u32).to_le_bytes());
            result.extend_from_slice(&data);
            result.extend_from_slice(&(offsets.len() as u32).to_le_bytes());
            for &off in &offsets {
                result.extend_from_slice(&off.to_le_bytes());
            }
            if let Some(bitmap) = bitmap {
                result.push(1);
                let bitmap_bytes = bitmap.as_raw_slice();
                let bitmap_bit_len = bitmap.len() as u32;
                result.extend_from_slice(&bitmap_bit_len.to_le_bytes());
                result.extend_from_slice(&(bitmap_bytes.len() as u32).to_le_bytes());
                result.extend_from_slice(bitmap_bytes);
            } else {
                result.push(0);
            }
            // Encoding
            result.push(col.encoding_type().to_u8());
            if col.encoding_type() != EncodingType::None {
                let mut meta_buf = Vec::new();
                let _ = col.encoding().serialize_meta(&mut meta_buf);
                result.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
                result.extend_from_slice(&meta_buf);
            }
            // MVCC: row_start_ts
            let row_start = col.row_start_ts_vec();
            result.extend_from_slice(&(row_start.len() as u32).to_le_bytes());
            for &ts in row_start {
                result.extend_from_slice(&ts.to_le_bytes());
            }
            // MVCC: version chains per row
            let chains = col.version_chains_ref();
            result.extend_from_slice(&(chains.len() as u32).to_le_bytes());
            for chain in chains {
                result.extend_from_slice(&(chain.len() as u32).to_le_bytes());
                for entry in chain {
                    result.extend_from_slice(&entry.start_ts.to_le_bytes());
                    result.extend_from_slice(&entry.end_ts.to_le_bytes());
                    match &entry.value {
                        None => result.push(0),
                        Some(v) => {
                            result.push(1);
                            let v_bytes = postcard::to_allocvec(v).unwrap_or_default();
                            result.extend_from_slice(&(v_bytes.len() as u32).to_le_bytes());
                            result.extend_from_slice(&v_bytes);
                        }
                    }
                }
            }
            // Stats
            if let Some(stats) = col.stats() {
                result.push(1);
                let mut stats_buf = Vec::new();
                let _ = stats.serialize_meta(&mut stats_buf);
                result.extend_from_slice(&(stats_buf.len() as u32).to_le_bytes());
                result.extend_from_slice(&stats_buf);
            } else {
                result.push(0);
            }
        }

        // Zone maps (table-level)
        result.extend_from_slice(&(self.zone_maps.len() as u32).to_le_bytes());
        for (col_name, chunks) in &self.zone_maps {
            let name_bytes = col_name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            result.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
            for stats in chunks {
                let mut meta_buf = Vec::new();
                let _ = stats.serialize_meta(&mut meta_buf);
                result.extend_from_slice(&(meta_buf.len() as u32).to_le_bytes());
                result.extend_from_slice(&meta_buf);
            }
        }

        let checksum = crc32fast::hash(&result[checksum_pos + 4..]);
        result[checksum_pos..checksum_pos + 4].copy_from_slice(&checksum.to_le_bytes());
        result
    }

    pub fn load(&mut self, data: &[u8]) -> StorageResult<()> {
        if data.is_empty() {
            return Ok(());
        }
        let mut cursor = data;
        let (_version, section_id) = read_header(&mut cursor)?;
        if section_id != section::PROPERTY_TABLE {
            return Err(StorageError::deserialize_error(format!(
                "invalid section_id for PropertyTable: expected 0x{:04X}, got 0x{:04X}",
                section::PROPERTY_TABLE,
                section_id
            )));
        }
        if cursor.len() < 4 {
            return Err(StorageError::deserialize_error(
                "PropertyTable data too short for checksum",
            ));
        }
        let stored_checksum = u32::from_le_bytes(cursor[..4].try_into().map_err(|_| {
            StorageError::deserialize_error("failed to read PropertyTable checksum")
        })?);
        let payload = &cursor[4..];
        let computed_checksum = crc32fast::hash(payload);
        if stored_checksum != computed_checksum {
            return Err(StorageError::deserialize_error(format!(
                "PropertyTable checksum mismatch: stored {:#x}, computed {:#x}",
                stored_checksum, computed_checksum
            )));
        }
        let data = payload;
        let mut offset = 0usize;
        let version = data.get(offset).copied().ok_or_else(|| {
            StorageError::deserialize_error("PropertyTable data missing version byte")
        })?;
        offset += 1;
        if version != PROPERTY_TABLE_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "Unsupported PropertyTable version: expected {PROPERTY_TABLE_VERSION}, got {version}"
            )));
        }
        let schema_len = read_u32_le(data, &mut offset)? as usize;
        self.schema.clear();
        self.name_indexer.clear();
        for _ in 0..schema_len {
            let name_len = read_u32_le(data, &mut offset)? as usize;
            if offset + name_len > data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
            offset += name_len;
            let prop_id_bytes: [u8; 4] = data[offset..offset + 4]
                .try_into()
                .map_err(|_| StorageError::deserialize_error("failed to read prop_id"))?;
            let prop_id = i32::from_le_bytes(prop_id_bytes);
            offset += 4;
            let data_type = match DataType::from_u8(data[offset]) {
                Ok(dt) => {
                    offset += 1;
                    dt
                }
                Err(TypeCodecError::ParameterizedTypeCode(code)) => {
                    let (info, rest) =
                        postcard::take_from_bytes(&data[offset + 1..]).map_err(|e| {
                            StorageError::deserialize_error(format!(
                                "failed to decode TypeInfo for code {code}: {e}"
                            ))
                        })?;
                    let consumed = (data.len() - rest.len()) - (offset + 1);
                    offset += 1 + consumed;
                    data_type_from_info(code, &info).ok_or_else(|| {
                        StorageError::deserialize_error(format!(
                            "TypeInfo mismatch for parameterized code {code}"
                        ))
                    })?
                }
                Err(e) => {
                    return Err(StorageError::deserialize_error(format!(
                        "failed to decode data type: {}",
                        e
                    )))
                }
            };
            if offset + 2 > data.len() {
                return Err(StorageError::deserialize_error(
                    "unexpected end of data after parameterized type block",
                ));
            }
            let nullable = data[offset] == 1;
            offset += 1;
            let encoding_type = EncodingType::from_u8(data[offset]);
            offset += 1;
            let prop_schema = PropertySchema::new(name.clone(), prop_id, data_type)
                .nullable(nullable)
                .with_encoding(encoding_type);
            self.name_indexer.register(name.clone())?;
            self.schema.push(prop_schema);
        }

        // Row metadata
        let row_len = read_u32_le(data, &mut offset)? as usize;
        self.row_create_ts.clear();
        self.row_create_ts.reserve(row_len);
        for _ in 0..row_len {
            let ts = read_u64_le(data, &mut offset)?;
            self.row_create_ts.push(ts);
        }
        self.row_delete_ts.clear();
        self.row_delete_ts.reserve(row_len);
        for _ in 0..row_len {
            if offset >= data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let has = data[offset];
            offset += 1;
            if has == 1 {
                let ts = read_u64_le(data, &mut offset)?;
                self.row_delete_ts.push(Some(ts));
            } else {
                self.row_delete_ts.push(None);
            }
        }
        self.row_count = read_u32_le(data, &mut offset)? as usize;
        let free_list_len = read_u32_le(data, &mut offset)? as usize;
        self.free_list.clear();
        for _ in 0..free_list_len {
            let mut cur = Cursor::new(&data[offset..]);
            let off = decode_varint(&mut cur)?;
            offset += cur.position() as usize;
            self.free_list.push(off);
        }

        // Column store
        let col_count = read_u32_le(data, &mut offset)? as usize;
        self.column_store = ColumnStore::new();
        for _ in 0..col_count {
            let name_len = read_u32_le(data, &mut offset)? as usize;
            if offset + name_len > data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
            offset += name_len;
            let data_len = read_u32_le(data, &mut offset)? as usize;
            if offset + data_len > data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let col_data = data[offset..offset + data_len].to_vec();
            offset += data_len;
            let offsets_cnt = read_u32_le(data, &mut offset)? as usize;
            let mut offsets = Vec::with_capacity(offsets_cnt);
            for _ in 0..offsets_cnt {
                let v = u64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
                offset += 8;
                offsets.push(v);
            }
            let has_bitmap = data[offset];
            offset += 1;
            let (bitmap_raw, bitmap_len) = if has_bitmap == 1 {
                let bit_len = read_u32_le(data, &mut offset)? as usize;
                let bytes_len = read_u32_le(data, &mut offset)? as usize;
                if offset + bytes_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let bytes = data[offset..offset + bytes_len].to_vec();
                offset += bytes_len;
                (Some(bytes), bit_len)
            } else {
                (None, 0)
            };
            let enc_type = EncodingType::from_u8(data[offset]);
            offset += 1;
            let enc_meta = if enc_type != EncodingType::None {
                let meta_len = read_u32_le(data, &mut offset)? as usize;
                if offset + meta_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let meta = data[offset..offset + meta_len].to_vec();
                offset += meta_len;
                Some((enc_type, meta))
            } else {
                None
            };
            // Find schema for this column to create it
            let schema_entry = self.schema.iter().find(|s| s.name == name).ok_or_else(|| {
                StorageError::deserialize_error(format!("column {} not in schema", name))
            })?;
            self.column_store.add_column(
                name.clone(),
                schema_entry.data_type.clone(),
                schema_entry.nullable,
            );
            // Load raw data
            self.column_store
                .load_column_from_raw(&name, col_data, offsets, bitmap_raw, bitmap_len)?;
            // Apply encoding if any
            if let Some((enc_t, meta_bytes)) = enc_meta {
                let mut cur = if !meta_bytes.is_empty() && meta_bytes[0] == enc_t.to_u8() {
                    &meta_bytes[1..]
                } else {
                    &meta_bytes[..]
                };
                let col = self.column_store.get_column_mut(&name).unwrap();
                match enc_t {
                    EncodingType::Fsst => {
                        let c = crate::encoding::FsstColumn::deserialize_meta(&mut cur)?;
                        col.apply_fsst_from_meta(c)?;
                    }
                    EncodingType::Dictionary => {
                        let c = crate::encoding::DictionaryColumn::deserialize_meta(&mut cur)?;
                        col.apply_dictionary_from_meta(c)?;
                    }
                    EncodingType::Rle => {
                        // Need to distinguish Bool vs Int
                        let col_dt = col.data_type.clone();
                        if col_dt == DataType::Bool {
                            let c = crate::encoding::RleBoolColumn::deserialize_meta(&mut cur)?;
                            col.apply_rle_bool_from_meta(c)?;
                        } else {
                            let c = crate::encoding::RleIntColumn::deserialize_meta(&mut cur)?;
                            col.apply_rle_int_from_meta(c)?;
                        }
                    }
                    EncodingType::BitPacking => {
                        let c = crate::encoding::BitPackedIntColumn::deserialize_meta(&mut cur)?;
                        col.apply_bitpacked_from_meta(c)?;
                    }
                    EncodingType::Alp => {
                        let c = crate::encoding::AlpColumn::deserialize_meta(&mut cur)?;
                        col.apply_alp_from_meta(c)?;
                    }
                    EncodingType::Constant => {
                        let c = crate::encoding::ConstantColumn::deserialize_meta(&mut cur)?;
                        col.apply_constant_from_meta(c)?;
                    }
                    EncodingType::None => {}
                }
            }
            // MVCC row_start
            let rs_len = read_u32_le(data, &mut offset)? as usize;
            let mut row_start = Vec::with_capacity(rs_len);
            for _ in 0..rs_len {
                row_start.push(read_u64_le(data, &mut offset)?);
            }
            // Version chains
            let chains_len = read_u32_le(data, &mut offset)? as usize;
            let mut chains: Vec<Vec<crate::vertex::column_store::VersionEntry>> =
                Vec::with_capacity(chains_len);
            for _ in 0..chains_len {
                let chain_n = read_u32_le(data, &mut offset)? as usize;
                let mut chain = Vec::with_capacity(chain_n);
                for _ in 0..chain_n {
                    let start_ts = read_u64_le(data, &mut offset)?;
                    let end_ts = read_u64_le(data, &mut offset)?;
                    let has_val = data[offset];
                    offset += 1;
                    let value = if has_val == 1 {
                        let v_len = read_u32_le(data, &mut offset)? as usize;
                        if offset + v_len > data.len() {
                            return Err(StorageError::deserialize_error("unexpected end of data"));
                        }
                        let v_bytes = &data[offset..offset + v_len];
                        offset += v_len;
                        let v: Value = postcard::from_bytes(v_bytes).map_err(|e| {
                            StorageError::deserialize_error(format!("Value decode error: {e}"))
                        })?;
                        Some(v)
                    } else {
                        None
                    };
                    chain.push(crate::vertex::column_store::VersionEntry {
                        start_ts,
                        end_ts,
                        value,
                    });
                }
                chains.push(chain);
            }
            if let Some(col) = self.column_store.get_column_mut(&name) {
                col.set_row_start_ts(row_start);
                col.set_version_chains(chains);
            }
            // Stats
            let has_stats = data[offset];
            offset += 1;
            if has_stats == 1 {
                let stats_len = read_u32_le(data, &mut offset)? as usize;
                if offset + stats_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let mut cur = &data[offset..offset + stats_len];
                let stats = crate::column_stats::ColumnStats::deserialize_meta(&mut cur)?;
                offset += stats_len;
                if let Some(col) = self.column_store.get_column_mut(&name) {
                    col.set_stats(stats);
                }
            }
        }

        // Zone maps
        self.zone_maps.clear();
        let zm_len = read_u32_le(data, &mut offset)? as usize;
        for _ in 0..zm_len {
            let name_len = read_u32_le(data, &mut offset)? as usize;
            if offset + name_len > data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let name = String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
            offset += name_len;
            let chunk_count = read_u32_le(data, &mut offset)? as usize;
            let mut chunks = Vec::with_capacity(chunk_count);
            for _ in 0..chunk_count {
                let meta_len = read_u32_le(data, &mut offset)? as usize;
                if offset + meta_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let mut cur = &data[offset..offset + meta_len];
                if let Ok(stats) = ColumnStats::deserialize_meta(&mut cur) {
                    chunks.push(stats);
                }
                offset += meta_len;
            }
            self.zone_maps.insert(name, chunks);
        }
        if self.zone_maps.is_empty() && !self.row_create_ts.is_empty() {
            self.rebuild_zone_maps();
        }

        // Rebuild tombstones
        self.tombstones_manager = TieredTombstoneManager::new(10_000);
        for (idx, del_opt) in self.row_delete_ts.iter().enumerate() {
            if let Some(del_ts) = del_opt {
                let off = prop_index_to_offset(idx);
                self.tombstones_manager.add_tombstone(off, *del_ts);
            }
        }
        // Rebuild value index from column store
        self.value_index.rebuild_columnar(
            &self.schema,
            &self.column_store,
            &self.row_create_ts,
            &self.row_delete_ts,
            &self.free_list,
        );

        Ok(())
    }
}
