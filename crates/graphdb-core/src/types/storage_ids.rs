//! Storage Identifier Types
//!
//! Provides fundamental type aliases and identifier structures shared across
//! storage and transaction modules. This eliminates bidirectional dependencies
//! by centralizing cross-module types.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Add;

use crate::{DataType, Value};

// ============================================================================
// Fundamental Type Aliases
// ============================================================================

/// Timestamp type for MVCC
pub type Timestamp = u64;

/// Invalid timestamp sentinel value (u64::MAX indicates "deleted" or "not set")
///
/// Timestamp allocator invariant: `write_ts` starts at 1 and only grows by 1
/// per allocation, so it can never reach this value in practice
/// (`TimestampExhausted` is returned on `u64` overflow first). No committed or
/// snapshot timestamp may ever equal this sentinel; GC watermarks reuse it as
/// `NO_ACTIVE_SNAPSHOT`, which is safe only because watermarks resolve it to
/// `last_published_commit` before any version comparison.
pub const INVALID_TIMESTAMP: Timestamp = u64::MAX;
/// Maximum valid timestamp value (u64::MAX - 1 used for "latest" queries)
///
/// Reserved alongside `INVALID_TIMESTAMP`: the allocator must stop before
/// either sentinel (see `is_allocatable_timestamp`).
pub const MAX_TIMESTAMP: Timestamp = u64::MAX - 1;

/// Whether `ts` may be handed out by the MVCC timestamp allocator.
///
/// Only values strictly below both sentinels are allocatable. This is the
/// single choke point for the "allocator never touches a sentinel" invariant.
#[inline]
pub fn is_allocatable_timestamp(ts: Timestamp) -> bool {
    ts != INVALID_TIMESTAMP && ts != MAX_TIMESTAMP
}

/// Label ID type for vertex and edge type identification
pub type LabelId = u32;

/// Snapshot handle for MVCC - identifies a consistent snapshot at a specific timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SnapshotHandle {
    /// Timestamp of the snapshot
    pub ts: Timestamp,
    /// Monotonically increasing handle to distinguish concurrent snapshots at the same timestamp
    pub id: u64,
}

impl SnapshotHandle {
    /// Create a new snapshot handle
    #[inline]
    pub fn new(ts: Timestamp, id: u64) -> Self {
        Self { ts, id }
    }
}

// ============================================================================
// EdgeId - Newtype Wrapper
// ============================================================================

/// Edge ID type - unique edge identifier with type safety.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct EdgeId(pub u64);

pub const INVALID_EDGE_ID: EdgeId = EdgeId(u64::MAX);

impl EdgeId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    pub fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }

    /// Increment and return the previous value (for sequential ID generation).
    pub fn fetch_add(&mut self) -> Self {
        let old = *self;
        self.0 += 1;
        old
    }
}

impl From<u64> for EdgeId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<EdgeId> for u64 {
    fn from(id: EdgeId) -> Self {
        id.0
    }
}

impl Add<u64> for EdgeId {
    type Output = Self;
    fn add(self, rhs: u64) -> Self {
        Self(self.0 + rhs)
    }
}

impl fmt::Display for EdgeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

// ============================================================================
// ColumnId - Newtype Wrapper (u32, replaces old i32 alias)
// ============================================================================

/// Column ID type for property columns.
/// Uses u32 (not i32) since negative values have no valid use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ColumnId(pub u32);

impl ColumnId {
    pub const fn new(id: u32) -> Self {
        Self(id)
    }

    pub fn as_u32(self) -> u32 {
        self.0
    }

    pub fn as_usize(self) -> usize {
        self.0 as usize
    }
}

impl From<u32> for ColumnId {
    fn from(id: u32) -> Self {
        Self(id)
    }
}

impl From<ColumnId> for u32 {
    fn from(id: ColumnId) -> Self {
        id.0
    }
}

impl From<ColumnId> for usize {
    fn from(id: ColumnId) -> Self {
        id.0 as usize
    }
}

impl fmt::Display for ColumnId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "col{}", self.0)
    }
}

// ============================================================================
// TransactionId - Newtype Wrapper
// ============================================================================

