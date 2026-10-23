//! Client crate for Linkrs.
//!
//! Provides HTTP client, session state, and transport result types for
//! connecting to a Linkrs server. Wire DTOs are re-exported from
//! `linkrs-wire`.

pub mod config;
pub mod http_client;
pub mod outbox;
pub mod schema;
pub mod session;
pub mod sse;
pub mod transaction;
pub mod types;
pub mod utils;

pub use utils::error::ClientError;

pub use config::{ClientConfig, SessionInfo};
pub use http_client::HttpClient;
pub use linkrs_wire::batch::BatchItem;
pub use linkrs_wire::meta::{
    ConfigItem, ConfigSection, DatabaseStatistics, QueryStatistics, ServerConfig,
    SessionStatistics, TransactionResponse as TransactionInfo,
};
pub use linkrs_wire::schema::{EdgeTypeInfo, FieldInfo, PropertyDef, SpaceInfo, TagInfo};
pub use session::manager::{Session, SessionManager};
pub use transaction::TransactionOptions;
pub use types::{QueryErrorInfo, QueryResult};
