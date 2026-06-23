//! Write-Ahead Log (WAL) Core Types
//!
//! Provides the fundamental types and traits for the Write-Ahead Log system.
//! These types are shared between the transaction layer and storage engine.

pub mod redo;
pub mod traits;
pub mod types;

pub use redo::{
    AddEdgePropRedo, AddVertexPropRedo, AlterSpaceCommentRedo, ClearSpaceRedo, CompactRedo,
    CreateEdgeTypeRedo, CreateSpaceRedo, CreateVertexTypeRedo, DeleteEdgePropRedo, DeleteEdgeRedo,
    DeleteEdgeTypeRedo, DeleteVertexPropRedo, DeleteVertexRedo, DeleteVertexTypeRedo,
    DropSpaceRedo, InsertEdgeRedo, InsertVertexRedo, RenameEdgePropRedo, RenameVertexPropRedo,
    UpdateEdgePropRedo, UpdateVertexPropRedo,
};
pub use traits::{RecoveryApplier, WalWriter};
pub use types::{
    align_to_block, block_padding_needed, blocks_needed, is_block_aligned, wal_flags, ArchiveMode,
    CompressionLevel, Lsn, ParsedWalEntry, RecordType, RecoveryResult, SyncPolicy, UpdateWalUnit,
    WalCompression, WalConfig, WalContentUnit, WalError, WalFileHeader, WalHeader, WalOpType,
    WalRecoveryMode, WalResult, WalStats, WAL_BLOCK_SIZE, WAL_FILE_HEADER_SIZE, WAL_HEADER_SIZE,
    WAL_MAGIC, WAL_MAX_RECORD_SIZE, WAL_VERSION,
};
