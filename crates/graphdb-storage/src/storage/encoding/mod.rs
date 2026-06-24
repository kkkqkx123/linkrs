//! Column Encoding Module
//!
//! Provides storage-level compression encodings:
//! - Dictionary encoding for low-cardinality strings
//! - RLE (Run-Length Encoding) for repeated values
//! - BitPacking for small-range integers
//! - FSST for long string compression
//! - ALP for floating-point compression
//! - Tiered compression strategy selector

pub mod alp;
pub mod bitpacking;
pub mod dictionary;
pub mod fsst;
pub mod rle;
pub mod selector;

use crate::core::{DataType, Value};

pub use alp::AlpColumn;
pub use bitpacking::BitPackedIntColumn;
pub use dictionary::DictionaryColumn;
pub use fsst::{FsstColumn, FsstEncoder};
pub use rle::{RleBoolColumn, RleIntColumn};
pub use selector::{ColumnStats, CompressionConfig, CompressionSelector};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EncodingType {
    #[default]
    None,
    Dictionary,
    Rle,
    BitPacking,
    Fsst,
    Alp,
}

impl EncodingType {
    pub fn to_u8(self) -> u8 {
        match self {
            EncodingType::None => 0,
            EncodingType::Dictionary => 1,
            EncodingType::Rle => 2,
            EncodingType::BitPacking => 3,
            EncodingType::Fsst => 4,
            EncodingType::Alp => 5,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => EncodingType::Dictionary,
            2 => EncodingType::Rle,
            3 => EncodingType::BitPacking,
            4 => EncodingType::Fsst,
            5 => EncodingType::Alp,
            _ => EncodingType::None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub enum ColumnEncoding {
    #[default]
    None,
    Fsst(FsstColumn),
    Dictionary(DictionaryColumn),
    RleInt(RleIntColumn),
    RleBool(RleBoolColumn),
    BitPacked(BitPackedIntColumn),
    Alp(AlpColumn),
}

impl ColumnEncoding {
    pub fn encoding_type(&self) -> EncodingType {
        match self {
            Self::None => EncodingType::None,
            Self::Fsst(_) => EncodingType::Fsst,
            Self::Dictionary(_) => EncodingType::Dictionary,
            Self::RleInt(_) | Self::RleBool(_) => EncodingType::Rle,
            Self::BitPacked(_) => EncodingType::BitPacking,
            Self::Alp(_) => EncodingType::Alp,
        }
    }

    pub fn get(&self, row_idx: usize) -> Option<Value> {
        match self {
            Self::None => None,
            Self::Fsst(col) => col.get(row_idx).map(Value::String),
            Self::Dictionary(col) => col.get(row_idx),
            Self::RleInt(col) => col.get(row_idx),
            Self::RleBool(col) => col.get(row_idx),
            Self::BitPacked(col) => col.get(row_idx),
            Self::Alp(col) => col.get_value(row_idx),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Fsst(col) => col.len(),
            Self::Dictionary(col) => col.len(),
            Self::RleInt(col) => col.len(),
            Self::RleBool(col) => col.len(),
            Self::BitPacked(col) => col.len(),
            Self::Alp(col) => col.len(),
        }
    }

    pub fn memory_usage(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Fsst(col) => col.memory_usage(),
            Self::Dictionary(col) => col.memory_usage(),
            Self::RleInt(col) => col.memory_usage(),
            Self::RleBool(col) => col.memory_usage(),
            Self::BitPacked(col) => col.memory_usage(),
            Self::Alp(col) => col.memory_usage(),
        }
    }

    pub fn is_encoded(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn set(&mut self, row_idx: usize, value: Option<&Value>) -> crate::core::StorageResult<()> {
        use crate::core::StorageError;

        match self {
            Self::None => Err(StorageError::invalid_operation(
                "Cannot set value on unencoded column through ColumnEncoding".to_string(),
            )),
            Self::Fsst(col) => match value {
                Some(Value::String(s)) => {
                    col.set(row_idx, Some(s.as_str()))?;
                    Ok(())
                }
                Some(v) => Err(StorageError::type_mismatch(DataType::String, v.data_type())),
                None => {
                    col.set(row_idx, None)?;
                    Ok(())
                }
            },
            Self::Dictionary(col) => {
                col.set(row_idx, value)?;
                Ok(())
            }
            Self::RleInt(col) => {
                col.append(value)?;
                Ok(())
            }
            Self::RleBool(col) => {
                col.append(value)?;
                Ok(())
            }
            Self::BitPacked(col) => {
                col.set(row_idx, value)?;
                Ok(())
            }
            Self::Alp(col) => {
                let float_val = value.and_then(|v| match v {
                    Value::Float(f) => Some(*f as f64),
                    Value::Double(d) => Some(*d),
                    _ => None,
                });
                col.set(row_idx, float_val)?;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::NullBitmap;

    fn build_fsst_column(strings: &[Option<&str>], max_symbols: usize) -> FsstColumn {
        let non_null: Vec<&str> = strings.iter().filter_map(|s| *s).collect();
        let encoder = if non_null.is_empty() {
            FsstEncoder::new()
        } else {
            FsstEncoder::train(&non_null, max_symbols)
        };

        let mut column = FsstColumn {
            encoder,
            encoded_data: Vec::with_capacity(strings.len()),
            null_bitmap: NullBitmap::with_capacity(strings.len()),
        };

        for value in strings {
            match value {
                Some(s) => {
                    column.encoded_data.push(column.encoder.encode(s));
                    column.null_bitmap.push(false);
                }
                None => {
                    column.encoded_data.push(Vec::new());
                    column.null_bitmap.push(true);
                }
            }
        }

        column
    }

    #[test]
    fn test_column_encoding_none() {
        let encoding = ColumnEncoding::None;

        assert_eq!(encoding.encoding_type(), EncodingType::None);
        assert!(!encoding.is_encoded());
        assert_eq!(encoding.len(), 0);
        assert_eq!(encoding.memory_usage(), 0);
        assert!(encoding.get(0).is_none());
    }

    #[test]
    fn test_column_encoding_fsst() {
        let strings = vec![Some("hello world"), None, Some("hello rust")];
        let col = build_fsst_column(&strings, 100);
        let encoding = ColumnEncoding::Fsst(col);

        assert_eq!(encoding.encoding_type(), EncodingType::Fsst);
        assert!(encoding.is_encoded());
        assert_eq!(encoding.len(), 3);
        assert!(encoding.memory_usage() > 0);
        assert!(encoding.get(0).is_some());
    }

    #[test]
    fn test_column_encoding_dictionary() {
        let mut col = DictionaryColumn::new();
        col.set(0, Some(&Value::String("apple".to_string())))
            .unwrap();
        col.set(1, Some(&Value::String("banana".to_string())))
            .unwrap();
        col.set(2, None).unwrap();

        let encoding = ColumnEncoding::Dictionary(col);

        assert_eq!(encoding.encoding_type(), EncodingType::Dictionary);
        assert!(encoding.is_encoded());
        assert_eq!(encoding.len(), 3);
        assert!(encoding.get(0).is_some());
    }

    #[test]
    fn test_column_encoding_rle_int() {
        let mut col = RleIntColumn::new();
        col.append(Some(&Value::Int(1))).unwrap();
        col.append(Some(&Value::Int(1))).unwrap();
        col.append(Some(&Value::Int(2))).unwrap();

        let encoding = ColumnEncoding::RleInt(col);

        assert_eq!(encoding.encoding_type(), EncodingType::Rle);
        assert!(encoding.is_encoded());
        assert_eq!(encoding.len(), 3);
        assert!(encoding.get(0).is_some());
    }

    #[test]
    fn test_column_encoding_bitpacked() {
        let values = vec![
            Some(Value::Int(10)),
            Some(Value::Int(20)),
            Some(Value::Int(30)),
        ];
        let col = BitPackedIntColumn::analyze(&values, DataType::Int).unwrap();

        let encoding = ColumnEncoding::BitPacked(col);

        assert_eq!(encoding.encoding_type(), EncodingType::BitPacking);
        assert!(encoding.is_encoded());
        assert_eq!(encoding.len(), 3);
        assert!(encoding.get(0).is_some());
    }

    #[test]
    fn test_column_encoding_alp() {
        let values = vec![Some(Value::Double(1.5)), Some(Value::Double(2.5)), None];
        let col = AlpColumn::analyze_values(&values, DataType::Double).unwrap();

        let encoding = ColumnEncoding::Alp(col);

        assert_eq!(encoding.encoding_type(), EncodingType::Alp);
        assert!(encoding.is_encoded());
        assert_eq!(encoding.len(), 3);
        assert!(encoding.get(0).is_some());
    }

    #[test]
    fn test_column_encoding_set_fsst() {
        let strings = vec![Some("hello")];
        let col = build_fsst_column(&strings, 100);
        let mut encoding = ColumnEncoding::Fsst(col);

        encoding
            .set(0, Some(&Value::String("world".to_string())))
            .unwrap();
        assert_eq!(encoding.get(0), Some(Value::String("world".to_string())));

        encoding.set(0, None).unwrap();
    }
}
