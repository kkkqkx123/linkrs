//! Client module for GraphDB CLI
//!
//! Provides HTTP client for connecting to GraphDB server. Wire DTOs are
//! re-exported from `linkrs-wire` (the single contract source shared with
//! the server) so the CLI never mirrors HTTP types by hand.

mod config;
mod http_client;
mod schema;
mod sse;
mod transaction;
mod types;

pub use config::{ClientConfig, SessionInfo};
pub use http_client::HttpClient;
pub use linkrs_wire::batch::BatchItem;
pub use linkrs_wire::meta::{
    ConfigItem, ConfigSection, DatabaseStatistics, QueryStatistics, ServerConfig,
    SessionStatistics, TransactionResponse as TransactionInfo,
};
pub use linkrs_wire::schema::{EdgeTypeInfo, FieldInfo, PropertyDef, SpaceInfo, TagInfo};
pub use transaction::TransactionOptions;
pub use types::{QueryErrorInfo, QueryResult};
