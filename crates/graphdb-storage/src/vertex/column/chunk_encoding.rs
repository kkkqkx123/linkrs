//! Chunk-level encoding overlay and in-place update decisions.
//!
//! An `UpdateOverlay` buffers row-level overwrites on top of an encoded chunk
//! so point writes never pay for a whole-column decode. Reads check the
//! overlay first, then the encoded base. Flush merges the overlay back into
//! the base and re-encodes the chunk.

use std::collections::HashMap;

use graphdb_core::{DataType, Value};

use crate::encoding::{ChunkEncodingMeta, ColumnEncoding};

/// Chunk-row divisor turning chunk size into its overlay budget.
///
/// Pending-update memory for a column is then `rows / OVERLAY_CAPACITY_DIVISOR`
/// no matter how finely the column is chunked: shrinking the chunk shrinks
/// each chunk's budget by the same factor.
pub const OVERLAY_CAPACITY_DIVISOR: usize = 64;

/// Overlay budget for a chunk holding `chunk_rows` rows.
pub fn overlay_capacity_for(chunk_rows: usize) -> usize {
    (chunk_rows / OVERLAY_CAPACITY_DIVISOR).max(1)
}

/// Default dictionary entry cap per chunk.
pub const DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK: usize = 65536;
/// Default ALP exception-rate ceiling above which a chunk falls back to raw.
pub const DEFAULT_ALP_EXCEPTION_THRESHOLD: f64 = 0.25;

/// Row-level overwrite buffer on top of an encoded chunk.
#[derive(Debug, Clone, Default)]
pub struct UpdateOverlay {
    values: HashMap<u32, Option<Value>>,
    capacity: usize,
}

impl UpdateOverlay {
    pub fn new(capacity: usize) -> Self {
        Self {
            values: HashMap::new(),
            capacity: capacity.max(1),
        }
    }

    pub fn get(&self, local_row: u32) -> Option<Option<Value>> {
        self.values.get(&local_row).cloned()
    }

    pub fn put(&mut self, local_row: u32, value: Option<Value>) {
        self.values.insert(local_row, value);
    }

    pub fn remove(&mut self, local_row: u32) {
        self.values.remove(&local_row);
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Row budget this overlay absorbs before the chunk is due a re-encode.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn is_full(&self) -> bool {
        self.values.len() >= self.capacity
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u32, &Option<Value>)> {
        self.values.iter()
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }

    pub fn memory_usage(&self) -> usize {
        self.values
            .values()
            .map(|v| {
                std::mem::size_of::<(u32, Option<Value>)>()
                    + v.as_ref().map(|vv| vv.estimated_size()).unwrap_or(0)
            })
            .sum::<usize>()
    }
}

/// In-place update decision for one row write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    InPlace,
    Overlay,
    OverlayAndRecode,
}

/// Decide how a row write should be applied to an encoded chunk.
pub fn can_update_in_place(
    encoding: &ColumnEncoding,
    data_type: &DataType,
    meta: &ChunkEncodingMeta,
    row: u32,
    value: Option<&Value>,
    overlay_full: bool,
) -> UpdateDecision {
    if overlay_full {
        return UpdateDecision::OverlayAndRecode;
    }
    match encoding {
        ColumnEncoding::None => UpdateDecision::InPlace,
        ColumnEncoding::Constant(col) => {
            let base = col.value();
            match (base, value) {
                (None, None) => UpdateDecision::InPlace,
                (Some(a), Some(b)) if &a == b => UpdateDecision::InPlace,
                (None, Some(_)) | (Some(_), None) => UpdateDecision::Overlay,
                _ => UpdateDecision::Overlay,
            }
        }
        ColumnEncoding::BitPacked(bp) => match value {
            None => UpdateDecision::InPlace,
            Some(v) => {
                if bp.can_store_value(v) {
                    UpdateDecision::InPlace
                } else {
                    UpdateDecision::OverlayAndRecode
                }
            }
        },
        ColumnEncoding::RleInt(col) => {
            if is_rle_tail_append(col.len() as u32, row, value, col.get(row as usize)) {
                UpdateDecision::InPlace
            } else {
                UpdateDecision::Overlay
            }
        }
        ColumnEncoding::RleBool(col) => {
            if is_rle_tail_append(col.len() as u32, row, value, col.get(row as usize)) {
                UpdateDecision::InPlace
            } else {
                UpdateDecision::Overlay
            }
        }
        ColumnEncoding::Alp(col) => {
            if col.spare_exception_slots() > 0 {
                UpdateDecision::InPlace
            } else {
                let rate = col.exception_rate();
                if rate > DEFAULT_ALP_EXCEPTION_THRESHOLD {
                    UpdateDecision::OverlayAndRecode
                } else {
                    UpdateDecision::Overlay
                }
            }
        }
        ColumnEncoding::Dictionary(col) => match value {
            None => UpdateDecision::InPlace,
            Some(v) => {
                let s = v.string_value();
                match s {
                    None => UpdateDecision::Overlay,
                    Some(text) => {
                        if col.contains(text)
                            || col.len_entries() < DEFAULT_DICT_MAX_ENTRIES_PER_CHUNK
                        {
                            UpdateDecision::InPlace
                        } else {
                            UpdateDecision::Overlay
                        }
                    }
                }
            }
        },
        ColumnEncoding::Fsst(_) => {
            let _ = (data_type, meta);
            UpdateDecision::InPlace
        }
    }
}

