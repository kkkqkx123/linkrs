//! Shared variant set and helpers for typed columnar layouts.
//!
//! [`TypedColumn`](super::typed::TypedColumn) (immutable per-chunk snapshot)
//! and [`BatchColumn`](super::columnar_batch::BatchColumn) (accumulable
//! blocking-operator state) intentionally share one variant set so a chunk's
//! typed columns can move into a batch without re-inspecting values. The
//! single source of truth lives here:
//!
//! - [`column_variants!`] declares the enum plus the mechanical `len`,
//!   `is_empty`, and `estimated_size` methods for both types;
//! - [`gather_column!`] builds either output from a `TypedColumn` source at
//!   an index selection, backing both gather entry points;
//! - the validity-bitmap helpers and [`gather_bitmap`] are shared instead of
//!   being duplicated per module.
//!
//! `value_at`, comparison, and accumulation keep hand-written impls: their
//! contracts differ per type (`Option<Value>` vs `Value`, `Empty`, row
//! comparison), so generating them would obscure rather than unify.

use std::sync::Arc;

use linkrs_core::types::VertexId;
use linkrs_core::{EdgeHeader, Value};

/// Struct-of-arrays edge identity column: one `(src, dst, edge_type,
/// ranking)` tuple per row with no property map and no storage-internal
/// identifier. All four vectors always share one length; every constructor
/// and mutator below maintains that invariant together, so element access
/// stays index-aligned without per-access checks.
#[derive(Debug, Clone)]
pub struct EdgeHeaderColumn {
    src: Vec<VertexId>,
    dst: Vec<VertexId>,
    edge_type: Vec<Arc<str>>,
    ranking: Vec<i64>,
}

