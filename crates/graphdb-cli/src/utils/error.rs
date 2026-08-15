use thiserror::Error;

#[derive(Error, Debug)]
pub enum CliError {
    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Authentication failed: {0}")]
    AuthError(String),

    #[error("Query execution failed: {0}")]
    QueryError(String),

    #[error("Session error: {0}")]
    SessionError(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("HTTP request error: {0}")]
    HttpError(#[from] reqwest::Error),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Command error: {0}")]
    CommandError(String),

    #[error("No active connection")]
    NotConnected,

    #[error("No space selected")]
    NoSpaceSelected,

    #[error("Unknown command: {0}")]
    UnknownCommand(String),

    #[error("Script file not found: {0}")]
    ScriptNotFound(String),

    #[error("Transaction already active")]
    TransactionAlreadyActive,

    #[error("No active transaction")]
    NoActiveTransaction,

    #[error("Savepoint not found: {0}")]
    SavepointNotFound(String),

    #[error("Transaction timeout")]
    TransactionTimeout,

    #[error("Transaction is in failed state: {0}")]
    TransactionFailed(String),

    #[error("Cannot change autocommit while transaction is active")]
    CannotChangeAutocommit,

    #[error("Invalid value: {0}")]
    InvalidValue(String),

    #[error("Import error: {0}")]
    ImportError(String),

    #[error("Export error: {0}")]
    ExportError(String),

    #[error("{0}")]
    AnyhowError(String),

    #[error("{0}")]
    Other(String),
}

impl From<anyhow::Error> for CliError {
    fn from(err: anyhow::Error) -> Self {
        CliError::AnyhowError(err.to_string())
    }
}

impl CliError {
    pub fn connection(msg: impl Into<String>) -> Self {
        CliError::ConnectionError(msg.into())
    }

    pub fn auth(msg: impl Into<String>) -> Self {
        CliError::AuthError(msg.into())
    }

    pub fn query(msg: impl Into<String>) -> Self {
        CliError::QueryError(msg.into())
    }

    pub fn session(msg: impl Into<String>) -> Self {
        CliError::SessionError(msg.into())
    }

    pub fn config(msg: impl Into<String>) -> Self {
        CliError::ConfigError(msg.into())
    }

    pub fn command(msg: impl Into<String>) -> Self {
        CliError::CommandError(msg.into())
    }

    pub fn import(msg: impl Into<String>) -> Self {
        CliError::ImportError(msg.into())
    }

    pub fn export(msg: impl Into<String>) -> Self {
        CliError::ExportError(msg.into())
    }

    pub fn transaction(msg: impl Into<String>) -> Self {
        CliError::TransactionError(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, CliError>;
