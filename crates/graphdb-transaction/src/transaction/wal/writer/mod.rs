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
mod dummy;
mod factory;
mod local;
mod sync;

pub use compression::decompress_payload;
pub use dummy::DummyWalWriter;
pub use factory::WalWriterFactory;
pub use local::LocalWalWriter;
pub use crate::core::wal::traits::WalWriter;
