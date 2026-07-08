//! Transaction management types

/// Transaction options for beginning a transaction
#[derive(Debug, Clone, Default)]
pub struct TransactionOptions {
    pub read_only: bool,
    pub timeout_seconds: Option<u64>,
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
}

/// Transaction information returned after beginning a transaction
#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub transaction_id: u64,
    pub status: String,
}