/// Transaction ID type.
/// Defined once here; do NOT duplicate in other modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct TransactionId(pub u64);

impl TransactionId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub fn as_u64(self) -> u64 {
        self.0
    }

    pub fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    pub fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(u64::from_le_bytes(bytes))
    }
}

impl From<u64> for TransactionId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<TransactionId> for u64 {
    fn from(id: TransactionId) -> Self {
        id.0
    }
}

impl fmt::Display for TransactionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "txn{}", self.0)
    }
}

// ============================================================================
// VertexId - Typed Byte Representation
// ============================================================================

/// Maximum size for VertexId in bytes
/// Supports int64 (8 bytes) and small strings (up to 32 bytes)
pub const VERTEX_ID_MAX_SIZE: usize = 32;

/// Discriminant for the payload carried by a [`VertexId`].
///
/// The kind is the single source of truth for how the stored bytes must be
/// interpreted. Length-based guessing is forbidden: an 8-byte UTF-8 string is
/// text, never an integer, and integer ordering is numeric, never bytewise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VertexIdKind {
    Empty,
    Int,
    Uint,
    Text,
    EdgeEndpoint,
}

impl VertexIdKind {
    pub fn as_u8(self) -> u8 {
        match self {
            VertexIdKind::Empty => 0,
            VertexIdKind::Int => 1,
            VertexIdKind::Uint => 2,
            VertexIdKind::Text => 3,
            VertexIdKind::EdgeEndpoint => 4,
        }
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(VertexIdKind::Empty),
            1 => Some(VertexIdKind::Int),
            2 => Some(VertexIdKind::Uint),
            3 => Some(VertexIdKind::Text),
            4 => Some(VertexIdKind::EdgeEndpoint),
            _ => None,
        }
    }
}

/// Vertex identifier - typed byte representation.
///
/// The kind tag records whether the payload is an integer, an unsigned
/// integer, text, an edge-endpoint key, or empty. All interpretation
/// (ordering, display, conversion) branches on the kind; byte length is
/// never used to infer the type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct VertexId {
    data: [u8; VERTEX_ID_MAX_SIZE],
    len: u8,
    kind: u8,
}

impl<'de> serde::Deserialize<'de> for VertexId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct RawVertexId {
            data: [u8; VERTEX_ID_MAX_SIZE],
            len: u8,
            kind: u8,
        }
        let raw = RawVertexId::deserialize(deserializer)?;
        let kind = VertexIdKind::from_u8(raw.kind).ok_or_else(|| {
            serde::de::Error::invalid_value(
                serde::de::Unexpected::Unsigned(raw.kind as u64),
                &"a vertex id discriminant in 0..=4",
            )
        })?;
        if (raw.len as usize) > VERTEX_ID_MAX_SIZE {
            return Err(serde::de::Error::invalid_length(
                raw.len as usize,
                &"at most 32 payload bytes",
            ));
        }
        Self::from_typed_bytes(kind, &raw.data[..raw.len as usize])
            .map_err(serde::de::Error::custom)
    }
}

impl VertexId {
    pub const fn new() -> Self {
        VertexId {
            data: [0; VERTEX_ID_MAX_SIZE],
            len: 0,
            kind: 0,
        }
    }

    /// Construct a signed integer id, rejecting negatives with an error.
    ///
    /// This is the only way to build an `Int` id: there is no infallible
    /// bypass, so every caller handles the negative case through `Result`.
    pub fn try_from_int64(id: i64) -> Result<Self, crate::StorageError> {
        if id < 0 {
            return Err(crate::StorageError::invalid_input(format!(
                "Vertex id cannot be negative: {}",
                id
            )));
        }
        Self::from_typed_bytes(VertexIdKind::Int, &id.to_be_bytes())
            .map_err(crate::StorageError::invalid_input)
    }

    pub fn from_u64(id: u64) -> Self {
        let bytes = id.to_be_bytes();
        let mut data = [0u8; VERTEX_ID_MAX_SIZE];
        data[..8].copy_from_slice(&bytes);
        VertexId {
            data,
            len: 8,
            kind: VertexIdKind::Uint.as_u8(),
        }
    }

