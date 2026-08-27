//! WAL Writer Module
//!
//! Provides Write-Ahead Log writing functionality with:
//! - Local file-based WAL writer
//! - Group commit batching
//! - Configurable compression (Zstd)
//! - Multiple sync policies
//! - File rotation and cleanup
//! - Archive support

mod compression;
mod group_commit;
mod local;
mod sync;

pub use crate::core::wal::traits::WalWriter;
pub use compression::decompress_payload;
pub use group_commit::GroupCommitCoordinator;
pub use local::LocalWalWriter;
