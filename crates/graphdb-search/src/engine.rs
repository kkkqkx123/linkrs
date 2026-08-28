#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ConsistencyState {
    Consistent,
    Inconsistent,
    Rebuilding,
}

pub use graphdb_config::fulltext::FulltextEngineType as EngineType;
