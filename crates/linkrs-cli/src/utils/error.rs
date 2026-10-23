pub use linkrs_client::ClientError as CliError;

pub type Result<T> = std::result::Result<T, CliError>;
