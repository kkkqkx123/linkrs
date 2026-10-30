//! Transaction options builder (wire request construction helper).

/// Transaction options for beginning a transaction
#[derive(Debug, Clone, Default)]
pub struct TransactionOptions {
    pub read_only: bool,
    pub timeout_seconds: Option<u64>,
    pub query_timeout_seconds: Option<u64>,
    pub statement_timeout_seconds: Option<u64>,
    pub idle_timeout_seconds: Option<u64>,
    pub isolation_level: Option<String>,
}

impl TransactionOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub fn with_timeout(mut self, seconds: u64) -> Self {
        self.timeout_seconds = Some(seconds);
        self
    }

    pub fn with_query_timeout(mut self, seconds: u64) -> Self {
        self.query_timeout_seconds = Some(seconds);
        self
    }

    pub fn with_statement_timeout(mut self, seconds: u64) -> Self {
        self.statement_timeout_seconds = Some(seconds);
        self
    }

    pub fn with_idle_timeout(mut self, seconds: u64) -> Self {
        self.idle_timeout_seconds = Some(seconds);
        self
    }

    pub fn with_isolation_level(mut self, level: impl Into<String>) -> Self {
        self.isolation_level = Some(level.into());
        self
    }
}
