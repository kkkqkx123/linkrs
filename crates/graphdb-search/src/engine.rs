#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConsistencyState {
    Consistent,
    Inconsistent,
    Rebuilding,
}

pub use crate::config::common::fulltext::FulltextEngineType as EngineType;