    /// Construct an `Int` id from a `u32` row key.
    ///
    /// Infallible by type: every `u32` is a valid non-negative `Int` id.
    /// This is the only non-`try_` integer constructor, reserved for
    /// storage-internal row keys that are `u32` by construction.
    pub fn from_u32(id: u32) -> Self {
        let bytes = (id as i64).to_be_bytes();
        let mut data = [0u8; VERTEX_ID_MAX_SIZE];
        data[..8].copy_from_slice(&bytes);
        VertexId {
            data,
            len: 8,
            kind: VertexIdKind::Int.as_u8(),
        }
    }

    pub fn kind(&self) -> VertexIdKind {
        VertexIdKind::from_u8(self.kind).unwrap_or(VertexIdKind::Empty)
    }

    /// Decode the 8-byte big-endian payload without checking the kind tag.
    ///
    /// Callers have already matched on the kind; a short payload decodes as
    /// `None` so every user of the bits stays total without panicking.
    pub fn int_bits(&self) -> Option<i64> {
        if self.len != 8 {
            return None;
        }
        let arr: [u8; 8] = self.data[..8].try_into().ok()?;
        Some(i64::from_be_bytes(arr))
    }

    pub fn uint_bits(&self) -> Option<u64> {
        if self.len != 8 {
            return None;
        }
        let arr: [u8; 8] = self.data[..8].try_into().ok()?;
        Some(u64::from_be_bytes(arr))
    }

    pub fn is_empty_id(&self) -> bool {
        self.kind() == VertexIdKind::Empty
    }

    pub fn is_int_id(&self) -> bool {
        self.kind() == VertexIdKind::Int
    }

    pub fn is_uint_id(&self) -> bool {
        self.kind() == VertexIdKind::Uint
    }

    pub fn is_text_id(&self) -> bool {
        self.kind() == VertexIdKind::Text
    }

    pub fn is_edge_endpoint_id(&self) -> bool {
        self.kind() == VertexIdKind::EdgeEndpoint
    }

    /// Rebuild an id from an explicitly typed payload, e.g. after decoding a
    /// persisted index entry. Rejects malformed combinations instead of
    /// padding or truncating them.
    pub fn from_typed_bytes(kind: VertexIdKind, bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > VERTEX_ID_MAX_SIZE {
            return Err(format!(
                "VertexId payload exceeds max length of {} bytes: got {} bytes",
                VERTEX_ID_MAX_SIZE,
                bytes.len()
            ));
        }
        match kind {
            VertexIdKind::Empty => {
                if !bytes.is_empty() {
                    return Err("Empty VertexId must carry zero bytes".to_string());
                }
            }
            VertexIdKind::Int | VertexIdKind::Uint => {
                if bytes.len() != 8 {
                    return Err(format!(
                        "Integer VertexId must carry exactly 8 bytes: got {} bytes",
                        bytes.len()
                    ));
                }
            }
            VertexIdKind::EdgeEndpoint => {
                if bytes.len() != 16 {
                    return Err(format!(
                        "Edge endpoint key must carry exactly 16 bytes: got {} bytes",
                        bytes.len()
                    ));
                }
            }
            VertexIdKind::Text => {}
        }
        let mut data = [0u8; VERTEX_ID_MAX_SIZE];
        data[..bytes.len()].copy_from_slice(bytes);
        Ok(VertexId {
            data,
            len: bytes.len() as u8,
            kind: kind.as_u8(),
        })
    }

