//! Typed gRPC client for the Linkrs service.
//!
//! [`GrpcClient`] wraps a tonic channel plus the generated
//! `LinkrsServiceClient`, so callers issue typed RPCs without hand-writing
//! tonic glue. The generated request/response message types are re-exported
//! under [`proto`]; methods take the generated request messages and return the
//! generated response messages directly (requests without fields take no
//! argument).
//!
//! Every RPC in `proto/linkrs.proto` is wrapped: health, authentication,
//! sessions, user management, queries (including cursors), transactions,
//! schema, configuration, statistics, batch, migration, import/export,
//! custom functions, vector, fulltext, and sync.
//!
//! The implementation is split by domain: the `core` module owns the client
//! state and transport construction, `error` maps statuses, and the remaining
//! submodules add the RPC wrappers for each service area. The public API is
//! unchanged by this split.
//!
//! Authentication mirrors the HTTP client: [`GrpcClient::login`] stores the
//! session id returned by the server behind a lock, [`GrpcClient::logout`] and
//! [`GrpcClient::auth_me`] reuse it, and [`GrpcClient::authed`] additionally
//! sends it as `x-session-id` metadata for servers that install an auth
//! interceptor. The current server reads the session id from request messages
//! instead, so request-carrying methods fill missing `session_id` (and
//! transaction `owner`) fields from the stored session before dispatch; an
//! explicit caller value always wins.
//!
//! TLS is opt-in through [`GrpcTlsConfig`] together with
//! [`GrpcClient::from_endpoint_with_tls`] and
//! [`GrpcClient::connect_endpoint_with_tls`]. Plaintext constructors keep
//! working unchanged and never touch TLS code.
//!
//! Streaming RPCs ([`GrpcClient::execute_query_stream`],
//! [`GrpcClient::stream_migration_progress`]) hand back the raw
//! `tonic::Streaming` values so callers control consumption and backpressure.
//!
//! Errors are mapped into the crate [`ClientError`]: connection establishment
//! failures become `ConnectionError`, while every non-OK `tonic::Status`
//! (including call-time transport failures) becomes a structured `GrpcError`
//! carrying a [`GrpcErrorCode`] and the server message.

pub mod proto;

mod auth;
mod batch;
mod config;
mod core;
mod error;
mod fulltext;
mod functions;
mod migration;
mod query;
mod schema;
mod statistics;
mod sync;
mod transaction;
mod transfer;
mod users;
mod vector;

pub use core::{GrpcClient, GrpcTlsConfig};
