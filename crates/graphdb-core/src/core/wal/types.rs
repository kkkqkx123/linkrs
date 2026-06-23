//! WAL Types
//!
//! Type definitions for Write-Ahead Log

use std::fmt;
use std::time::Duration;

use crc32fast::Hasher;
use serde::{Deserialize, Serialize};

use crate::core::types::Timestamp;

pub const WAL_MAGIC: u32 = 0x47524150;
pub const WAL_VERSION: u32 = 2;
pub const WAL_FILE_HEADER_SIZE: usize = 64;
pub const WAL_HEADER_SIZE: usize = 40;
pub const WAL_BLOCK_SIZE: usize = 32 * 1024;
pub const WAL_MAX_RECORD_SIZE: usize = WAL_BLOCK_SIZE - WAL_HEADER_SIZE;

pub fn block_padding_needed(current_offset: usize) -> usize {
    let remainder = current_offset % WAL_BLOCK_SIZE;
    if remainder == 0 {
        0
    } else {
        WAL_BLOCK_SIZE - remainder
    }
}

pub fn is_block_aligned(offset: usize) -> bool {
    offset.is_multiple_of(WAL_BLOCK_SIZE)
}

pub fn align_to_block(offset: usize) -> usize {
    offset.div_ceil(WAL_BLOCK_SIZE) * WAL_BLOCK_SIZE
}

pub fn blocks_needed(size: usize) -> usize {
    size.div_ceil(WAL_BLOCK_SIZE)
}

// TransactionId is defined in core::types::storage_ids — use that definition.
pub use crate::core::types::TransactionId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Lsn(pub u64);

impl Lsn {
    pub const ZERO: Lsn = Lsn(0);
    pub const MAX: Lsn = Lsn(u64::MAX);

    pub fn new(value: u64) -> Self {
        Self(value)
    }

    pub fn increment(&mut self, bytes: u64) -> Self {
        let old = *self;
        self.0 += bytes;
        old
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }

    pub fn offset_in_file(&self, file_start_lsn: Lsn) -> u64 {
        self.0.saturating_sub(file_start_lsn.0)
    }
}

