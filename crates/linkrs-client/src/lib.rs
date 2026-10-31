//! Client crate for Linkrs.
//!
//! Provides HTTP client, gRPC client (behind the `grpc` feature), session
//! state, and transport result types for connecting to a Linkrs server. Wire
//! DTOs are re-exported from `linkrs-wire`.

pub mod config;
#[cfg(feature = "grpc")]
pub mod grpc_client;
pub mod http_client;
pub mod outbox;
pub mod schema;
pub mod session;
pub mod sse;
pub mod transaction;
pub mod types;
pub mod utils;

pub use utils::error::{ClientError, GrpcErrorCode};

pub use config::{ClientConfig, SessionInfo};
#[cfg(feature = "grpc")]
pub use grpc_client::{GrpcClient, GrpcTlsConfig};
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