    /// Create from a string, returning an error if the string exceeds max size.
    ///
    /// This is the only way to build a `Text` id: overlong input is an error,
    /// never a truncation.
    pub fn try_from_string(s: impl AsRef<str>) -> Result<Self, String> {
        let bytes = s.as_ref().as_bytes();
        if bytes.len() > VERTEX_ID_MAX_SIZE {
            return Err(format!(
                "VertexId string exceeds max length of {} bytes: got {} bytes",
                VERTEX_ID_MAX_SIZE,
                bytes.len()
            ));
        }
        let len = bytes.len();
        let mut data = [0u8; VERTEX_ID_MAX_SIZE];
        data[..len].copy_from_slice(bytes);
        Ok(VertexId {
            data,
            len: len as u8,
            kind: if len == 0 {
                VertexIdKind::Empty.as_u8()
            } else {
                VertexIdKind::Text.as_u8()
            },
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }

    pub fn as_int64(&self) -> Option<i64> {
        if self.kind() != VertexIdKind::Int {
            return None;
        }
        let arr: [u8; 8] = self.data[..8].try_into().ok()?;
        Some(i64::from_be_bytes(arr))
    }

    pub fn as_u64(&self) -> Option<u64> {
        let arr: [u8; 8] = self.data[..8].try_into().ok()?;
        match self.kind() {
            VertexIdKind::Uint => Some(u64::from_be_bytes(arr)),
            VertexIdKind::Int => {
                let value = i64::from_be_bytes(arr);
                u64::try_from(value).ok()
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        if self.kind() != VertexIdKind::Text {
            return None;
        }
        std::str::from_utf8(self.as_bytes()).ok()
    }

    pub fn is_int64(&self) -> bool {
        self.kind() == VertexIdKind::Int
    }

    /// Project an integer id to a storage-internal `u32` row key.
    ///
    /// Returns `None` for non-integer ids, negatives, and out-of-range
    /// values instead of collapsing them to row zero.
    pub fn as_internal_u32(&self) -> Option<u32> {
        match self.kind() {
            VertexIdKind::Int => u32::try_from(self.as_int64()?).ok(),
            VertexIdKind::Uint => u32::try_from(self.as_u64()?).ok(),
            _ => None,
        }
    }

    /// Normalize an external id against the owning space's `vid_type`.
    ///
    /// This is the only place where a numeric text id may become an integer
    /// id (or vice versa). Core constructors never convert silently; every
    /// other path keeps the kind it was given. The mapping is idempotent.
    pub fn normalize_for_vid_type(
        vid_type: &DataType,
        vid: VertexId,
    ) -> Result<VertexId, crate::StorageError> {
        let invalid = |detail: String| crate::StorageError::invalid_input(detail);
        match vid_type {
            DataType::SmallInt | DataType::Int | DataType::BigInt => match vid.kind() {
                VertexIdKind::Int => vid
                    .int_bits()
                    .ok_or_else(|| invalid("Malformed integer vertex id".to_string()))
                    .and_then(VertexId::try_from_int64),
                VertexIdKind::Uint => {
                    let value = vid
                        .uint_bits()
                        .ok_or_else(|| invalid("Malformed unsigned vertex id".to_string()))?;
                    i64::try_from(value)
                        .map_err(|_| invalid(format!("Vertex id {} overflows INT64 space", value)))
                        .and_then(VertexId::try_from_int64)
                }
                VertexIdKind::Text => {
                    let text = vid
                        .as_str()
                        .ok_or_else(|| invalid("Malformed text vertex id".to_string()))?;
                    text.parse::<i64>()
                        .map_err(|_| {
                            invalid(format!(
                                "Vertex id {:?} is not a valid integer for INT64 space",
                                text
                            ))
                        })
                        .and_then(VertexId::try_from_int64)
                }
                VertexIdKind::Empty | VertexIdKind::EdgeEndpoint => Err(invalid(
                    "Empty vertex id is not valid for INT64 space".to_string(),
                )),
            },
            DataType::String | DataType::FixedString(_) => {
                let text = match vid.kind() {
                    VertexIdKind::Text => vid
                        .as_str()
                        .ok_or_else(|| invalid("Malformed text vertex id".to_string()))?
                        .to_string(),
                    VertexIdKind::Int => vid
                        .int_bits()
                        .ok_or_else(|| invalid("Malformed integer vertex id".to_string()))?
                        .to_string(),
                    VertexIdKind::Uint => vid
                        .uint_bits()
                        .ok_or_else(|| invalid("Malformed unsigned vertex id".to_string()))?
                        .to_string(),
                    VertexIdKind::Empty | VertexIdKind::EdgeEndpoint => {
                        return Err(invalid(
                            "Empty vertex id is not valid for STRING space".to_string(),
                        ));
                    }
                };
                if let DataType::FixedString(width) = vid_type {
                    if text.len() > *width {
                        return Err(invalid(format!(
                            "Vertex id exceeds FIXEDSTRING({}) width: {} bytes",
                            width,
                            text.len()
                        )));
                    }
                }
                Self::try_from_string(&text).map_err(invalid)
            }
            _ => Ok(vid),
        }
    }

    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }

    pub fn as_usize(&self) -> Option<usize> {
        self.as_int64().map(|v| v as usize)
    }

    pub const fn const_default() -> Self {
        Self::new()
    }

    /// Encode an edge endpoint as `(endpoint: u32, rank: i64)` into a 16-byte key.
    ///
    /// Format: `[endpoint as i64 big-endian 8 bytes][rank big-endian 8 bytes]`.
    /// This is the canonical encoding used by the CSR layer to store neighbor keys.
    /// The key carries the edge-endpoint kind so generic id paths (ordering,
    /// display, int/text projection) never mistake it for a vertex id.
    pub fn edge_endpoint_key(endpoint: u32, rank: i64) -> Self {
        let mut data = [0u8; VERTEX_ID_MAX_SIZE];
        data[..8].copy_from_slice(&(endpoint as i64).to_be_bytes());
        data[8..16].copy_from_slice(&rank.to_be_bytes());
        VertexId {
            data,
            len: 16,
            kind: VertexIdKind::EdgeEndpoint.as_u8(),
        }
    }

    /// Decode an edge endpoint key back to `(endpoint_vertex_id, rank)`.
    ///
    /// Returns `None` unless the id is a well-formed 16-byte edge-endpoint
    /// key. Short, long, or non-endpoint ids are rejected instead of being
    /// zero-padded into a bogus endpoint.
    pub fn try_decode_edge_endpoint(&self) -> Option<(Self, i64)> {
        if self.kind() != VertexIdKind::EdgeEndpoint || self.len != 16 {
            return None;
        }
        let bytes = self.as_bytes();
        let mut endpoint_bytes = [0u8; 8];
        endpoint_bytes.copy_from_slice(&bytes[..8]);
        let mut rank_bytes = [0u8; 8];
        rank_bytes.copy_from_slice(&bytes[8..16]);
        let endpoint = VertexId::try_from_int64(i64::from_be_bytes(endpoint_bytes)).ok()?;
        Some((endpoint, i64::from_be_bytes(rank_bytes)))
    }
}

impl fmt::Display for VertexId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.kind() {
            VertexIdKind::Int => match self.int_bits() {
                Some(v) => write!(f, "{}", v),
                None => write!(f, "{:?}", self.as_bytes()),
            },
            VertexIdKind::Uint => match self.uint_bits() {
                Some(v) => write!(f, "{}", v),
                None => write!(f, "{:?}", self.as_bytes()),
            },
            VertexIdKind::Text => match self.as_str() {
                Some(s) => write!(f, "\"{}\"", s),
                None => write!(f, "{:?}", self.as_bytes()),
            },
            VertexIdKind::EdgeEndpoint => match self.try_decode_edge_endpoint() {
                Some((endpoint, rank)) => match endpoint.int_bits() {
                    Some(v) => write!(f, "edge_endpoint({}, {})", v, rank),
                    None => write!(f, "{:?}", self.as_bytes()),
                },
                None => write!(f, "{:?}", self.as_bytes()),
            },
            VertexIdKind::Empty => write!(f, "\"\""),
        }
    }
}

impl Default for VertexId {
    fn default() -> Self {
        Self::new()
    }
}

impl AsRef<[u8]> for VertexId {
    fn as_ref(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl VertexId {
    /// Add a `u64` offset, returning `None` for non-integer vertex IDs and
    /// on arithmetic overflow.
    pub fn checked_add(self, rhs: u64) -> Option<Self> {
        match self.kind() {
            VertexIdKind::Int => {
                let id = self.int_bits()?;
                id.checked_add_unsigned(rhs)
                    .and_then(|next| Self::try_from_int64(next).ok())
            }
            VertexIdKind::Uint => {
                let id = self.uint_bits()?;
                id.checked_add(rhs).map(Self::from_u64)
            }
            _ => None,
        }
    }
}

impl TryFrom<&Value> for VertexId {
    type Error = crate::StorageError;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        match value {
            Value::SmallInt(i) => Self::try_from_int64(*i as i64),
            Value::Int(i) => Self::try_from_int64(*i as i64),
            Value::BigInt(i) => Self::try_from_int64(*i),
            // Numeric text stays text here. Coercion to integer happens only
            // through normalize_for_vid_type at storage write/read entries,
            // where the owning space's vid_type authorizes it.
            Value::String(s) => {
                Self::try_from_string(s.as_str()).map_err(crate::StorageError::invalid_input)
            }
            Value::FixedString(s) => {
                Self::try_from_string(s).map_err(crate::StorageError::invalid_input)
            }
            Value::Vertex(v) => Ok(v.vid),
            Value::VertexId(vid) => Ok(*vid),
            _ => Err(crate::StorageError::invalid_input(
                "Cannot convert Value to VertexId",
            )),
        }
    }
}

impl From<u64> for VertexId {
    fn from(id: u64) -> Self {
        Self::from_u64(id)
    }
}

impl From<VertexId> for Value {
    fn from(vid: VertexId) -> Self {
        match vid.kind() {
            VertexIdKind::Int => match vid.int_bits() {
                Some(v) => Value::BigInt(v),
                None => Value::Blob(vid.into_inner()),
            },
            VertexIdKind::Uint => match vid.uint_bits() {
                Some(value) => i64::try_from(value)
                    .map(Value::BigInt)
                    .unwrap_or_else(|_| Value::Blob(vid.into_inner())),
                None => Value::Blob(vid.into_inner()),
            },
            VertexIdKind::Text => match vid.as_str() {
                Some(s) => Value::string(s),
                None => Value::Blob(vid.into_inner()),
            },
            VertexIdKind::EdgeEndpoint | VertexIdKind::Empty => Value::Blob(vid.into_inner()),
        }
    }
}

impl Ord for VertexId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self.kind(), other.kind()) {
            (VertexIdKind::Empty, VertexIdKind::Empty) => std::cmp::Ordering::Equal,
            (VertexIdKind::Int, VertexIdKind::Int) => match (self.int_bits(), other.int_bits()) {
                (Some(left), Some(right)) => left.cmp(&right),
                _ => self.as_bytes().cmp(other.as_bytes()),
            },
            (VertexIdKind::Uint, VertexIdKind::Uint) => {
                match (self.uint_bits(), other.uint_bits()) {
                    (Some(left), Some(right)) => left.cmp(&right),
                    _ => self.as_bytes().cmp(other.as_bytes()),
                }
            }
            (VertexIdKind::Int, VertexIdKind::Uint) => match (self.int_bits(), other.uint_bits()) {
                (Some(left), Some(right)) => (left as i128).cmp(&(right as i128)),
                _ => self.as_bytes().cmp(other.as_bytes()),
            },
            (VertexIdKind::Uint, VertexIdKind::Int) => match (self.uint_bits(), other.int_bits()) {
                (Some(left), Some(right)) => (left as i128).cmp(&(right as i128)),
                _ => self.as_bytes().cmp(other.as_bytes()),
            },
            (VertexIdKind::Text, VertexIdKind::Text) => self.as_bytes().cmp(other.as_bytes()),
            (VertexIdKind::EdgeEndpoint, VertexIdKind::EdgeEndpoint) => {
                self.as_bytes().cmp(other.as_bytes())
            }
            (left, right) => (left.as_u8()).cmp(&right.as_u8()),
        }
    }
}