impl Default for Lsn {
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Display for Lsn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LSN({:#018x})", self.0)
    }
}

impl From<u64> for Lsn {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl From<Lsn> for u64 {
    fn from(lsn: Lsn) -> Self {
        lsn.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WalOpType {
    InsertVertex = 0,
    InsertEdge = 1,
    CreateVertexType = 2,
    CreateEdgeType = 3,
    AddVertexProp = 4,
    AddEdgeProp = 5,
    UpdateVertexProp = 6,
    UpdateEdgeProp = 7,
    DeleteVertex = 8,
    DeleteEdge = 9,
    DeleteVertexType = 10,
    DeleteEdgeType = 11,
    DeleteVertexProp = 12,
    DeleteEdgeProp = 13,
    RenameVertexProp = 14,
    RenameEdgeProp = 15,
    Compact = 16,
    CreateSpace = 17,
    DropSpace = 18,
    ClearSpace = 19,
    AlterSpaceComment = 20,
}

impl TryFrom<u8> for WalOpType {
    type Error = WalError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(WalOpType::InsertVertex),
            1 => Ok(WalOpType::InsertEdge),
            2 => Ok(WalOpType::CreateVertexType),
            3 => Ok(WalOpType::CreateEdgeType),
            4 => Ok(WalOpType::AddVertexProp),
            5 => Ok(WalOpType::AddEdgeProp),
            6 => Ok(WalOpType::UpdateVertexProp),
            7 => Ok(WalOpType::UpdateEdgeProp),
            8 => Ok(WalOpType::DeleteVertex),
            9 => Ok(WalOpType::DeleteEdge),
            10 => Ok(WalOpType::DeleteVertexType),
            11 => Ok(WalOpType::DeleteEdgeType),
            12 => Ok(WalOpType::DeleteVertexProp),
            13 => Ok(WalOpType::DeleteEdgeProp),
            14 => Ok(WalOpType::RenameVertexProp),
            15 => Ok(WalOpType::RenameEdgeProp),
            16 => Ok(WalOpType::Compact),
            17 => Ok(WalOpType::CreateSpace),
            18 => Ok(WalOpType::DropSpace),
            19 => Ok(WalOpType::ClearSpace),
            20 => Ok(WalOpType::AlterSpaceComment),
            _ => Err(WalError::InvalidOpType(value)),
        }
    }
}

impl fmt::Display for WalOpType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WalOpType::InsertVertex => write!(f, "InsertVertex"),
            WalOpType::InsertEdge => write!(f, "InsertEdge"),
            WalOpType::CreateVertexType => write!(f, "CreateVertexType"),
            WalOpType::CreateEdgeType => write!(f, "CreateEdgeType"),
            WalOpType::AddVertexProp => write!(f, "AddVertexProp"),
            WalOpType::AddEdgeProp => write!(f, "AddEdgeProp"),
            WalOpType::UpdateVertexProp => write!(f, "UpdateVertexProp"),
            WalOpType::UpdateEdgeProp => write!(f, "UpdateEdgeProp"),
            WalOpType::DeleteVertex => write!(f, "DeleteVertex"),
            WalOpType::DeleteEdge => write!(f, "DeleteEdge"),
            WalOpType::DeleteVertexType => write!(f, "DeleteVertexType"),
            WalOpType::DeleteEdgeType => write!(f, "DeleteEdgeType"),
            WalOpType::DeleteVertexProp => write!(f, "DeleteVertexProp"),
            WalOpType::DeleteEdgeProp => write!(f, "DeleteEdgeProp"),
            WalOpType::RenameVertexProp => write!(f, "RenameVertexProp"),
            WalOpType::RenameEdgeProp => write!(f, "RenameEdgeProp"),
            WalOpType::Compact => write!(f, "Compact"),
            WalOpType::CreateSpace => write!(f, "CreateSpace"),
            WalOpType::DropSpace => write!(f, "DropSpace"),
            WalOpType::ClearSpace => write!(f, "ClearSpace"),
            WalOpType::AlterSpaceComment => write!(f, "AlterSpaceComment"),
        }
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WalFileHeader {
    pub magic: u32,
    pub version: u32,
    pub checkpoint_seq: u64,
    pub start_lsn: u64,
    pub salt1: u32,
    pub salt2: u32,
    pub created_at: u64,
    pub thread_id: u32,
    pub reserved: [u8; 20],
}

impl WalFileHeader {
    pub const SIZE: usize = WAL_FILE_HEADER_SIZE;

    pub fn new(thread_id: u32, checkpoint_seq: u64, start_lsn: Lsn) -> Self {
        use rand::Rng;
        let mut rng = rand::thread_rng();

        Self {
            magic: WAL_MAGIC,
            version: WAL_VERSION,
            checkpoint_seq,
            start_lsn: start_lsn.0,
            salt1: rng.gen(),
            salt2: rng.gen(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            thread_id,
            reserved: [0; 20],
        }
    }

    /// Serialize header to fixed-size byte array (safe, stack-allocated)
    pub fn as_bytes_array(&self) -> [u8; 64] {
        let mut bytes = [0u8; 64];
        let mut offset = 0;

        bytes[offset..offset+4].copy_from_slice(&self.magic.to_le_bytes());
        offset += 4;

        bytes[offset..offset+4].copy_from_slice(&self.version.to_le_bytes());
        offset += 4;

        bytes[offset..offset+8].copy_from_slice(&self.checkpoint_seq.to_le_bytes());
        offset += 8;

        bytes[offset..offset+8].copy_from_slice(&self.start_lsn.to_le_bytes());
        offset += 8;

        bytes[offset..offset+4].copy_from_slice(&self.salt1.to_le_bytes());
        offset += 4;

        bytes[offset..offset+4].copy_from_slice(&self.salt2.to_le_bytes());
        offset += 4;

        bytes[offset..offset+8].copy_from_slice(&self.created_at.to_le_bytes());
        offset += 8;

        bytes[offset..offset+4].copy_from_slice(&self.thread_id.to_le_bytes());
        offset += 4;

        bytes[offset..offset+20].copy_from_slice(&self.reserved);

        bytes
    }

    /// Serialize header to byte vector (safe, heap-allocated when needed)
    pub fn as_bytes_safe(&self) -> Vec<u8> {
        self.as_bytes_array().to_vec()
    }

    /// Deserialize from byte slice (safe implementation)
    pub fn from_bytes_safe(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }

        let mut offset = 0;

        let magic = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let version = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let checkpoint_seq = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
        offset += 8;

        let start_lsn = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
        offset += 8;

        let salt1 = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let salt2 = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let created_at = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
        offset += 8;

        let thread_id = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let mut reserved = [0u8; 20];
        reserved.copy_from_slice(&bytes[offset..offset+20]);

        Some(Self {
            magic,
            version,
            checkpoint_seq,
            start_lsn,
            salt1,
            salt2,
            created_at,
            thread_id,
            reserved,
        })
    }


