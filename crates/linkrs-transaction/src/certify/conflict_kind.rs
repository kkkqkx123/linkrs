//! Structured conflict classification for observability and retry policy.
//!
//! Every certification failure maps to one variant; the error is still
/// surfaced as a unified `TransactionError` to the client but the
/// classification is logged and counted in `TransactionStats`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConflictType {
    WriteWrite,
    ReadWrite,
    Phantom,
    SchemaGeneration,
    IndexGeneration,
}

impl std::fmt::Display for ConflictType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ConflictType::WriteWrite => "write_write",
            ConflictType::ReadWrite => "read_write",
            ConflictType::Phantom => "phantom",
            ConflictType::SchemaGeneration => "schema_generation",
            ConflictType::IndexGeneration => "index_generation",
        };
        write!(f, "{}", s)
    }
}