fn is_rle_tail_append(len: u32, row: u32, value: Option<&Value>, current: Option<Value>) -> bool {
    if row == len {
        return true;
    }
    match (current, value) {
        (Some(a), Some(b)) => &a == b,
        (None, None) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::{BitPackedIntColumn, ConstantColumn, DictionaryColumn, EncodingType};

    fn empty_meta(scheme: EncodingType) -> ChunkEncodingMeta {
        ChunkEncodingMeta {
            scheme,
            ..Default::default()
        }
    }

    #[test]
    fn overlay_put_get_clear() {
        let mut ov = UpdateOverlay::new(2);
        assert!(!ov.is_full());
        ov.put(1, Some(Value::Int(7)));
        ov.put(2, None);
        assert!(ov.is_full());
        assert_eq!(ov.get(1), Some(Some(Value::Int(7))));
        assert_eq!(ov.len(), 2);
        let entries: Vec<_> = ov.iter().map(|(k, v)| (*k, v.clone())).collect();
        assert_eq!(entries.len(), 2);
        ov.clear();
        assert_eq!(ov.len(), 0);
    }

    #[test]
    fn constant_decision_table() {
        let col = ConstantColumn::new(Some(Value::Int(1)), 4);
        let enc = ColumnEncoding::Constant(col);
        let meta = empty_meta(EncodingType::Constant);
        assert_eq!(
            can_update_in_place(&enc, &DataType::Int, &meta, 0, Some(&Value::Int(1)), false),
            UpdateDecision::InPlace
        );
        assert_eq!(
            can_update_in_place(&enc, &DataType::Int, &meta, 0, Some(&Value::Int(2)), false),
            UpdateDecision::Overlay
        );
    }

    #[test]
    fn bitpacking_fit_and_overflow() {
        let values = vec![Some(Value::Int(0)), Some(Value::Int(3))];
        let bp = BitPackedIntColumn::analyze(&values, DataType::Int).unwrap();
        let enc = ColumnEncoding::BitPacked(bp);
        let meta = empty_meta(EncodingType::BitPacking);
        assert_eq!(
            can_update_in_place(&enc, &DataType::Int, &meta, 0, Some(&Value::Int(2)), false),
            UpdateDecision::InPlace
        );
        assert_eq!(
            can_update_in_place(
                &enc,
                &DataType::Int,
                &meta,
                0,
                Some(&Value::Int(1_000_000)),
                false
            ),
            UpdateDecision::OverlayAndRecode
        );
    }

    #[test]
    fn overlay_full_forces_recode() {
        let values = vec![Some(Value::Int(0)), Some(Value::Int(3))];
        let bp = BitPackedIntColumn::analyze(&values, DataType::Int).unwrap();
        let enc = ColumnEncoding::BitPacked(bp);
        let meta = empty_meta(EncodingType::BitPacking);
        assert_eq!(
            can_update_in_place(&enc, &DataType::Int, &meta, 0, Some(&Value::Int(1)), true),
            UpdateDecision::OverlayAndRecode
        );
    }

    #[test]
    fn dictionary_known_vs_overflow() {
        let mut dict = DictionaryColumn::new();
        dict.set(0, Some(&Value::string("a"))).unwrap();
        let enc = ColumnEncoding::Dictionary(dict);
        let meta = empty_meta(EncodingType::Dictionary);
        assert_eq!(
            can_update_in_place(
                &enc,
                &DataType::String,
                &meta,
                0,
                Some(&Value::string("a")),
                false
            ),
            UpdateDecision::InPlace
        );
        assert_eq!(
            can_update_in_place(
                &enc,
                &DataType::String,
                &meta,
                0,
                Some(&Value::string("b")),
                false
            ),
            UpdateDecision::InPlace
        );
        assert_eq!(
            can_update_in_place(&enc, &DataType::String, &meta, 0, None, false),
            UpdateDecision::InPlace
        );
    }
}
