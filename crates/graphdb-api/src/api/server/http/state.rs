use crate::api::server::HttpServer;
use crate::storage::{
    StorageClient, StorageSchemaContextOps, StorageSyncContextOps, StorageTransactionContextOps,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState<
    S: StorageClient
        + StorageSchemaContextOps
        + StorageSyncContextOps
        + StorageTransactionContextOps
        + Clone
        + 'static,
> {
    pub server: Arc<HttpServer<S>>,
}

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageTransactionContextOps
            + Clone
            + 'static,
    > AppState<S>
{
    pub fn new(server: Arc<HttpServer<S>>) -> Self {
        Self { server }
    }
}
