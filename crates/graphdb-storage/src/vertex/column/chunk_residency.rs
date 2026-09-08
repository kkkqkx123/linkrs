//! Chunk residency tracking.
//!
//! `ChunkResidency` tracks whether a chunk's data is in memory (Resident).

// ---------------------------------------------------------------------------
// ChunkResidency
// ---------------------------------------------------------------------------

/// Memory residency state of a [`ColumnChunk`].
#[derive(Debug, Clone, Default)]
pub enum ChunkResidency {
    /// Data is in memory and accessible.
    #[default]
    Resident,
}