impl PartialOrd for VertexId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// ============================================================================
// Edge Key and Identifier Types
// ============================================================================

/// Edge key for identifying an edge type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeKey {
    pub src_label: LabelId,
    pub dst_label: LabelId,
    pub edge_label: LabelId,
}

impl EdgeKey {
    pub fn new(src_label: LabelId, dst_label: LabelId, edge_label: LabelId) -> Self {
        Self {
            src_label,
            dst_label,
            edge_label,
        }
    }
}

/// Edge identifier for fully identifying an edge instance
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeIdentifier {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
}

impl EdgeIdentifier {
    pub fn new(
        src_label: LabelId,
        src_vid: VertexId,
        dst_label: LabelId,
        dst_vid: VertexId,
        edge_label: LabelId,
        rank: i64,
    ) -> Self {
        Self {
            src_label,
            src_vid,
            dst_label,
            dst_vid,
            edge_label,
            rank,
        }
    }
}

/// Edge operation context containing all necessary information for edge operations
#[derive(Debug, Clone)]
pub struct EdgeOperationContext {
    pub edge_key: EdgeKey,
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub rank: i64,
    pub timestamp: Timestamp,
}

impl EdgeOperationContext {
    pub fn new(
        src_label: LabelId,
        dst_label: LabelId,
        edge_label: LabelId,
        src_vid: VertexId,
        dst_vid: VertexId,
        rank: i64,
        timestamp: Timestamp,
    ) -> Self {
        Self {
            edge_key: EdgeKey::new(src_label, dst_label, edge_label),
            src_vid,
            dst_vid,
            rank,
            timestamp,
        }
    }
}

