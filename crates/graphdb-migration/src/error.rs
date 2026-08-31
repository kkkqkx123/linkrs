use graphdb_core::StorageError;

use crate::converter::ConversionError;

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error("storage error: {0}")]
    Storage(#[from] Box<StorageError>),

    #[error("plan error: {0}")]
    Plan(String),

    #[error("conversion error: {0}")]
    Conversion(#[from] ConversionError),

    #[error("lock error: {0}")]
    Lock(String),

    #[error("checkpoint error: {0}")]
    Checkpoint(String),
}

impl From<StorageError> for MigrationError {
    fn from(e: StorageError) -> Self {
        MigrationError::Storage(Box::new(e))
    }
}
