//! Network Service Layer
//!
//! Provide a GraphDB service interface based on HTTP/RPC

pub mod auth;
pub mod batch;
pub mod client;
pub mod graph_service;
#[cfg(feature = "grpc")]
pub mod grpc;
pub mod http;
pub mod permission;
pub mod session;
pub mod web;

pub use auth::{Authenticator, PasswordAuthenticator};
pub use batch::BatchManager;
pub use client::{ClientSession, Session};
pub use graph_service::GraphService;
#[cfg(feature = "grpc")]
pub use grpc::{run_server, GraphDBService};
pub use http::HttpServer;
pub use permission::{Permission, PermissionChecker, PermissionManager, RoleType};
pub use session::GraphSessionManager;
pub use web::WebState;