    /// Get bytes reference (unsafe but documented)
    ///
    /// # Safety
    /// This is safe because WalFileHeader is repr(C) with fixed-size fields.
    /// Use as_bytes_safe() for a guaranteed safe alternative.
    #[deprecated(since = "0.2.0", note = "Use as_bytes_safe() instead")]
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const WalFileHeader as *const u8,
                std::mem::size_of::<WalFileHeader>(),
            )
        }
    }

    /// Deserialize from bytes (unsafe but documented)
    ///
    /// # Safety
    /// This is safe only if the input comes from a properly serialized WalFileHeader.
    /// Use from_bytes_safe() for a guaranteed safe alternative.
    #[deprecated(since = "0.2.0", note = "Use from_bytes_safe() instead")]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        let header: WalFileHeader =
            unsafe { std::ptr::read(bytes.as_ptr() as *const WalFileHeader) };
        Some(header)
    }

    pub fn is_valid(&self) -> bool {
        self.magic == WAL_MAGIC
    }

    pub fn salts(&self) -> (u32, u32) {
        (self.salt1, self.salt2)
    }

    pub fn start_lsn(&self) -> Lsn {
        Lsn(self.start_lsn)
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct WalHeader {
    pub length: u32,
    pub op_type: u8,
    pub is_update: bool,
    pub record_type: RecordType,
    pub flags: u16,
    pub timestamp: Timestamp,
    pub lsn: u64,
    pub prev_lsn: u64,
    pub checksum: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum RecordType {
    #[default]
    Full = 0,
    First = 1,
    Middle = 2,
    Last = 3,
}

impl RecordType {
    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => RecordType::Full,
            1 => RecordType::First,
            2 => RecordType::Middle,
            3 => RecordType::Last,
            _ => RecordType::Full,
        }
    }
}

impl WalHeader {
    pub const SIZE: usize = WAL_HEADER_SIZE;

    pub fn new(op_type: WalOpType, timestamp: Timestamp, length: u32) -> Self {
        let is_update = matches!(
            op_type,
            WalOpType::UpdateVertexProp
                | WalOpType::UpdateEdgeProp
                | WalOpType::DeleteVertex
                | WalOpType::DeleteEdge
                | WalOpType::DeleteVertexType
                | WalOpType::DeleteEdgeType
                | WalOpType::DeleteVertexProp
                | WalOpType::DeleteEdgeProp
                | WalOpType::RenameVertexProp
                | WalOpType::RenameEdgeProp
                | WalOpType::Compact
        );

        Self {
            length,
            op_type: op_type as u8,
            is_update,
            record_type: RecordType::Full,
            flags: 0,
            timestamp,
            lsn: 0,
            prev_lsn: 0,
            checksum: 0,
        }
    }

    pub fn with_lsn(mut self, lsn: Lsn, prev_lsn: Lsn) -> Self {
        self.lsn = lsn.0;
        self.prev_lsn = prev_lsn.0;
        self
    }

    pub fn with_record_type(mut self, record_type: RecordType) -> Self {
        self.record_type = record_type;
        self
    }

    pub fn with_checksum(mut self, payload: &[u8]) -> Self {
        let mut hasher = Hasher::new();
        hasher.update(&self.length.to_le_bytes());
        hasher.update(&[self.op_type, self.is_update as u8, self.record_type as u8]);
        hasher.update(&self.flags.to_le_bytes());
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.update(&self.lsn.to_le_bytes());
        hasher.update(&self.prev_lsn.to_le_bytes());
        hasher.update(payload);
        self.checksum = hasher.finalize();
        self
    }

