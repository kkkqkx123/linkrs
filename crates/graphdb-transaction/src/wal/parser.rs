//! WAL Parser
//!
//! Provides Write-Ahead Log parsing functionality for recovery.
//!
//! Result types live in [`types`], the fragment reassembly state machine in
//! [`fragment`], the byte-scan engine in [`scan`], shared directory discovery
//! and header validation in [`discover`], the sequential and parallel
//! schedulers in [`sequential`] and [`parallel`], and the parser trait plus
//! factory in [`factory`].

mod discover;
mod factory;
mod fragment;
mod parallel;
mod scan;
mod sequential;
mod types;

pub use factory::{WalParser, WalParserFactory};
pub use parallel::ParallelWalParser;
pub use scan::{compute_checksum_public, verify_entry_checksum};
pub use sequential::LocalWalParser;
pub use types::{ParsedWalEntry, RecoveryResult, WalEntryIter};