impl EdgeHeaderColumn {
    /// Take over storage batch vectors. The storage batch contract guarantees
    /// equal lengths across the four vectors.
    pub(crate) fn from_parts(
        src: Vec<VertexId>,
        dst: Vec<VertexId>,
        edge_type: Vec<String>,
        ranking: Vec<i64>,
    ) -> Self {
        debug_assert!(
            src.len() == dst.len() && src.len() == edge_type.len() && src.len() == ranking.len(),
            "edge identity vectors must share one length"
        );
        let edge_type = edge_type
            .into_iter()
            .map(|s| Arc::from(s.as_str()))
            .collect();
        Self {
            src,
            dst,
            edge_type,
            ranking,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.src.len()
    }

    pub(crate) fn estimated_size(&self) -> usize {
        self.src.capacity() * std::mem::size_of::<VertexId>()
            + self.dst.capacity() * std::mem::size_of::<VertexId>()
            + self.edge_type.iter().map(|s| s.len()).sum::<usize>()
            + self.ranking.capacity() * std::mem::size_of::<i64>()
    }

    /// Materialize the header at `idx` (`None` when out of bounds).
    pub(crate) fn header_at(&self, idx: usize) -> Option<EdgeHeader> {
        match (
            self.src.get(idx),
            self.dst.get(idx),
            self.edge_type.get(idx),
            self.ranking.get(idx),
        ) {
            (Some(&src), Some(&dst), Some(edge_type), Some(&ranking)) => {
                Some(EdgeHeader::new(src, dst, edge_type.to_string(), ranking))
            }
            _ => None,
        }
    }

    /// Materialize every row (row-major fallback for row-path consumers).
    pub(crate) fn to_values(&self) -> Vec<Value> {
        self.src
            .iter()
            .zip(self.dst.iter())
            .zip(self.edge_type.iter())
            .zip(self.ranking.iter())
            .map(|(((src, dst), edge_type), ranking)| {
                Value::edge_header(EdgeHeader::new(*src, *dst, edge_type.to_string(), *ranking))
            })
            .collect()
    }

    /// Gather the entries at `indices`, preserving order.
    pub(crate) fn gather(&self, indices: &[usize]) -> Self {
        Self {
            src: indices.iter().map(|&i| self.src[i]).collect(),
            dst: indices.iter().map(|&i| self.dst[i]).collect(),
            edge_type: indices.iter().map(|&i| self.edge_type[i].clone()).collect(),
            ranking: indices.iter().map(|&i| self.ranking[i]).collect(),
        }
    }

    pub(crate) fn truncate(&mut self, len: usize) {
        self.src.truncate(len);
        self.dst.truncate(len);
        self.edge_type.truncate(len);
        self.ranking.truncate(len);
    }

    /// Reorder rows by `perm` (`result[i] = old[perm[i]]`).
    pub(crate) fn permute(&mut self, perm: &[usize]) {
        let old = std::mem::replace(
            self,
            Self {
                src: Vec::new(),
                dst: Vec::new(),
                edge_type: Vec::new(),
                ranking: Vec::new(),
            },
        );
        self.src = perm.iter().map(|&i| old.src[i]).collect();
        self.dst = perm.iter().map(|&i| old.dst[i]).collect();
        self.edge_type = perm.iter().map(|&i| old.edge_type[i].clone()).collect();
        self.ranking = perm.iter().map(|&i| old.ranking[i]).collect();
    }

    /// Append the entries of `other` at `indices`.
    pub(crate) fn extend_indexed(&mut self, other: &Self, indices: &[usize]) {
        self.src.extend(indices.iter().map(|&i| other.src[i]));
        self.dst.extend(indices.iter().map(|&i| other.dst[i]));
        self.edge_type
            .extend(indices.iter().map(|&i| other.edge_type[i].clone()));
        self.ranking
            .extend(indices.iter().map(|&i| other.ranking[i]));
    }

    /// Append one header.
    pub(crate) fn push(&mut self, src: VertexId, dst: VertexId, edge_type: &str, ranking: i64) {
        self.src.push(src);
        self.dst.push(dst);
        self.edge_type.push(Arc::from(edge_type));
        self.ranking.push(ranking);
    }
}

/// Whether row `idx` is valid in `bitmap` (bit set = valid, bit clear = NULL).
#[inline]
pub(crate) fn bitmap_is_valid(bitmap: &[u64], idx: usize) -> bool {
    bitmap[idx / 64] & (1u64 << (idx % 64)) != 0
}

/// Set (`valid == true`) or clear the validity bit of row `idx`.
#[inline]
pub(crate) fn bitmap_set_bit(bitmap: &mut [u64], idx: usize, valid: bool) {
    if valid {
        bitmap[idx / 64] |= 1u64 << (idx % 64);
    } else {
        bitmap[idx / 64] &= !(1u64 << (idx % 64));
    }
}

/// Gather the validity bits at `indices` into a new bitmap.
pub(crate) fn gather_bitmap(bitmap: &[u64], indices: &[usize]) -> Vec<u64> {
    let mut out = vec![0u64; indices.len().div_ceil(64)];
    for (j, &i) in indices.iter().enumerate() {
        bitmap_set_bit(&mut out, j, bitmap_is_valid(bitmap, i));
    }
    out
}

/// Declare a typed columnar enum with the shared variant set.
///
/// `$name` is the enum to declare; `$extra` lists additional leading
/// variants (e.g. `BatchColumn::Empty`) that contribute `0` to `len` and
/// `estimated_size`. The generated `len`, `is_empty`, and `estimated_size`
/// are textually identical for both column types, so adding a kind touches
/// this list once instead of drifting across two files.
///
/// The macro expands at the call site, so `Arc`, `Decimal128Value`,
/// `EdgeHeaderColumn`, and `Value` must be in scope where it is invoked
/// (`VertexId` is named by absolute path and needs no import).
macro_rules! column_variants {
    ($name:ident $(, $extra:ident)*) => {
        #[derive(Debug, Clone)]
        pub enum $name {
            $($extra,)*
            I64(Vec<i64>),
            F64(Vec<f64>),
            I32(Vec<i32>),
            Bool(Vec<bool>),
            /// Days since epoch per row (see [`DateValue::to_days`]).
            Date(Vec<i64>),
            /// Micros since epoch per row (see [`DateTimeValue::to_micros`]).
            DateTime(Vec<i64>),
            Utf8(Vec<Arc<str>>),
            /// Decimal128 per row (decimal semantics, `Ord`).
            Decimal(Vec<Decimal128Value>),
            /// I64 column with a validity bitmap (bit set = valid).
            NullableI64(Vec<i64>, Vec<u64>),
            /// F64 column with a validity bitmap.
            NullableF64(Vec<f64>, Vec<u64>),
            /// I32 column with a validity bitmap.
            NullableI32(Vec<i32>, Vec<u64>),
            /// Bool column with a validity bitmap.
            NullableBool(Vec<bool>, Vec<u64>),
            /// Date column with a validity bitmap.
            NullableDate(Vec<i64>, Vec<u64>),
            /// DateTime column with a validity bitmap.
            NullableDateTime(Vec<i64>, Vec<u64>),
            /// Utf8 column with a validity bitmap.
            NullableUtf8(Vec<Arc<str>>, Vec<u64>),
            /// Decimal column with a validity bitmap.
            NullableDecimal(Vec<Decimal128Value>, Vec<u64>),
            /// Vertex identity column: one id per row, never NULL.
            VertexIdentity(Vec<linkrs_core::types::VertexId>),
            /// Edge identity column: struct-of-arrays headers, never NULL.
            EdgeHeader(EdgeHeaderColumn),
            Fallback(Vec<Value>),
        }

        impl $name {
            pub fn len(&self) -> usize {
                match self {
                    $($name::$extra => 0,)*
                    $name::I64(v) => v.len(),
                    $name::F64(v) => v.len(),
                    $name::I32(v) => v.len(),
                    $name::Bool(v) => v.len(),
                    $name::Date(v) => v.len(),
                    $name::DateTime(v) => v.len(),
                    $name::Utf8(v) => v.len(),
                    $name::Decimal(v) => v.len(),
                    $name::NullableI64(v, _) => v.len(),
                    $name::NullableF64(v, _) => v.len(),
                    $name::NullableI32(v, _) => v.len(),
                    $name::NullableBool(v, _) => v.len(),
                    $name::NullableDate(v, _) => v.len(),
                    $name::NullableDateTime(v, _) => v.len(),
                    $name::NullableUtf8(v, _) => v.len(),
                    $name::NullableDecimal(v, _) => v.len(),
                    $name::VertexIdentity(v) => v.len(),
                    $name::EdgeHeader(v) => v.len(),
                    $name::Fallback(v) => v.len(),
                }
            }

            pub fn is_empty(&self) -> bool {
                self.len() == 0
            }

            /// Estimated heap bytes of this column (for memory accounting).
            pub fn estimated_size(&self) -> usize {
                match self {
                    $($name::$extra => 0,)*
                    $name::I64(v) => v.capacity() * std::mem::size_of::<i64>(),
                    $name::F64(v) => v.capacity() * std::mem::size_of::<f64>(),
                    $name::I32(v) => v.capacity() * std::mem::size_of::<i32>(),
                    $name::Bool(v) => v.capacity() * std::mem::size_of::<bool>(),
                    $name::Date(v) => v.capacity() * std::mem::size_of::<i64>(),
                    $name::DateTime(v) => v.capacity() * std::mem::size_of::<i64>(),
                    $name::Utf8(v) => v.iter().map(|s| s.len()).sum(),
                    $name::Decimal(v) => v.capacity() * std::mem::size_of::<Decimal128Value>(),
                    $name::NullableI64(v, b) => {
                        v.capacity() * std::mem::size_of::<i64>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableF64(v, b) => {
                        v.capacity() * std::mem::size_of::<f64>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableI32(v, b) => {
                        v.capacity() * std::mem::size_of::<i32>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableBool(v, b) => {
                        v.capacity() * std::mem::size_of::<bool>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableDate(v, b) => {
                        v.capacity() * std::mem::size_of::<i64>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableDateTime(v, b) => {
                        v.capacity() * std::mem::size_of::<i64>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableUtf8(v, b) => {
                        v.iter().map(|s| s.len()).sum::<usize>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::NullableDecimal(v, b) => {
                        v.capacity() * std::mem::size_of::<Decimal128Value>()
                            + b.capacity() * std::mem::size_of::<u64>()
                    }
                    $name::VertexIdentity(v) => {
                        v.capacity() * std::mem::size_of::<linkrs_core::types::VertexId>()
                    }
                    $name::EdgeHeader(v) => v.estimated_size(),
                    $name::Fallback(v) => v.iter().map(Value::estimated_size).sum(),
                }
            }
        }
    };
}

pub(crate) use column_variants;

/// Gather a `TypedColumn` source at `indices` into the `$out` column type.
///
/// Both column types share variant names, so one match serves the
/// `TypedColumn` gather and the `BatchColumn` gather without duplication.
/// The caller must have `TypedColumn` and `gather_bitmap` in scope.
macro_rules! gather_column {
    ($out:ident, $col:expr, $indices:expr) => {{
        let indices: &[usize] = $indices;
        match $col {
            TypedColumn::I64(v) => $out::I64(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::F64(v) => $out::F64(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::I32(v) => $out::I32(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Bool(v) => $out::Bool(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Date(v) => $out::Date(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::DateTime(v) => $out::DateTime(indices.iter().map(|&i| v[i]).collect()),
            TypedColumn::Utf8(v) => $out::Utf8(indices.iter().map(|&i| v[i].clone()).collect()),
            TypedColumn::Decimal(v) => {
                $out::Decimal(indices.iter().map(|&i| v[i].clone()).collect())
            }
            TypedColumn::NullableI64(v, b) => $out::NullableI64(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableF64(v, b) => $out::NullableF64(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableI32(v, b) => $out::NullableI32(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableBool(v, b) => $out::NullableBool(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableDate(v, b) => $out::NullableDate(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableDateTime(v, b) => $out::NullableDateTime(
                indices.iter().map(|&i| v[i]).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableUtf8(v, b) => $out::NullableUtf8(
                indices.iter().map(|&i| v[i].clone()).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::NullableDecimal(v, b) => $out::NullableDecimal(
                indices.iter().map(|&i| v[i].clone()).collect(),
                gather_bitmap(b, indices),
            ),
            TypedColumn::VertexIdentity(v) => {
                $out::VertexIdentity(indices.iter().map(|&i| v[i]).collect())
            }
            TypedColumn::EdgeHeader(v) => $out::EdgeHeader(v.gather(indices)),
            TypedColumn::Fallback(v) => {
                $out::Fallback(indices.iter().map(|&i| v[i].clone()).collect())
            }
        }
    }};
}

pub(crate) use gather_column;
