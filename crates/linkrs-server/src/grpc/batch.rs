//! Batch task handlers and wire mapping.

use std::collections::HashMap;

use tonic::{Request, Response, Status};

use crate::storage::{
    StorageClient, StorageOperationContextOps, StorageSchemaContextOps, StorageSyncContextOps,
};

use super::convert::proto_value_to_core;
use super::proto::*;
use super::service::LinkrsService;

impl<
        S: StorageClient
            + StorageSchemaContextOps
            + StorageSyncContextOps
            + StorageOperationContextOps
            + Clone
            + Send
            + Sync
            + 'static,
    > LinkrsService<S>
{
    pub(crate) async fn handle_create_batch(
        &self,
        request: Request<CreateBatchRequest>,
    ) -> Result<Response<CreateBatchResponse>, Status> {
        let req = request.into_inner();
        if req.space_name.is_empty() {
            return Err(Status::invalid_argument("space_name must not be empty"));
        }
        let storage = self.app_state.server.get_storage();
        let space_id = storage
            .read()
            .get_space_id(&req.space_name)
            .map_err(|_| Status::not_found(format!("space '{}' not found", req.space_name)))?;
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .create_task(space_id, crate::batch::BatchType::Mixed, 1000)
            .map_err(|e| Status::internal(format!("failed to create batch task: {e}")))?;
        Ok(Response::new(CreateBatchResponse {
            success: true,
            batch_id: task.id,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_add_batch_items(
        &self,
        request: Request<AddBatchItemsRequest>,
    ) -> Result<Response<AddBatchItemsResponse>, Status> {
        let req = request.into_inner();
        if req.items.is_empty() {
            return Err(Status::invalid_argument("batch items must not be empty"));
        }
        let mut wire_items = Vec::with_capacity(req.items.len());
        for item in req.items {
            wire_items.push(proto_batch_item_to_wire(item)?);
        }
        let batch_manager = self.app_state.server.get_batch_manager();
        let accepted = batch_manager
            .add_items(&req.batch_id, wire_items)
            .map_err(|e| {
                let message = e.to_string();
                if message.contains("does not exist") {
                    Status::not_found(message)
                } else {
                    Status::failed_precondition(message)
                }
            })?;
        Ok(Response::new(AddBatchItemsResponse {
            success: true,
            items_added: accepted as i32,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_execute_batch(
        &self,
        request: Request<ExecuteBatchRequest>,
    ) -> Result<Response<ExecuteBatchResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .get_task(&req.batch_id)
            .ok_or_else(|| Status::not_found(format!("batch task '{}' not found", req.batch_id)))?;
        let space_name = {
            let storage = self.app_state.server.get_storage();
            let storage_guard = storage.read();
            storage_guard
                .get_space_by_id(task.space_id)
                .map_err(|e| Status::internal(format!("failed to resolve batch space: {e}")))?
                .map(|info| info.space_name)
                .ok_or_else(|| Status::not_found(format!("space id {} not found", task.space_id)))?
        };
        let result = batch_manager
            .execute_task(&req.batch_id, &space_name)
            .await
            .map_err(|e| Status::internal(format!("failed to execute batch task: {e}")))?;
        let success = result.errors.is_empty();
        let mut results: Vec<BatchResult> = result
            .errors
            .iter()
            .map(|e| BatchResult {
                success: false,
                error: e.error.clone(),
            })
            .collect();
        if success {
            results.push(BatchResult {
                success: true,
                error: String::new(),
            });
        }
        Ok(Response::new(ExecuteBatchResponse {
            success,
            results,
            error: result
                .errors
                .iter()
                .map(|e| e.error.clone())
                .collect::<Vec<_>>()
                .join("; "),
            vertices_inserted: result.vertices_inserted as i64,
            edges_inserted: result.edges_inserted as i64,
            vertices_updated: result.vertices_updated as i64,
            edges_updated: result.edges_updated as i64,
            vertices_deleted: result.vertices_deleted as i64,
            edges_deleted: result.edges_deleted as i64,
        }))
    }

    pub(crate) async fn handle_get_batch_status(
        &self,
        request: Request<GetBatchStatusRequest>,
    ) -> Result<Response<GetBatchStatusResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        let task = batch_manager
            .get_task(&req.batch_id)
            .ok_or_else(|| Status::not_found(format!("batch task '{}' not found", req.batch_id)))?;
        Ok(Response::new(GetBatchStatusResponse {
            status: batch_status_name(&task.status),
            total_items: task.progress.total as i32,
            processed_items: task.progress.processed as i32,
            failed_items: task.progress.failed as i32,
            error: String::new(),
        }))
    }

    pub(crate) async fn handle_delete_batch(
        &self,
        request: Request<DeleteBatchRequest>,
    ) -> Result<Response<DeleteBatchResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        match batch_manager.remove_task(&req.batch_id) {
            Ok(()) => Ok(Response::new(DeleteBatchResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => {
                let message = e.to_string();
                if message.contains("does not exist") {
                    Err(Status::not_found(message))
                } else {
                    Err(Status::internal(message))
                }
            }
        }
    }

    pub(crate) async fn handle_cancel_batch(
        &self,
        request: Request<CancelBatchRequest>,
    ) -> Result<Response<CancelBatchResponse>, Status> {
        let req = request.into_inner();
        let batch_manager = self.app_state.server.get_batch_manager();
        match batch_manager.cancel_task(&req.batch_id) {
            Ok(()) => Ok(Response::new(CancelBatchResponse {
                success: true,
                error: String::new(),
            })),
            Err(e) => {
                let message = e.to_string();
                if message.contains("does not exist") {
                    Err(Status::not_found(message))
                } else {
                    Err(Status::failed_precondition(message))
                }
            }
        }
    }
}

/// Map a proto batch item onto the wire batch item.
///
/// Inserts run through the core batch operation while updates and deletes
/// run as direct storage mutations in the batch manager; all variants have
/// a wire representation and are reported with per-item error indexes.
pub(crate) fn proto_batch_item_to_wire(
    item: super::proto::BatchItem,
) -> Result<crate::batch::BatchItem, Status> {
    use super::proto::batch_item::Operation;
    let operation = item
        .operation
        .ok_or_else(|| Status::invalid_argument("batch item operation is required"))?;
    match operation {
        Operation::InsertVertex(v) => {
            Ok(crate::batch::BatchItem::Vertex(crate::batch::VertexData {
                vid: serde_json::Value::String(v.vid),
                tag: v.tag_name,
                properties: proto_properties_to_json(v.properties),
            }))
        }
        Operation::InsertEdge(e) => Ok(crate::batch::BatchItem::Edge(crate::batch::EdgeData {
            edge_type: e.edge_type,
            src_vid: serde_json::Value::String(e.src),
            dst_vid: serde_json::Value::String(e.dst),
            properties: proto_properties_to_json(e.properties),
        })),
        Operation::UpdateVertex(v) => Ok(crate::batch::BatchItem::UpdateVertex(
            crate::batch::VertexData {
                vid: serde_json::Value::String(v.vid),
                tag: v.tag_name,
                properties: proto_properties_to_json(v.properties),
            },
        )),
        Operation::UpdateEdge(e) => Ok(crate::batch::BatchItem::UpdateEdgeData(
            crate::batch::UpdateEdgeData {
                edge_type: e.edge_type,
                src_vid: serde_json::Value::String(e.src),
                dst_vid: serde_json::Value::String(e.dst),
                rank: e.ranking,
                properties: proto_properties_to_json(e.properties),
            },
        )),
        Operation::DeleteVertex(v) => Ok(crate::batch::BatchItem::DeleteVertex(
            crate::batch::DeleteVertexData {
                vid: serde_json::Value::String(v.vid),
                tag_names: v.tag_names,
            },
        )),
        Operation::DeleteEdge(e) => Ok(crate::batch::BatchItem::DeleteEdgeData(
            crate::batch::DeleteEdgeData {
                edge_type: e.edge_type,
                src_vid: serde_json::Value::String(e.src),
                dst_vid: serde_json::Value::String(e.dst),
                rank: e.ranking,
            },
        )),
    }
}

pub(crate) fn proto_properties_to_json(
    properties: HashMap<String, super::proto::Value>,
) -> HashMap<String, serde_json::Value> {
    properties
        .into_iter()
        .map(|(k, v)| (k, crate::value::to_json(proto_value_to_core(v))))
        .collect()
}

/// Render the internal batch status with its proto spelling.
pub(crate) fn batch_status_name(status: &crate::batch::BatchStatus) -> String {
    use crate::batch::BatchStatus;
    match status {
        BatchStatus::Created => "PENDING",
        BatchStatus::Running => "RUNNING",
        BatchStatus::Completed => "COMPLETED",
        BatchStatus::Failed => "FAILED",
        BatchStatus::Cancelled => "CANCELLED",
    }
    .to_string()
}
