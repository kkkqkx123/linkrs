//! Parser abstraction and factory.

use graphdb_core::types::Timestamp;
use graphdb_core::wal::types::{WalError, WalResult};

use super::sequential::LocalWalParser;

/// WAL parser trait
pub trait WalParser: Send + Sync {
    /// Open and parse WAL files
    fn open(&mut self, wal_uri: &str) -> WalResult<()>;

    /// Close the parser
    fn close(&mut self);

    /// Get the last timestamp
    fn last_timestamp(&self) -> Timestamp;
}

/// WAL parser factory
pub struct WalParserFactory;

impl WalParserFactory {
    /// Create a WAL parser based on the URI scheme
    pub fn create_wal_parser(wal_uri: &str) -> WalResult<Box<dyn WalParser>> {
        let scheme = Self::get_scheme(wal_uri);

        match scheme.as_str() {
            "file" | "" => Ok(Box::new(LocalWalParser::new())),
            _ => Err(WalError::IoError(format!(
                "Unknown WAL parser scheme: {}",
                scheme
            ))),
        }
    }

    fn get_scheme(uri: &str) -> String {
        if let Some(pos) = uri.find("://") {
            uri[..pos].to_string()
        } else {
            "file".to_string()
        }
    }
}