/// Vertex identifier for identifying a vertex
#[derive(Debug, Clone)]
pub struct VertexIdentifier {
    pub label: LabelId,
    pub vid: VertexId,
}

impl VertexIdentifier {
    pub fn new(label: LabelId, vid: VertexId) -> Self {
        Self { label, vid }
    }
}

/// Edge property update context
#[derive(Debug, Clone)]
pub struct EdgePropertyUpdateContext {
    pub edge_id: EdgeIdentifier,
    pub property_name: String,
    pub timestamp: Timestamp,
}

impl EdgePropertyUpdateContext {
    pub fn new(edge_id: EdgeIdentifier, property_name: String, timestamp: Timestamp) -> Self {
        Self {
            edge_id,
            property_name,
            timestamp,
        }
    }
}

/// Edge deletion context keyed by edge identifier
#[derive(Debug, Clone)]
pub struct EdgeDeletionContext {
    pub edge_id: EdgeIdentifier,
    pub timestamp: Timestamp,
}

/// Parameters for creating EdgeDeletionContext
pub struct EdgeDeletionContextParams {
    pub src_label: LabelId,
    pub src_vid: VertexId,
    pub dst_label: LabelId,
    pub dst_vid: VertexId,
    pub edge_label: LabelId,
    pub rank: i64,
    pub timestamp: Timestamp,
}