    pub fn verify_checksum(&self, payload: &[u8]) -> bool {
        let mut hasher = Hasher::new();
        hasher.update(&self.length.to_le_bytes());
        hasher.update(&[self.op_type, self.is_update as u8, self.record_type as u8]);
        hasher.update(&self.flags.to_le_bytes());
        hasher.update(&self.timestamp.to_le_bytes());
        hasher.update(&self.lsn.to_le_bytes());
        hasher.update(&self.prev_lsn.to_le_bytes());
        hasher.update(payload);
        hasher.finalize() == self.checksum
    }

    pub fn with_compression(mut self, compression: WalCompression) -> Self {
        self.flags = (self.flags & !wal_flags::COMPRESSION_MASK) | (compression.flag_byte() as u16);
        self
    }

    pub fn compression(&self) -> WalCompression {
        WalCompression::from_flag_byte((self.flags & wal_flags::COMPRESSION_MASK) as u8)
    }

    pub fn is_compressed(&self) -> bool {
        self.compression() != WalCompression::None
    }

    pub fn lsn(&self) -> Lsn {
        Lsn(self.lsn)
    }

    pub fn prev_lsn(&self) -> Lsn {
        Lsn(self.prev_lsn)
    }

    pub fn is_fragmented(&self) -> bool {
        self.record_type != RecordType::Full
    }

    /// Serialize header to fixed-size byte array (safe, stack-allocated)
    pub fn as_bytes_array(&self) -> [u8; 40] {
        let mut bytes = [0u8; 40];
        let mut offset = 0;

        bytes[offset..offset+4].copy_from_slice(&self.length.to_le_bytes());
        offset += 4;

        bytes[offset] = self.op_type;
        offset += 1;

        bytes[offset] = self.is_update as u8;
        offset += 1;

        bytes[offset] = self.record_type as u8;
        offset += 1;

        bytes[offset] = 0; // padding for alignment
        offset += 1;

        bytes[offset..offset+2].copy_from_slice(&self.flags.to_le_bytes());
        offset += 2;

        // timestamp is u32, not u64
        bytes[offset..offset+4].copy_from_slice(&self.timestamp.to_le_bytes());
        offset += 4;

        bytes[offset..offset+8].copy_from_slice(&self.lsn.to_le_bytes());
        offset += 8;

        bytes[offset..offset+8].copy_from_slice(&self.prev_lsn.to_le_bytes());
        offset += 8;

        bytes[offset..offset+4].copy_from_slice(&self.checksum.to_le_bytes());

        bytes
    }

    /// Serialize header to byte vector (safe, heap-allocated when needed)
    pub fn as_bytes_safe(&self) -> Vec<u8> {
        self.as_bytes_array().to_vec()
    }

    /// Deserialize from byte slice (safe implementation)
    pub fn from_bytes_safe(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }

        let mut offset = 0;

        let length = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let op_type = bytes[offset];
        offset += 1;

        let is_update = bytes[offset] != 0;
        offset += 1;

        let record_type = RecordType::from_u8(bytes[offset]);
        offset += 1;

        // Skip padding
        offset += 1;

        let flags = u16::from_le_bytes(bytes[offset..offset+2].try_into().ok()?);
        offset += 2;

        // timestamp is u32, not u64
        let timestamp = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);
        offset += 4;

        let lsn = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
        offset += 8;

        let prev_lsn = u64::from_le_bytes(bytes[offset..offset+8].try_into().ok()?);
        offset += 8;

        let checksum = u32::from_le_bytes(bytes[offset..offset+4].try_into().ok()?);

        Some(Self {
            length,
            op_type,
            is_update,
            record_type,
            flags,
            timestamp,
            lsn,
            prev_lsn,
            checksum,
        })
    }

    /// Get bytes reference (unsafe but documented)
    ///
    /// # Safety
    /// This is safe because WalHeader is repr(C) with fixed-size fields.
    /// Use as_bytes_safe() for a guaranteed safe alternative.
    #[deprecated(since = "0.2.0", note = "Use as_bytes_safe() instead")]
    pub fn as_bytes(&self) -> &[u8] {
        unsafe {
            std::slice::from_raw_parts(
                self as *const WalHeader as *const u8,
                std::mem::size_of::<WalHeader>(),
            )
        }
    }

    /// Deserialize from bytes (unsafe but documented)
    ///
    /// # Safety
    /// This is safe only if the input comes from a properly serialized WalHeader.
    /// Use from_bytes_safe() for a guaranteed safe alternative.
    #[deprecated(since = "0.2.0", note = "Use from_bytes_safe() instead")]
    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < Self::SIZE {
            return None;
        }
        let header: WalHeader = unsafe { std::ptr::read(bytes.as_ptr() as *const WalHeader) };
        Some(header)
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum WalError {
    #[error("IO error: {0}")]
    IoError(String),

    #[error("Invalid operation type: {0}")]
    InvalidOpType(u8),

    #[error("Invalid header")]
    InvalidHeader,

    #[error("Invalid file header")]
    InvalidFileHeader,

    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: u32, actual: u32 },

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Corrupted WAL: {0}")]
    Corrupted(String),

    #[error("WAL is closed")]
    Closed,

    #[error("Unsupported WAL version: {0}")]
    UnsupportedVersion(u32),

    #[error("Recovery aborted: {0}")]
    RecoveryAborted(String),

    #[error("Invalid operation: {0}")]
    InvalidOperation(String),
}

