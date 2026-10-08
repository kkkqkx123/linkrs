use std::collections::BTreeSet;
use std::sync::atomic::Ordering;

use linkrs_core::{DataType, StorageError, StorageResult, Value};

use super::Column;

impl Column {
    /// Mark the page containing `row_idx` as dirty. The mark lives in the
    /// owning chunk's segment state; the page id is global so flush
    /// aggregation stays a plain union. The owner is located by direct
    /// capacity division with a range recheck, falling back to a linear
    /// scan only for windows realigned by a capacity change.
    #[inline]
    pub fn mark_dirty(&self, row_idx: usize) {
        let page_id = crate::persistence::dirty_page::row_to_page(row_idx);
        self.total_dirty_pages
            .fetch_max(page_id + 1, Ordering::Relaxed);
        let chunks = self.chunks.read();
        let direct = row_idx / self.chunk_capacity().max(1);
        if let Some(chunk) = chunks.get(direct) {
            if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                chunk.write_state().dirty_pages.insert(page_id as u32);
                return;
            }
        }
        for chunk in chunks.iter() {
            if row_idx >= chunk.row_offset && row_idx < chunk.row_offset + chunk.row_count {
                chunk.write_state().dirty_pages.insert(page_id as u32);
                break;
            }
        }
    }

    /// High-water page count backing the dirty-ratio pre-pass. Monotonic
    /// maximum recording the largest page id ever dirtied; reset only by
    /// exclusive `clear`. Table-level ratios use actual row-count pages, so
    /// this high-water mark is an observability hint, not routing state.
    pub fn total_pages(&self) -> usize {
        self.total_dirty_pages.load(Ordering::Relaxed)
    }

    pub fn dirty_pages(&self) -> Vec<usize> {
        let chunks = self.chunks.read();
        let mut pages = BTreeSet::new();
        for chunk in chunks.iter() {
            pages.extend(chunk.read_state().dirty_pages.iter().copied());
        }
        pages.into_iter().map(|id| id as usize).collect()
    }

    pub fn dirty_count(&self) -> usize {
        let chunks = self.chunks.read();
        chunks
            .iter()
            .map(|chunk| chunk.read_state().dirty_pages.len())
            .sum()
    }

    pub fn clear_dirty(&self) {
        let chunks = self.chunks.read();
        for chunk in chunks.iter() {
            chunk.write_state().dirty_pages.clear();
        }
        self.total_dirty_pages.store(0, Ordering::Relaxed);
    }

    /// Clear the dirty mark for a single row-page (keeps other dirty pages).
    ///
    /// Page ids are global; the owner is located by direct capacity division
    /// from the page's first row with a range recheck, falling back to a
    /// scan only for windows realigned by a capacity change.
    #[inline]
    pub fn clear_page_dirty(&self, page_id: usize) {
        let chunks = self.chunks.read();
        let Ok(mark) = u32::try_from(page_id) else {
            return;
        };
        let first_row = page_id * crate::persistence::dirty_page::ROWS_PER_PAGE;
        let direct = first_row / self.chunk_capacity().max(1);
        if let Some(chunk) = chunks.get(direct) {
            if first_row >= chunk.row_offset
                && first_row < chunk.row_offset + chunk.row_count
                && chunk.write_state().dirty_pages.remove(&mark)
            {
                return;
            }
        }
        for chunk in chunks.iter() {
            if chunk.write_state().dirty_pages.remove(&mark) {
                break;
            }
        }
    }

    /// Serialize a single page for incremental checkpoint.
    ///
    /// Compact per-type encoding: fixed columns copy raw bytes, variable
    /// columns copy length-prefixed payloads, plus a null bitmap. No generic
    /// value-enum framing crosses the page boundary, so equal increments
    /// persist fewer bytes with less CPU. Page boundaries and dirty
    /// granularity stay unchanged. Old generic pages fail the magic check
    /// and rebuild through a fresh checkpoint with no dual-format branch.
    pub fn serialize_page(&self, page_id: usize) -> StorageResult<Vec<u8>> {
        use super::fixed_width::{element_size, write_fixed_value};
        let rows_per_page = crate::persistence::dirty_page::ROWS_PER_PAGE;
        let start = page_id * rows_per_page;
        let total = self.len();
        if start >= total {
            return Err(StorageError::invalid_input(format!(
                "page {} out of range (total rows {})",
                page_id, total
            )));
        }
        let end = (start + rows_per_page).min(total);
        let count = end - start;
        let mut payload = Vec::new();
        payload.extend_from_slice(b"CPG1");
        payload.extend_from_slice(&(count as u32).to_le_bytes());
        let elem = element_size(&self.data_type);
        let is_fixed = elem > 0;
        let mut null_bits = vec![0u8; count.div_ceil(8)];
        let mut body = Vec::new();
        for (i, row) in (start..end).enumerate() {
            let value = self.try_get(row).map_err(|e| {
                StorageError::deserialize_error(format!(
                    "column {} page {} row {} encode failed: {}",
                    self.name, page_id, row, e
                ))
            })?;
            if value.is_none() {
                null_bits[i / 8] |= 1 << (i % 8);
                if is_fixed {
                    body.extend(std::iter::repeat_n(0u8, elem));
                } else {
                    body.extend_from_slice(&0u64.to_le_bytes());
                }
                continue;
            }
            let v = value.as_ref().unwrap();
            if is_fixed {
                if let DataType::FixedString(limit) = &self.data_type {
                    let bytes: &[u8] = match v {
                        Value::FixedString(s) => s.as_bytes(),
                        Value::String(s) => s.as_bytes(),
                        _ => {
                            return Err(StorageError::deserialize_error(format!(
                                "column {} page {} row {} fixed string type mismatch",
                                self.name, page_id, row
                            )));
                        }
                    };
                    if bytes.len() > *limit {
                        return Err(StorageError::deserialize_error(format!(
                            "column {} page {} row {} fixed string overflow",
                            self.name, page_id, row
                        )));
                    }
                    body.extend_from_slice(bytes);
                    body.extend(std::iter::repeat_n(0u8, limit - bytes.len()));
                } else if let DataType::VectorDense(dim) = &self.data_type {
                    let dense: &[f32] = match v {
                        Value::Vector(vec) => vec.as_dense().ok_or_else(|| {
                            StorageError::deserialize_error(format!(
                                "column {} page {} row {} sparse vector in dense slot",
                                self.name, page_id, row
                            ))
                        })?,
                        _ => {
                            return Err(StorageError::deserialize_error(format!(
                                "column {} page {} row {} vector type mismatch",
                                self.name, page_id, row
                            )));
                        }
                    };
                    if dense.len() != *dim {
                        return Err(StorageError::deserialize_error(format!(
                            "column {} page {} row {} vector dimension mismatch",
                            self.name, page_id, row
                        )));
                    }
                    for f in dense {
                        body.extend_from_slice(&f.to_le_bytes());
                    }
                } else {
                    let mut slot = vec![0u8; elem];
                    write_fixed_value(&mut slot, 0, elem, v).map_err(|e| {
                        StorageError::deserialize_error(format!(
                            "column {} page {} row {} fixed encode failed: {}",
                            self.name, page_id, row, e
                        ))
                    })?;
                    body.extend_from_slice(&slot);
                }
            } else {
                super::variable_width::write_variable_value(&mut body, v).map_err(|e| {
                    StorageError::deserialize_error(format!(
                        "column {} page {} row {} variable encode failed: {}",
                        self.name, page_id, row, e
                    ))
                })?;
            }
        }
        payload.extend_from_slice(&(null_bits.len() as u32).to_le_bytes());
        payload.extend_from_slice(&null_bits);
        payload.extend_from_slice(&body);
        let page_data =
            crate::persistence::dirty_page::PageData::new(page_id as u32, payload, false);
        Ok(page_data.serialize())
    }

    /// Deserialize and apply a single compact page. Does not mark the page
    /// dirty (clean after checkpoint load). Old generic pages fail the magic
    /// check with no fallback branch.
    pub fn deserialize_page(&self, data: &[u8]) -> StorageResult<()> {
        use super::fixed_width::{convert_to_type, element_size, read_fixed_value};
        let page =
            crate::persistence::dirty_page::PageData::deserialize(data).ok_or_else(|| {
                StorageError::deserialize_error(
                    "invalid page data or checksum mismatch".to_string(),
                )
            })?;
        let mut cursor = &page.data[..];
        if cursor.len() < 8 || &cursor[..4] != b"CPG1" {
            return Err(StorageError::deserialize_error(
                "page payload magic mismatch: old generic page format is not supported, rebuild through a fresh checkpoint".to_string(),
            ));
        }
        cursor = &cursor[4..];
        let mut u32b = [0u8; 4];
        u32b.copy_from_slice(&cursor[..4]);
        cursor = &cursor[4..];
        let count = u32::from_le_bytes(u32b) as usize;
        u32b.copy_from_slice(&cursor[..4]);
        cursor = &cursor[4..];
        let bitmap_len = u32::from_le_bytes(u32b) as usize;
        if cursor.len() < bitmap_len {
            return Err(StorageError::deserialize_error(
                "compact page truncated null bitmap".to_string(),
            ));
        }
        let null_bits = &cursor[..bitmap_len];
        cursor = &cursor[bitmap_len..];
        let rows_per_page = crate::persistence::dirty_page::ROWS_PER_PAGE;
        let start = page.header.page_id as usize * rows_per_page;
        if start + count > self.len() {
            self.resize(start + count);
            if start + count > self.len() {
                return Err(StorageError::deserialize_error(format!(
                    "column {} page {} replay length {} exceeds column length {}",
                    self.name,
                    page.header.page_id,
                    start + count,
                    self.len()
                )));
            }
        }
        let elem = element_size(&self.data_type);
        let is_fixed = elem > 0;
        for i in 0..count {
            let row_idx = start + i;
            let is_null = null_bits
                .get(i / 8)
                .is_some_and(|b| (b >> (i % 8)) & 1 == 1);
            if is_null {
                self.write_value_without_dirty(row_idx, None)?;
                if is_fixed {
                    if cursor.len() < elem {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated fixed body".to_string(),
                        ));
                    }
                    cursor = &cursor[elem..];
                } else {
                    if cursor.len() < 8 {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated variable prefix".to_string(),
                        ));
                    }
                    let mut lb = [0u8; 8];
                    lb.copy_from_slice(&cursor[..8]);
                    let len = u64::from_le_bytes(lb) as usize;
                    cursor = &cursor[8..];
                    if cursor.len() < len {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated variable body".to_string(),
                        ));
                    }
                    cursor = &cursor[len..];
                }
                continue;
            }
            if is_fixed {
                if let DataType::FixedString(limit) = &self.data_type {
                    if cursor.len() < *limit {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated fixed string body".to_string(),
                        ));
                    }
                    let slot = &cursor[..*limit];
                    cursor = &cursor[*limit..];
                    let mut end = slot.len();
                    while end > 0 && slot[end - 1] == 0 {
                        end -= 1;
                    }
                    let s = String::from_utf8(slot[..end].to_vec()).map_err(|e| {
                        StorageError::deserialize_error(format!("compact page UTF-8: {}", e))
                    })?;
                    let v = Value::FixedString(s);
                    self.write_value_without_dirty(row_idx, Some(&v))?;
                } else if let DataType::VectorDense(dim) = &self.data_type {
                    let bytes = dim * 4;
                    if cursor.len() < bytes {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated vector body".to_string(),
                        ));
                    }
                    let slot = &cursor[..bytes];
                    cursor = &cursor[bytes..];
                    let mut out = Vec::with_capacity(*dim);
                    for k in 0..*dim {
                        let chunk: [u8; 4] = slot[k * 4..(k + 1) * 4].try_into().map_err(|_| {
                            StorageError::deserialize_error(
                                "compact page vector component undecodable".to_string(),
                            )
                        })?;
                        out.push(f32::from_le_bytes(chunk));
                    }
                    let v = Value::Vector(linkrs_core::value::VectorValue::dense(out));
                    self.write_value_without_dirty(row_idx, Some(&v))?;
                } else {
                    if cursor.len() < elem {
                        return Err(StorageError::deserialize_error(
                            "compact page truncated fixed body".to_string(),
                        ));
                    }
                    let slot = &cursor[..elem];
                    cursor = &cursor[elem..];
                    let raw = read_fixed_value(slot, 0, elem).ok_or_else(|| {
                        StorageError::deserialize_error(
                            "compact page fixed value undecodable".to_string(),
                        )
                    })?;
                    let v = convert_to_type(raw, &self.data_type);
                    self.write_value_without_dirty(row_idx, Some(&v))?;
                }
            } else {
                if cursor.len() < 8 {
                    return Err(StorageError::deserialize_error(
                        "compact page truncated variable prefix".to_string(),
                    ));
                }
                let mut lb = [0u8; 8];
                lb.copy_from_slice(&cursor[..8]);
                cursor = &cursor[8..];
                let len = u64::from_le_bytes(lb) as usize;
                if cursor.len() < len {
                    return Err(StorageError::deserialize_error(
                        "compact page truncated variable body".to_string(),
                    ));
                }
                let payload = &cursor[..len];
                cursor = &cursor[len..];
                let mut tmp = Vec::with_capacity(8 + len);
                tmp.extend_from_slice(&(len as u64).to_le_bytes());
                tmp.extend_from_slice(payload);
                let v = self.decode_compact_variable_payload(&tmp)?;
                self.write_value_without_dirty(row_idx, Some(&v))?;
            }
        }
        if !cursor.is_empty() {
            return Err(StorageError::deserialize_error(
                "compact page trailing bytes".to_string(),
            ));
        }
        // Mark page clean after successful restore (was dirtied by writes if any).
        self.clear_page_dirty(page.header.page_id as usize);
        Ok(())
    }

    /// Decode one compact variable payload (`len prefix + bytes`) for this
    /// column type. Mirrors the strict variable decoder with explicit errors
    /// and no generic value-enum framing.
    fn decode_compact_variable_payload(&self, tmp: &[u8]) -> StorageResult<Value> {
        use linkrs_core::value::VectorValue;
        if tmp.len() < 8 {
            return Err(StorageError::deserialize_error(
                "compact variable payload truncated prefix".to_string(),
            ));
        }
        let mut lb = [0u8; 8];
        lb.copy_from_slice(&tmp[..8]);
        let len = u64::from_le_bytes(lb) as usize;
        if tmp.len() < 8 + len {
            return Err(StorageError::deserialize_error(
                "compact variable payload truncated body".to_string(),
            ));
        }
        let bytes = &tmp[8..8 + len];
        if matches!(self.data_type, DataType::Geography) {
            let geo =
                postcard::from_bytes::<linkrs_core::value::Geography>(bytes).map_err(|e| {
                    StorageError::deserialize_error(format!("compact geography: {}", e))
                })?;
            Ok(Value::Geography(geo))
        } else if matches!(
            self.data_type,
            DataType::Vector | DataType::VectorDense(_) | DataType::VectorSparse(_)
        ) {
            if !bytes.len().is_multiple_of(4) {
                return Err(StorageError::deserialize_error(
                    "compact vector length not a multiple of 4".to_string(),
                ));
            }
            let dim = bytes.len() / 4;
            if let DataType::VectorDense(expected) = &self.data_type {
                if *expected > 0 && dim != *expected {
                    return Err(StorageError::deserialize_error(format!(
                        "compact vector dimension mismatch: need {}, got {}",
                        expected, dim
                    )));
                }
            }
            let mut out = Vec::with_capacity(dim);
            for i in 0..dim {
                let chunk: [u8; 4] = bytes[i * 4..(i + 1) * 4].try_into().map_err(|_| {
                    StorageError::deserialize_error(
                        "compact vector component undecodable".to_string(),
                    )
                })?;
                out.push(f32::from_le_bytes(chunk));
            }
            Ok(Value::Vector(VectorValue::dense(out)))
        } else if matches!(self.data_type, DataType::Json) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!("compact JSON UTF-8: {}", e))
            })?;
            let j = linkrs_core::value::Json::parse(&s)
                .map_err(|e| StorageError::deserialize_error(format!("compact JSON: {}", e)))?;
            Ok(Value::Json(Box::new(j)))
        } else if matches!(self.data_type, DataType::JsonB) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!("compact JSONB UTF-8: {}", e))
            })?;
            let jb = linkrs_core::value::JsonB::parse(&s)
                .map_err(|e| StorageError::deserialize_error(format!("compact JSONB: {}", e)))?;
            Ok(Value::JsonB(Box::new(jb)))
        } else if matches!(self.data_type, DataType::FixedString(_)) {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!("compact fixed string UTF-8: {}", e))
            })?;
            Ok(Value::FixedString(s))
        } else if matches!(
            self.data_type,
            DataType::Struct(_)
                | DataType::Array(_)
                | DataType::List(_)
                | DataType::Map(_)
                | DataType::Set(_)
                | DataType::DataSet
                | DataType::Vertex
                | DataType::Edge
                | DataType::Path
                | DataType::Interval
                | DataType::Decimal128
                | DataType::Decimal { .. }
                | DataType::Union(_)
        ) {
            let v = postcard::from_bytes::<Value>(bytes)
                .map_err(|e| StorageError::deserialize_error(format!("compact opaque: {}", e)))?;
            Ok(v)
        } else if matches!(self.data_type, DataType::Blob) {
            Ok(Value::Blob(bytes.to_vec()))
        } else {
            let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
                StorageError::deserialize_error(format!("compact string UTF-8: {}", e))
            })?;
            Ok(Value::string(s))
        }
    }
}