impl EdgeDeletionContext {
    pub fn new(params: EdgeDeletionContextParams) -> Self {
        Self {
            edge_id: EdgeIdentifier::new(
                params.src_label,
                params.src_vid,
                params.dst_label,
                params.dst_vid,
                params.edge_label,
                params.rank,
            ),
            timestamp: params.timestamp,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_add_works_for_integer_ids() {
        let base = VertexId::try_from_int64(41).expect("valid test id");
        let next = VertexId::try_from_int64(42).expect("valid test id");
        assert_eq!(base.checked_add(1), Some(next));
        assert_eq!(
            VertexId::from_u64(u64::MAX - 1).checked_add(1),
            Some(VertexId::from_u64(u64::MAX))
        );
    }

    #[test]
    fn checked_add_returns_none_for_non_integer_ids() {
        // "abcdefghi" is 9 bytes, so it is not treated as an int64 ID.
        let text = VertexId::try_from_string("abcdefghi").expect("valid test id");
        assert_eq!(text.checked_add(1), None);
    }

    #[test]
    fn checked_add_preserves_empty_vertex_id() {
        // Empty IDs are neither integer nor string; must return None.
        assert_eq!(VertexId::new().checked_add(1), None);
    }

    #[test]
    fn eight_byte_text_is_not_an_integer() {
        let text = VertexId::try_from_string("12345678").expect("valid test id");
        let int_form = VertexId::try_from_int64(0x3132333435363738).expect("valid test id");
        assert_eq!(text.kind(), VertexIdKind::Text);
        assert_eq!(text.as_int64(), None);
        assert_eq!(text.as_str(), Some("12345678"));
        assert_eq!(text.to_string(), "\"12345678\"");
        assert_ne!(text, int_form);
    }

    #[test]
    fn integer_ordering_is_numeric_and_grouped_by_kind() {
        let neg_five = VertexId::from_typed_bytes(VertexIdKind::Int, &(-5i64).to_be_bytes())
            .expect("explicit typed bytes can express Int -5");
        let seven = VertexId::try_from_int64(7).expect("valid test id");
        let two = VertexId::try_from_int64(2).expect("valid test id");
        let max = VertexId::try_from_int64(i64::MAX).expect("valid test id");
        let text_a = VertexId::try_from_string("a").expect("valid test id");
        let zero = VertexId::try_from_int64(0).expect("valid test id");
        assert!(neg_five < seven);
        assert!(two < VertexId::from_u64(3));
        assert!(VertexId::from_u64(u64::MAX) > max);
        assert!(max < text_a);
        assert!(VertexId::new() < zero);
    }

    #[test]
    fn large_uint_keeps_sign_in_display_and_value() {
        let big = VertexId::from_u64(u64::MAX);
        assert_eq!(big.kind(), VertexIdKind::Uint);
        assert_eq!(big.to_string(), u64::MAX.to_string());
        assert_eq!(big.as_int64(), None);
        assert_eq!(Value::from(big), Value::Blob(big.into_inner()));
        let fits = VertexId::from_u64(42);
        assert_eq!(Value::from(fits), Value::BigInt(42));
    }

    #[test]
    fn user_facing_constructors_reject_bad_input() {
        assert!(VertexId::try_from_int64(-1).is_err());
        assert!(VertexId::try_from_string("x".repeat(33)).is_err());
        let from_value = VertexId::try_from(&Value::string("101")).expect("text stays text");
        assert_eq!(from_value.kind(), VertexIdKind::Text);
        assert!(VertexId::try_from(&Value::BigInt(-3)).is_err());
    }

    #[test]
    fn vid_type_normalization_is_explicit_and_idempotent() {
        let int_space = DataType::BigInt;
        let text_101 = VertexId::try_from_string("101").expect("valid test id");
        let normalized = VertexId::normalize_for_vid_type(&int_space, text_101)
            .expect("numeric text normalizes in INT space");
        let int_101 = VertexId::try_from_int64(101).expect("valid test id");
        assert_eq!(normalized, int_101);
        let text_abc = VertexId::try_from_string("abc").expect("valid test id");
        assert!(VertexId::normalize_for_vid_type(&int_space, text_abc).is_err());

        let str_space = DataType::String;
        let int_7 = VertexId::try_from_int64(7).expect("valid test id");
        let text = VertexId::normalize_for_vid_type(&str_space, int_7)
            .expect("int normalizes in STRING space");
        let text_7 = VertexId::try_from_string("7").expect("valid test id");
        assert_eq!(text, text_7);

        let again = VertexId::normalize_for_vid_type(&int_space, normalized).expect("idempotent");
        assert_eq!(again, normalized);
    }

    #[test]
    fn edge_endpoint_decode_rejects_malformed_keys() {
        let key = VertexId::edge_endpoint_key(9, 3);
        assert_eq!(key.kind(), VertexIdKind::EdgeEndpoint);
        let (endpoint, rank) = key.try_decode_edge_endpoint().expect("valid key decodes");
        let int_9 = VertexId::try_from_int64(9).expect("valid test id");
        assert_eq!(endpoint, int_9);
        assert_eq!(rank, 3);
        assert!(int_9.try_decode_edge_endpoint().is_none());
        let short = VertexId::try_from_string("short").expect("valid test id");
        assert!(short.try_decode_edge_endpoint().is_none());
        // Endpoint keys never project as vertex ids.
        assert_eq!(key.as_int64(), None);
        assert_eq!(key.as_str(), None);
        assert_eq!(key.as_internal_u32(), None);
    }
}
