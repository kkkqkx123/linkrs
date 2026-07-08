use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum IsolationLevel {
    ReadUncommitted,
    #[default]
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

impl IsolationLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            IsolationLevel::ReadUncommitted => "READ UNCOMMITTED",
            IsolationLevel::ReadCommitted => "READ COMMITTED",
            IsolationLevel::RepeatableRead => "REPEATABLE READ",
            IsolationLevel::Serializable => "SERIALIZABLE",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            IsolationLevel::ReadUncommitted => "May read uncommitted data (dirty reads)",
            IsolationLevel::ReadCommitted => "Only read committed data",
            IsolationLevel::RepeatableRead => "Consistent reads within transaction",
            IsolationLevel::Serializable => "Full isolation, transactions appear sequential",
        }
    }
}

impl std::str::FromStr for IsolationLevel {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().replace('_', " ").as_str() {
            "READ UNCOMMITTED" => Ok(IsolationLevel::ReadUncommitted),
            "READ COMMITTED" => Ok(IsolationLevel::ReadCommitted),
            "REPEATABLE READ" => Ok(IsolationLevel::RepeatableRead),
            "SERIALIZABLE" => Ok(IsolationLevel::Serializable),
            _ => Err(()),
        }
    }
}

impl std::fmt::Display for IsolationLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}
