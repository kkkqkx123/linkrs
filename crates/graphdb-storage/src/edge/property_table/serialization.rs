//! Serialization: on-disk dump/load and row (de)serialization.

use super::*;

impl PropertyTable {
    pub(super) fn serialize_row(&self, values: &[(String, Value)]) -> StorageResult<Vec<u8>> {
        let mut buffer = Vec::new();

        for schema in &self.schema {
            let value = values
                .iter()
                .find(|(k, _)| k == &schema.name)
                .map(|(_, v)| v.clone());

            self.serialize_value(&mut buffer, value.as_ref(), schema)?;
        }

        Ok(buffer)
    }

    pub(super) fn serialize_row_with_nulls(
        &self,
        values: &[(String, Option<Value>)],
    ) -> StorageResult<Vec<u8>> {
        let mut buffer = Vec::new();

        for schema in &self.schema {
            let value = values
                .iter()
                .find(|(k, _)| k == &schema.name)
                .and_then(|(_, v)| v.clone());

            self.serialize_value(&mut buffer, value.as_ref(), schema)?;
        }

        Ok(buffer)
    }

    fn serialize_value(
        &self,
        buffer: &mut Vec<u8>,
        value: Option<&Value>,
        schema: &PropertySchema,
    ) -> StorageResult<()> {
        match value {
            None => {
                buffer.push(0); // null marker
            }
            Some(val) => {
                buffer.push(1); // not null marker
                match &schema.data_type {
                    DataType::Bool => {
                        if let Value::Bool(b) = val {
                            buffer.push(if *b { 1 } else { 0 });
                        }
                    }
                    DataType::SmallInt => {
                        if let Value::SmallInt(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Int => {
                        if let Value::Int(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::BigInt => {
                        if let Value::BigInt(i) = val {
                            buffer.extend_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Float => {
                        if let Value::Float(f) = val {
                            buffer.extend_from_slice(&f.to_le_bytes());
                        }
                    }
                    DataType::Double => {
                        if let Value::Double(d) = val {
                            buffer.extend_from_slice(&d.to_le_bytes());
                        }
                    }
                    DataType::String => {
                        if let Value::String(s) = val {
                            let s_bytes = s.as_bytes();
                            encode_varint(s_bytes.len() as u32, buffer);
                            buffer.extend_from_slice(s_bytes);
                        }
                    }
                    DataType::Date => {
                        if let Value::Date(d) = val {
                            buffer.extend_from_slice(&d.year.to_le_bytes());
                            buffer.extend_from_slice(&d.month.to_le_bytes());
                            buffer.extend_from_slice(&d.day.to_le_bytes());
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    pub(super) fn deserialize_row(
        &self,
        record: &[u8],
    ) -> StorageResult<Vec<(String, Option<Value>)>> {
        let mut cursor = Cursor::new(record);
        let mut result = Vec::new();

        for schema in &self.schema {
            let mut null_marker = [0u8; 1];
            if cursor.read_exact(&mut null_marker).is_err() {
                result.push((schema.name.clone(), None));
                continue;
            }

            if null_marker[0] == 0 {
                result.push((schema.name.clone(), None));
            } else {
                let value = self.deserialize_value(&mut cursor, &schema.data_type)?;
                result.push((schema.name.clone(), value));
            }
        }

        Ok(result)
    }

    fn deserialize_value(
        &self,
        cursor: &mut Cursor<&[u8]>,
        data_type: &DataType,
    ) -> StorageResult<Option<Value>> {
        match data_type {
            DataType::Bool => {
                let mut b = [0u8; 1];
                cursor.read_exact(&mut b)?;
                Ok(Some(Value::Bool(b[0] != 0)))
            }
            DataType::SmallInt => {
                let mut buf = [0u8; 2];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::SmallInt(i16::from_le_bytes(buf))))
            }
            DataType::Int => {
                let mut buf = [0u8; 4];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Int(i32::from_le_bytes(buf))))
            }
            DataType::BigInt => {
                let mut buf = [0u8; 8];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::BigInt(i64::from_le_bytes(buf))))
            }
            DataType::Float => {
                let mut buf = [0u8; 4];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Float(f32::from_le_bytes(buf))))
            }
            DataType::Double => {
                let mut buf = [0u8; 8];
                cursor.read_exact(&mut buf)?;
                Ok(Some(Value::Double(f64::from_le_bytes(buf))))
            }
            DataType::String => {
                let len = decode_varint(cursor)? as usize;
                let mut str_buf = vec![0u8; len];
                cursor.read_exact(&mut str_buf)?;
                Ok(Some(Value::string(String::from_utf8_lossy(&str_buf))))
            }
            DataType::Date => {
                let mut buf = [0u8; 10];
                cursor.read_exact(&mut buf[..4])?;
                let year = i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                cursor.read_exact(&mut buf[..4])?;
                let month = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                cursor.read_exact(&mut buf[..4])?;
                let day = u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]);
                Ok(Some(Value::Date(DateValue { year, month, day })))
            }
            _ => Ok(None),
        }
    }

    /// Serialize a single value into a byte buffer at a given offset.
    /// Used for direct byte manipulation in set_property.
    pub(super) fn serialize_value_at_offset(
        &self,
        buffer: &mut [u8],
        value: Option<&Value>,
        col_idx: usize,
    ) -> StorageResult<()> {
        let byte_off = self
            .column_byte_offsets
            .get(col_idx)
            .ok_or_else(|| StorageError::column_not_found(format!("col_idx={}", col_idx)))?;

        let dt = &self.schema[col_idx].data_type;
        let val_size = Self::data_type_byte_size(dt).ok_or_else(|| {
            StorageError::not_supported(
                "Variable-size types not supported for direct update".to_string(),
            )
        })?;

        match value {
            None => {
                buffer[*byte_off] = 0; // null marker
                                       // Zero out value bytes (safety, but not strictly required)
                for i in 0..val_size {
                    buffer[*byte_off + 1 + i] = 0;
                }
            }
            Some(val) => {
                buffer[*byte_off] = 1; // not null marker
                let target = &mut buffer[*byte_off + 1..*byte_off + 1 + val_size];
                match dt {
                    DataType::Bool => {
                        if let Value::Bool(b) = val {
                            target[0] = if *b { 1 } else { 0 };
                        }
                    }
                    DataType::SmallInt => {
                        if let Value::SmallInt(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Int => {
                        if let Value::Int(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::BigInt => {
                        if let Value::BigInt(i) = val {
                            target.copy_from_slice(&i.to_le_bytes());
                        }
                    }
                    DataType::Float => {
                        if let Value::Float(f) = val {
                            target.copy_from_slice(&f.to_le_bytes());
                        }
                    }
                    DataType::Double => {
                        if let Value::Double(d) = val {
                            target.copy_from_slice(&d.to_le_bytes());
                        }
                    }
                    _ => {
                        return Err(StorageError::not_supported(format!(
                            "Unexpected fixed-size type: {:?}",
                            dt
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn dump(&self) -> Vec<u8> {
        let mut result = Vec::new();

        write_header(&mut result, section::PROPERTY_TABLE);

        let checksum_pos = result.len();
        result.extend_from_slice(&[0u8; 4]);

        // Version marker (development uses a single on-disk layout).
        result.push(PROPERTY_TABLE_VERSION);

        result.extend_from_slice(&(self.schema.len() as u32).to_le_bytes());
        for prop in &self.schema {
            let name_bytes = prop.name.as_bytes();
            result.extend_from_slice(&(name_bytes.len() as u32).to_le_bytes());
            result.extend_from_slice(name_bytes);
            result.extend_from_slice(&prop.prop_id.to_le_bytes());
            result.push(prop.data_type.as_u8());
            // Parameterized types (List/Map/Set/Struct/Array) carry a
            // postcard-encoded TypeInfo block right after the code byte. Plain
            // codes have no block, keeping the old format byte-compatible for
            // scalar types.
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
                // Infallible for schema-valid input: only an allocation
                // overflow could error, which would abort the process anyway.
                let bytes = postcard::to_allocvec(&info)
                    .expect("TypeInfo encoding cannot fail for schema-valid input");
                result.extend_from_slice(&bytes);
            }
            result.push(if prop.nullable { 1 } else { 0 });
            result.push(prop.encoding_type.to_u8());
        }

        result.extend_from_slice(&(self.records.len() as u32).to_le_bytes());

        // Store each PropertyRecord with timestamps
        for record_opt in &self.records {
            match record_opt {
                Some(record) => {
                    result.push(1); // marker: has data
                    result.extend_from_slice(&record.create_ts.to_le_bytes());
                    if let Some(del_ts) = record.delete_ts {
                        result.push(1); // marker: has delete_ts
                        result.extend_from_slice(&del_ts.to_le_bytes());
                    } else {
                        result.push(0); // marker: no delete_ts
                    }
                    result.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
                    result.extend_from_slice(&record.data);
                }
                None => {
                    result.push(0); // marker: deleted
                }
            }
        }

        // Store per-row before-image version chains (oldest first), matching
        // the record encoding: marker / create_ts / delete_ts marker / data.
        for chain in &self.chain_records {
            result.extend_from_slice(&(chain.len() as u32).to_le_bytes());
            for record in chain {
                result.extend_from_slice(&record.create_ts.to_le_bytes());
                if let Some(del_ts) = record.delete_ts {
                    result.push(1); // marker: has delete_ts
                    result.extend_from_slice(&del_ts.to_le_bytes());
                } else {
                    result.push(0); // marker: no delete_ts
                }
                result.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
                result.extend_from_slice(&record.data);
            }
        }

        // Store free list with Varint encoding
        result.extend_from_slice(&(self.free_list.len() as u32).to_le_bytes());
        for &off in &self.free_list {
            encode_varint(off, &mut result);
        }

        // ── zone maps (v4) ──
        // Persist per-column zone maps (chunk stats) for predicate pruning.
        // Columnar data itself is rebuilt from row records on load (dual-write
        // in-memory column store); persisting it separately would duplicate the
        // payload. Zone maps are small and save recompute on restart.
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

        // Read and validate the version. Development builds keep a single
        // on-disk layout; version bumps only start after a release.
        let version = data.get(offset).copied().ok_or_else(|| {
            StorageError::deserialize_error("PropertyTable data missing version byte")
        })?;
        offset += 1;

        if version != PROPERTY_TABLE_VERSION {
            if version < PROPERTY_TABLE_VERSION {
                return Err(StorageError::deserialize_error(format!(
                    "PropertyTable data uses legacy layout version {version}, which is no \
                     longer supported; re-import the data to upgrade to version {PROPERTY_TABLE_VERSION}"
                )));
            }
            return Err(StorageError::deserialize_error(format!(
                "Unsupported PropertyTable version: expected {PROPERTY_TABLE_VERSION}, got {version}"
            )));
        }

        let schema_len = read_u32_le(data, &mut offset)? as usize;

        self.schema.clear();
        self.name_indexer.clear();
        self.column_byte_offsets.clear();

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
                    // Known parameterized type: read the postcard TypeInfo
                    // block that follows the code byte.
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

        // Recompute column byte offsets after schema is loaded
        self.recompute_column_byte_offsets();

        // Load PropertyRecords with MVCC support
        let records_len = read_u32_le(data, &mut offset)? as usize;
        self.records.clear();
        self.row_count = 0;
        self.used_data_bytes = 0;

        for _ in 0..records_len {
            if offset >= data.len() {
                return Err(StorageError::deserialize_error("unexpected end of data"));
            }
            let marker = data[offset];
            offset += 1;

            if marker == 1 {
                let create_ts = read_u64_le(data, &mut offset)?;
                let has_delete_ts = data[offset];
                offset += 1;
                let delete_ts = if has_delete_ts == 1 {
                    Some(read_u64_le(data, &mut offset)?)
                } else {
                    None
                };
                let data_len = read_u32_le(data, &mut offset)? as usize;
                if offset + data_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let record_data = data[offset..offset + data_len].to_vec();
                offset += data_len;

                self.used_data_bytes += record_data.len();
                let record = PropertyRecord {
                    data: record_data,
                    create_ts,
                    delete_ts,
                };
                self.records.push(Some(record));
                self.row_count += 1;
            } else {
                self.records.push(None);
            }
        }

        // Load before-image version chains, oldest first.
        self.chain_records.clear();
        for _ in 0..self.records.len() {
            let chain_len = read_u32_le(data, &mut offset)? as usize;
            let mut chain = Vec::with_capacity(chain_len);
            for _ in 0..chain_len {
                let create_ts = read_u64_le(data, &mut offset)?;
                let has_delete_ts = data[offset];
                offset += 1;
                let delete_ts = if has_delete_ts == 1 {
                    let d = read_u64_le(data, &mut offset)?;
                    Some(d)
                } else {
                    None
                };
                let data_len = read_u32_le(data, &mut offset)? as usize;
                if offset + data_len > data.len() {
                    return Err(StorageError::deserialize_error("unexpected end of data"));
                }
                let record_data = data[offset..offset + data_len].to_vec();
                offset += data_len;
                chain.push(PropertyRecord {
                    data: record_data,
                    create_ts,
                    delete_ts,
                });
            }
            self.chain_records.push(chain);
        }
        self.ensure_chain_len();

        // Rebuild tiered tombstone manager from record timestamps
        // Tombstones are fully derivable from record delete_ts, so nothing
        // is persisted for them.
        self.tombstones_manager = TieredTombstoneManager::new(10_000);
        for (idx, record_opt) in self.records.iter().enumerate() {
            if let Some(record) = record_opt {
                if let Some(delete_ts) = record.delete_ts {
                    let prop_offset = prop_index_to_offset(idx);
                    self.tombstones_manager
                        .add_tombstone(prop_offset, delete_ts);
                }
            }
        }

        // Load free list with Varint decoding
        let free_list_len = read_u32_le(data, &mut offset)? as usize;
        self.free_list.clear();
        for _ in 0..free_list_len {
            let mut cursor = Cursor::new(&data[offset..]);
            let off = decode_varint(&mut cursor)?;
            offset += cursor.position() as usize;
            self.free_list.push(off);
        }

        // Rebuild property value index from loaded records
        self.value_index.rebuild(&self.schema, &self.records);

        // ── load zone maps and rebuild columnar store ──
        self.column_store = ColumnStore::new();
        for prop in &self.schema {
            self.column_store
                .add_column(prop.name.clone(), prop.data_type.clone(), prop.nullable);
        }
        // Ensure column store has enough rows.
        if !self.records.is_empty() {
            self.column_store.resize(self.records.len());
        }
        // Rebuild columnar store from row records (dual-write source of truth).
        for (row_idx, record_opt) in self.records.iter().enumerate() {
            if let Some(rec) = record_opt {
                if rec.delete_ts.is_some() {
                    continue;
                }
                if let Ok(props) = self.deserialize_row(&rec.data) {
                    for (name, opt_val) in props {
                        let _ = self.column_store.set_property_versioned(
                            row_idx,
                            &name,
                            opt_val.as_ref(),
                            rec.create_ts,
                        );
                    }
                }
            }
        }
        // Rebuild version chains for historical rows from chain_records.
        for (row_idx, chain) in self.chain_records.iter().enumerate() {
            for rec in chain {
                if let Ok(props) = self.deserialize_row(&rec.data) {
                    for (name, opt_val) in props {
                        // Historical versions: visible on [create_ts, delete_ts)
                        // ColumnStore version chain is per-cell, so we push
                        // each historical value as a before-image.
                        let end_ts = rec.delete_ts.unwrap_or(Timestamp::MAX);
                        // Only push if genuinely historical (create < delete).
                        if rec.create_ts < end_ts {
                            // Simulate versioned write: set current to historical
                            // then overwrite with next version in next iteration.
                            // For simplicity, ensure row meta allows history:
                            let _ = self.column_store.set_property_versioned(
                                row_idx,
                                &name,
                                opt_val.as_ref(),
                                rec.create_ts,
                            );
                        }
                    }
                }
            }
        }

        self.zone_maps.clear();
        if offset < data.len() {
            // Zone maps
            if offset + 4 <= data.len() {
                if let Ok(zm_len) = read_u32_le(data, &mut offset) {
                    for _ in 0..zm_len as usize {
                        if offset + 4 > data.len() {
                            break;
                        }
                        let name_len = match read_u32_le(data, &mut offset) {
                            Ok(v) => v as usize,
                            Err(_) => break,
                        };
                        if offset + name_len > data.len() {
                            break;
                        }
                        let name =
                            String::from_utf8_lossy(&data[offset..offset + name_len]).to_string();
                        offset += name_len;
                        if offset + 4 > data.len() {
                            break;
                        }
                        let chunk_count = match read_u32_le(data, &mut offset) {
                            Ok(v) => v as usize,
                            Err(_) => break,
                        };
                        let mut chunks = Vec::with_capacity(chunk_count);
                        for _ in 0..chunk_count {
                            if offset + 4 > data.len() {
                                break;
                            }
                            let meta_len = match read_u32_le(data, &mut offset) {
                                Ok(v) => v as usize,
                                Err(_) => break,
                            };
                            if offset + meta_len > data.len() {
                                break;
                            }
                            let mut cur = &data[offset..offset + meta_len];
                            if let Ok(stats) = ColumnStats::deserialize_meta(&mut cur) {
                                chunks.push(stats);
                            }
                            offset += meta_len;
                        }
                        self.zone_maps.insert(name, chunks);
                    }
                }
            }
            // If zone maps were empty (e.g., fresh v4 file with no data), rebuild.
            if self.zone_maps.is_empty() && !self.records.is_empty() {
                self.rebuild_zone_maps();
            }
        } else {
            // Fresh file with no zone-map section: rebuild from row records.
            self.rebuild_zone_maps();
        }

        Ok(())
    }

    /// Get the byte size of a fixed-size data type in the serialized row format.
    /// Returns None for variable-size types (String, Date, etc.).
    pub(super) fn data_type_byte_size(dt: &DataType) -> Option<usize> {
        match dt {
            DataType::Bool => Some(1),
            DataType::SmallInt => Some(2),
            DataType::Int => Some(4),
            DataType::BigInt => Some(8),
            DataType::Float => Some(4),
            DataType::Double => Some(8),
            _ => None, // Variable-size types
        }
    }
}
