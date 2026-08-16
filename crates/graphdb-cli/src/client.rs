//! Client module for GraphDB CLI
//!
//! Provides HTTP client for connecting to GraphDB server. Wire DTOs are
//! re-exported from `graphdb-wire` (the single contract source shared with
//! the server) so the CLI never mirrors HTTP types by hand.

mod config;
mod http_client;
mod schema;
mod transaction;
mod types;

pub use config::{ClientConfig, SessionInfo};
pub use graphdb_wire::batch::BatchItem;
pub use graphdb_wire::meta::{
    ColdSnapshotInfo, ConfigItem, ConfigSection, DatabaseStatistics, QueryStatistics,
    ServerConfig, SessionStatistics, TransactionResponse as TransactionInfo,
};
pub use graphdb_wire::schema::{EdgeTypeInfo, FieldInfo, PropertyDef, SpaceInfo, TagInfo};
pub use http_client::HttpClient;
pub use transaction::TransactionOptions;
pub use types::{QueryErrorInfo, QueryResult};
