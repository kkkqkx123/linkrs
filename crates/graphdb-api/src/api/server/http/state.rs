use crate::api::server::HttpServer;
use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageOperationContextOps
        + Clone
        + 'static,
> {
    pub server: Arc<HttpServer<S>>,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + 'static,
    > AppState<S>
{
    pub fn new(server: Arc<HttpServer<S>>) -> Self {
        Self { server }
    }
}
