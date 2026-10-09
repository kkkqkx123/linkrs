//! gRPC service state.
//!
//! Owns the shared server state carried by every RPC handler.

use std::time::Instant;

use tonic::Status;

use crate::config::Config;
use crate::http::AppState;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::proto::{MigrationProgressEvent, StreamResponse};

pub(crate) type ExecuteQueryStreamStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = Result<StreamResponse, Status>> + Send + 'static>,
>;

pub(crate) type StreamMigrationProgressStream = std::pin::Pin<
    Box<dyn tokio_stream::Stream<Item = Result<MigrationProgressEvent, Status>> + Send + 'static>,
>;

/// gRPC service implementation behind the generated trait.
pub struct LinkrsService<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
> {
    pub(crate) app_state: AppState<S>,
    pub(crate) config: Config,
    pub(crate) start_time: Instant,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > LinkrsService<S>
{
    /// Create a new gRPC service instance.
    pub fn new(app_state: AppState<S>, config: Config) -> Self {
        Self {
            app_state,
            config,
            start_time: Instant::now(),
        }
    }

    /// Get the current configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Get application state.
    pub fn app_state(&self) -> &AppState<S> {
        &self.app_state
    }

    /// Service start time used by health reporting.
    pub fn start_time(&self) -> Instant {
        self.start_time
    }
}