impl From<std::io::Error> for WalError {
    fn from(e: std::io::Error) -> Self {
        WalError::IoError(e.to_string())
    }
}

impl From<postcard::Error> for WalError {
    fn from(e: postcard::Error) -> Self {
        WalError::SerializationError(e.to_string())
    }
}

pub type WalResult<T> = Result<T, WalError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalRecoveryMode {
    AbortOnCorruption,
    #[default]
    SkipCorruption,
    WalOnly,
    ErrorIfMissing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WalCompression {
    #[default]
    None,
    Zstd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArchiveMode {
    #[default]
    None,
    Move,
    Copy,
}

impl WalCompression {
    pub fn flag_byte(&self) -> u8 {
        match self {
            WalCompression::None => 0,
            WalCompression::Zstd => 2,
        }
    }

    pub fn from_flag_byte(byte: u8) -> Self {
        match byte & 0x0F {
            2 => WalCompression::Zstd,
            _ => WalCompression::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressionLevel {
    pub algorithm: WalCompression,
    pub level: u8,
}

impl Default for CompressionLevel {
    fn default() -> Self {
        Self {
            algorithm: WalCompression::None,
            level: 3,
        }
    }
}

impl CompressionLevel {
    pub fn none() -> Self {
        Self {
            algorithm: WalCompression::None,
            level: 0,
        }
    }

    pub fn zstd(level: u8) -> Self {
        Self {
            algorithm: WalCompression::Zstd,
            level: level.clamp(1, 22),
        }
    }

    pub fn zstd_default() -> Self {
        Self {
            algorithm: WalCompression::Zstd,
            level: 3,
        }
    }

    pub fn zstd_fast() -> Self {
        Self {
            algorithm: WalCompression::Zstd,
            level: 1,
        }
    }

    pub fn zstd_best() -> Self {
        Self {
            algorithm: WalCompression::Zstd,
            level: 22,
        }
    }
}

pub mod wal_flags {
    pub const COMPRESSION_MASK: u16 = 0x000F;
    pub const COMPRESSED: u16 = 0x0001;
}

#[derive(Debug, Clone)]
pub struct WalContentUnit {
    pub data: Vec<u8>,
    pub size: usize,
}

impl WalContentUnit {
    pub fn new(data: Vec<u8>) -> Self {
        let size = data.len();
        Self { data, size }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Debug, Clone)]
pub struct UpdateWalUnit {
    pub timestamp: Timestamp,
    pub content: WalContentUnit,
}

impl UpdateWalUnit {
    pub fn new(timestamp: Timestamp, data: Vec<u8>) -> Self {
        Self {
            timestamp,
            content: WalContentUnit::new(data),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SyncPolicy {
    Never,
    #[default]
    EveryWrite,
    Periodic {
        interval_ms: u64,
    },
    Batch {
        batch_size: usize,
    },
}

impl SyncPolicy {
    pub fn periodic(interval: Duration) -> Self {
        Self::Periodic {
            interval_ms: interval.as_millis() as u64,
        }
    }

    pub fn batch(batch_size: usize) -> Self {
        Self::Batch { batch_size }
    }

    pub fn is_never(&self) -> bool {
        matches!(self, Self::Never)
    }

    pub fn is_every_write(&self) -> bool {
        matches!(self, Self::EveryWrite)
    }

    pub fn is_periodic(&self) -> bool {
        matches!(self, Self::Periodic { .. })
    }

    pub fn is_batch(&self) -> bool {
        matches!(self, Self::Batch { .. })
    }

    pub fn requires_sync(&self, write_count: usize, last_sync_time: Duration) -> bool {
        match self {
            SyncPolicy::Never => false,
            SyncPolicy::EveryWrite => true,
            SyncPolicy::Periodic { interval_ms } => {
                last_sync_time.as_millis() as u64 >= *interval_ms
            }
            SyncPolicy::Batch { batch_size } => write_count >= *batch_size,
        }
    }
}

#[derive(Debug, Clone)]
pub struct WalConfig {
    pub truncate_size: usize,
    pub max_file_size: usize,
    pub max_total_size: usize,
    pub ttl_seconds: u64,
    pub checkpoint_interval: u64,
    pub auto_checkpoint: bool,
    pub archive_dir: Option<String>,
    pub archive_mode: ArchiveMode,
    pub sync_policy: SyncPolicy,
    pub recovery_mode: WalRecoveryMode,
    pub compression: WalCompression,
    pub compression_level: CompressionLevel,
    pub checksum_enabled: bool,
    pub max_parallel_recovery_threads: usize,
    pub circular_buffer: bool,
    pub circular_buffer_size: usize,
}

impl Default for WalConfig {
    fn default() -> Self {
        Self {
            truncate_size: 4 * 1024 * 1024,
            max_file_size: 16 * 1024 * 1024,
            max_total_size: 256 * 1024 * 1024,
            ttl_seconds: 0,
            checkpoint_interval: 10000,
            auto_checkpoint: true,
            archive_dir: None,
            archive_mode: ArchiveMode::None,
            sync_policy: SyncPolicy::EveryWrite,
            recovery_mode: WalRecoveryMode::default(),
            compression: WalCompression::None,
            compression_level: CompressionLevel::default(),
            checksum_enabled: true,
            max_parallel_recovery_threads: 4,
            circular_buffer: false,
            circular_buffer_size: 16 * 1024 * 1024,
        }
    }
}

impl WalConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_truncate_size(mut self, size: usize) -> Self {
        self.truncate_size = size;
        self
    }

    pub fn with_max_file_size(mut self, size: usize) -> Self {
        self.max_file_size = size;
        self
    }

    pub fn with_sync_policy(mut self, policy: SyncPolicy) -> Self {
        self.sync_policy = policy;
        self
    }

    pub fn with_sync_on_write(mut self, sync: bool) -> Self {
        self.sync_policy = if sync {
            SyncPolicy::EveryWrite
        } else {
            SyncPolicy::Never
        };
        self
    }

    pub fn with_recovery_mode(mut self, mode: WalRecoveryMode) -> Self {
        self.recovery_mode = mode;
        self
    }

    pub fn with_compression(mut self, compression: WalCompression) -> Self {
        self.compression = compression;
        self.compression_level = CompressionLevel {
            algorithm: compression,
            level: self.compression_level.level,
        };
        self
    }

    pub fn with_compression_level(mut self, level: CompressionLevel) -> Self {
        self.compression = level.algorithm;
        self.compression_level = level;
        self
    }

    pub fn with_checksum(mut self, enabled: bool) -> Self {
        self.checksum_enabled = enabled;
        self
    }

    pub fn with_parallel_recovery(mut self, threads: usize) -> Self {
        self.max_parallel_recovery_threads = threads;
        self
    }

    pub fn with_circular_buffer(mut self, enabled: bool) -> Self {
        self.circular_buffer = enabled;
        self
    }

    pub fn with_circular_buffer_size(mut self, size: usize) -> Self {
        self.circular_buffer_size = size;
        self
    }

    pub fn with_max_total_size(mut self, size: usize) -> Self {
        self.max_total_size = size;
        self
    }

    pub fn with_ttl_seconds(mut self, seconds: u64) -> Self {
        self.ttl_seconds = seconds;
        self
    }

    pub fn with_checkpoint_interval(mut self, interval: u64) -> Self {
        self.checkpoint_interval = interval;
        self
    }

    pub fn with_auto_checkpoint(mut self, enabled: bool) -> Self {
        self.auto_checkpoint = enabled;
        self
    }

    pub fn with_archive_dir(mut self, dir: String) -> Self {
        self.archive_dir = Some(dir);
        self
    }

    pub fn with_archive_mode(mut self, mode: ArchiveMode) -> Self {
        self.archive_mode = mode;
        self
    }
}

#[derive(Debug, Clone, Default)]
pub struct WalStats {
    pub total_rotations: u64,
    pub total_files_deleted: u64,
    pub total_files_archived: u64,
    pub last_rotation_time: Option<u64>,
    pub total_bytes_written: u64,
    pub total_entries_written: u64,
    pub total_checkpoints: u64,
    pub last_checkpoint_duration_us: u64,
    pub total_syncs: u64,
    pub total_write_latency_us: u64,
    pub write_ops: u64,
    pub peak_dirty_pages: usize,
    pub current_dirty_pages: usize,
    pub total_bytes_compressed: u64,
    pub total_bytes_after_compression: u64,
}

impl WalStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_rotation(&mut self) {
        self.total_rotations += 1;
        self.last_rotation_time = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or(std::time::Duration::from_secs(0))
                .as_secs(),
        );
    }

    pub fn record_file_deleted(&mut self) {
        self.total_files_deleted += 1;
    }

    pub fn record_file_archived(&mut self) {
        self.total_files_archived += 1;
    }

    pub fn record_write(&mut self, bytes: u64) {
        self.total_bytes_written += bytes;
        self.total_entries_written += 1;
    }

    pub fn record_write_with_latency(&mut self, bytes: u64, latency_us: u64) {
        self.record_write(bytes);
        self.total_write_latency_us += latency_us;
        self.write_ops += 1;
    }

    pub fn record_checkpoint(&mut self, duration_us: u64) {
        self.total_checkpoints += 1;
        self.last_checkpoint_duration_us = duration_us;
    }

    pub fn record_sync(&mut self) {
        self.total_syncs += 1;
    }

    pub fn update_dirty_pages(&mut self, count: usize) {
        self.current_dirty_pages = count;
        if count > self.peak_dirty_pages {
            self.peak_dirty_pages = count;
        }
    }

    pub fn record_compression(&mut self, original: u64, compressed: u64) {
        self.total_bytes_compressed += original;
        self.total_bytes_after_compression += compressed;
    }

    pub fn average_write_latency_us(&self) -> u64 {
        self.total_write_latency_us
            .checked_div(self.write_ops)
            .unwrap_or(0)
    }

    pub fn compression_ratio(&self) -> f64 {
        if self.total_bytes_compressed == 0 {
            0.0
        } else {
            self.total_bytes_after_compression as f64 / self.total_bytes_compressed as f64
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParsedWalEntry {
    pub header: WalHeader,
    pub payload: Vec<u8>,
}

pub type RecoveryResult<T> = Result<T, WalError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsn() {
        let mut lsn = Lsn::new(100);
        assert_eq!(lsn.as_u64(), 100);

        let old = lsn.increment(50);
        assert_eq!(old.as_u64(), 100);
        assert_eq!(lsn.as_u64(), 150);

        let display = format!("{}", lsn);
        assert!(display.contains("LSN"));
    }

    #[test]
    fn test_wal_header() {
        let header = WalHeader::new(WalOpType::InsertVertex, 12345, 100);
        assert_eq!(header.length, 100);
        assert_eq!(header.timestamp, 12345);
        assert_eq!(header.op_type, WalOpType::InsertVertex as u8);
        assert!(!header.is_update);
        assert_eq!(header.record_type, RecordType::Full);
    }

    #[test]
    fn test_wal_header_with_lsn() {
        let header = WalHeader::new(WalOpType::InsertVertex, 12345, 100)
            .with_lsn(Lsn::new(1000), Lsn::new(900));

        assert_eq!(header.lsn().as_u64(), 1000);
        assert_eq!(header.prev_lsn().as_u64(), 900);
    }

    #[test]
    fn test_wal_op_type() {
        assert_eq!(WalOpType::try_from(0).unwrap(), WalOpType::InsertVertex);
        assert_eq!(WalOpType::try_from(6).unwrap(), WalOpType::UpdateVertexProp);
        assert_eq!(WalOpType::try_from(17).unwrap(), WalOpType::CreateSpace);
        assert!(WalOpType::try_from(100).is_err());
    }

    #[test]
    fn test_wal_header_serialization() {
        let header = WalHeader::new(WalOpType::InsertEdge, 999, 50);
        let bytes = header.as_bytes();
        assert_eq!(bytes.len(), WalHeader::SIZE);

        let parsed = WalHeader::from_bytes(bytes).unwrap();
        assert_eq!(parsed.length, 50);
        assert_eq!(parsed.timestamp, 999);
    }

    #[test]
    fn test_wal_header_checksum() {
        let payload = b"test_payload_data";
        let header = WalHeader::new(WalOpType::InsertVertex, 12345, payload.len() as u32)
            .with_lsn(Lsn::new(100), Lsn::new(0))
            .with_checksum(payload);

        assert!(header.verify_checksum(payload));

        let corrupted_payload = b"corrupted_data";
        assert!(!header.verify_checksum(corrupted_payload));
    }

    #[test]
    fn test_wal_file_header() {
        let header = WalFileHeader::new(1, 0, Lsn::new(1000));
        assert!(header.is_valid());
        assert_eq!(header.thread_id, 1);
        assert_eq!(header.checkpoint_seq, 0);
        assert_eq!(header.start_lsn().as_u64(), 1000);

        let bytes = header.as_bytes();
        assert_eq!(bytes.len(), WalFileHeader::SIZE);

        let parsed = WalFileHeader::from_bytes(bytes).unwrap();
        assert!(parsed.is_valid());
        assert_eq!(parsed.thread_id, 1);
        assert_eq!(parsed.start_lsn().as_u64(), 1000);
    }

    #[test]
    fn test_wal_config() {
        let config = WalConfig::new()
            .with_checksum(true)
            .with_recovery_mode(WalRecoveryMode::AbortOnCorruption)
            .with_sync_policy(SyncPolicy::Batch { batch_size: 100 })
            .with_parallel_recovery(8);

        assert!(config.checksum_enabled);
        assert_eq!(config.recovery_mode, WalRecoveryMode::AbortOnCorruption);
        assert_eq!(config.max_parallel_recovery_threads, 8);
    }

    #[test]
    fn test_sync_policy() {
        assert!(SyncPolicy::EveryWrite.requires_sync(0, Duration::ZERO));
        assert!(!SyncPolicy::Never.requires_sync(100, Duration::ZERO));
        assert!(SyncPolicy::Batch { batch_size: 10 }.requires_sync(10, Duration::ZERO));
        assert!(!SyncPolicy::Batch { batch_size: 10 }.requires_sync(5, Duration::ZERO));
    }

    #[test]
    fn test_record_type() {
        assert_eq!(RecordType::from_u8(0), RecordType::Full);
        assert_eq!(RecordType::from_u8(1), RecordType::First);
        assert_eq!(RecordType::from_u8(2), RecordType::Middle);
        assert_eq!(RecordType::from_u8(3), RecordType::Last);
    }

    #[test]
    fn test_block_alignment() {
        assert_eq!(WAL_BLOCK_SIZE, 32 * 1024);

        assert_eq!(block_padding_needed(0), 0);
        assert_eq!(block_padding_needed(WAL_BLOCK_SIZE), 0);
        assert_eq!(block_padding_needed(WAL_BLOCK_SIZE / 2), WAL_BLOCK_SIZE / 2);
        assert_eq!(block_padding_needed(100), WAL_BLOCK_SIZE - 100);

        assert!(is_block_aligned(0));
        assert!(is_block_aligned(WAL_BLOCK_SIZE));
        assert!(is_block_aligned(WAL_BLOCK_SIZE * 2));
        assert!(!is_block_aligned(100));
        assert!(!is_block_aligned(WAL_BLOCK_SIZE - 1));

        assert_eq!(align_to_block(0), 0);
        assert_eq!(align_to_block(1), WAL_BLOCK_SIZE);
        assert_eq!(align_to_block(WAL_BLOCK_SIZE), WAL_BLOCK_SIZE);
        assert_eq!(align_to_block(WAL_BLOCK_SIZE + 1), WAL_BLOCK_SIZE * 2);

        assert_eq!(blocks_needed(0), 0);
        assert_eq!(blocks_needed(1), 1);
        assert_eq!(blocks_needed(WAL_BLOCK_SIZE), 1);
        assert_eq!(blocks_needed(WAL_BLOCK_SIZE + 1), 2);
    }
}
