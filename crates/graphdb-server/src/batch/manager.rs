//! Batch Task Manager

use crate::batch::types::*;
use crate::storage::StorageClient;
use dashmap::DashMap;
use graphdb_api::api_core::{BatchConfig, BatchOperation};
use graphdb_api::api_core::{CoreError, CoreResult};
use graphdb_core::types::VertexId;
use graphdb_core::{Edge, Value, Vertex};
use parking_lot::RwLock;
use std::sync::Arc;
use uuid::Uuid;

/// Batch Task Manager
pub struct BatchManager<S: StorageClient + Clone + 'static> {
    /// Store all batch jobs
    tasks: Arc<DashMap<BatchId, BatchTask>>,
    /// Storage Client
    storage: Arc<RwLock<S>>,
}

impl<S: StorageClient + Clone + 'static> BatchManager<S> {
    /// Creating a new batch task manager
    pub fn new(storage: Arc<RwLock<S>>) -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            storage,
        }
    }

    /// Creating Batch Tasks
    pub fn create_task(
        &self,
        space_id: u64,
        batch_type: BatchType,
        batch_size: usize,
    ) -> CoreResult<BatchTask> {
        let batch_id = Uuid::new_v4().to_string();
        let task = BatchTask::new(batch_id.clone(), space_id, batch_type, batch_size);

        self.tasks.insert(batch_id.clone(), task.clone());

        Ok(task)
    }

    /// Get Batch Tasks
    pub fn get_task(&self, batch_id: &str) -> Option<BatchTask> {
        self.tasks.get(batch_id).map(|t| t.clone())
    }

    /// Adding Batch Items
    pub fn add_items(&self, batch_id: &str, items: Vec<BatchItem>) -> CoreResult<usize> {
        let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;

        if task.status != BatchStatus::Created {
            return Err(CoreError::InvalidParameter(format!(
                "Incorrect batch task status: {:?}",
                task.status
            )));
        }

        let count = task.add_items(items);
        Ok(count)
    }

    /// Perform batch tasks
    pub async fn execute_task(
        &self,
        batch_id: &str,
        space_name: &str,
    ) -> CoreResult<BatchResultData> {
        let task = self.tasks.get(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;

        if task.status != BatchStatus::Created {
            return Err(CoreError::InvalidParameter(format!(
                "Incorrect batch task status: {:?}",
                task.status
            )));
        }

        // Update status to running
        {
            let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
                CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
            })?;
            task.update_status(BatchStatus::Running);
        }

        // Get all buffered items
        let items = {
            let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
                CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
            })?;
            task.take_buffered_items()
        };

        // Perform batch insertion using core API
        let result = self.process_items(items, space_name).await;

        // Update task status and results
        {
            let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
                CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
            })?;

            match &result {
                Ok(data) => {
                    let status = if data.errors.is_empty() {
                        BatchStatus::Completed
                    } else {
                        BatchStatus::Failed
                    };
                    task.update_status(status);
                    task.set_result(data.clone());
                }
                Err(e) => {
                    task.update_status(BatchStatus::Failed);
                    task.set_result(BatchResultData {
                        vertices_inserted: 0,
                        edges_inserted: 0,
                        vertices_updated: 0,
                        edges_updated: 0,
                        vertices_deleted: 0,
                        edges_deleted: 0,
                        errors: vec![BatchErrorData {
                            index: 0,
                            item_type: BatchItemType::Vertex,
                            error: e.to_string(),
                        }],
                    });
                }
            }
        }

        result
    }

    /// Cancel Batch Tasks
    pub fn cancel_task(&self, batch_id: &str) -> CoreResult<()> {
        let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;

        match task.status {
            BatchStatus::Created | BatchStatus::Running => {
                task.update_status(BatchStatus::Cancelled);
                Ok(())
            }
            _ => Err(CoreError::InvalidParameter(format!(
                "Unable to cancel tasks with status {:?}",
                task.status
            ))),
        }
    }

    /// Delete Batch Tasks
    pub fn remove_task(&self, batch_id: &str) -> CoreResult<()> {
        self.tasks.remove(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;
        Ok(())
    }

    /// Processing of batch items using core API for inserts and direct
    /// storage writer calls for updates/deletes (both continue on error).
    async fn process_items(
        &self,
        items: Vec<BatchItem>,
        space_name: &str,
    ) -> CoreResult<BatchResultData> {
        // Partition by kind, keeping original indices for error reporting.
        let mut insert_items: Vec<graphdb_api::api_core::BatchItem> = Vec::new();
        let mut mutations: Vec<(usize, Mutation)> = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            match item {
                BatchItem::Vertex(data) => {
                    if let Some(core) = self
                        .convert_vertex_data(data)
                        .map(graphdb_api::api_core::BatchItem::Vertex)
                    {
                        insert_items.push(core);
                    }
                }
                BatchItem::Edge(data) => {
                    if let Some(core) = self
                        .convert_edge_data(data)
                        .map(graphdb_api::api_core::BatchItem::Edge)
                    {
                        insert_items.push(core);
                    }
                }
                BatchItem::UpdateVertex(data) => match self.convert_vertex_data(data) {
                    Some(vertex) => mutations.push((index, Mutation::UpdateVertex(vertex))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid("update_vertex has an invalid vid or empty tag".into()),
                    )),
                },
                BatchItem::UpdateEdgeData(data) => match self.convert_update_edge_data(&data) {
                    Some(edge) => mutations.push((index, Mutation::UpdateEdge(edge))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid("update_edge has invalid endpoint ids".into()),
                    )),
                },
                BatchItem::DeleteVertex(data) => match self.convert_delete_vertex_data(&data) {
                    Some(ops) => {
                        if ops.is_empty() {
                            mutations.push((
                                index,
                                Mutation::Invalid(
                                    "delete_vertex requires at least one tag_name".into(),
                                ),
                            ));
                        } else {
                            for op in ops {
                                mutations.push((index, op));
                            }
                        }
                    }
                    None => mutations.push((
                        index,
                        Mutation::Invalid("delete_vertex has an invalid vid".into()),
                    )),
                },
                BatchItem::DeleteEdgeData(data) => match self.convert_delete_edge_data(&data) {
                    Some(op) => mutations.push((index, op)),
                    None => mutations.push((
                        index,
                        Mutation::Invalid("delete_edge has invalid endpoint ids".into()),
                    )),
                },
            }
        }

        // Inserts go through the core batch operation.
        let config = BatchConfig::new().with_continue_on_error(true);
        let mut operation = BatchOperation::new(config);
        operation.add_items(insert_items);

        // Execute batch operation
        let mut storage = self.storage.write();
        let core_result = operation.execute_sync(&mut *storage, space_name)?;

        // Updates/deletes run against the same storage handle.
        let mut mutations_result = MutationCounts::default();
        let mut mutation_errors: Vec<BatchErrorData> = Vec::new();
        for (index, mutation) in mutations {
            let item_type = mutation.item_type();
            if let Err(error) =
                self.apply_mutation(&mut storage, space_name, mutation, &mut mutations_result)
            {
                mutation_errors.push(BatchErrorData {
                    index,
                    item_type,
                    error,
                });
            }
        }
        drop(storage);

        // Convert core result to server result
        Ok(BatchResultData {
            vertices_inserted: core_result.vertices_inserted,
            edges_inserted: core_result.edges_inserted,
            vertices_updated: mutations_result.vertices_updated,
            edges_updated: mutations_result.edges_updated,
            vertices_deleted: mutations_result.vertices_deleted,
            edges_deleted: mutations_result.edges_deleted,
            errors: core_result
                .errors
                .into_iter()
                .map(|e| BatchErrorData {
                    index: e.index,
                    item_type: match e.item_type {
                        graphdb_api::api_core::BatchItemType::Vertex => BatchItemType::Vertex,
                        graphdb_api::api_core::BatchItemType::Edge => BatchItemType::Edge,
                    },
                    error: e.message,
                })
                .chain(mutation_errors)
                .collect(),
        })
    }

    /// Apply one update/delete mutation, bumping the matching counter.
    fn apply_mutation(
        &self,
        storage: &mut S,
        space_name: &str,
        mutation: Mutation,
        counts: &mut MutationCounts,
    ) -> Result<(), String> {
        match mutation {
            Mutation::UpdateVertex(vertex) => storage
                .update_vertex(space_name, vertex)
                .map(|()| counts.vertices_updated += 1)
                .map_err(|e| format!("update vertex failed: {e}")),
            Mutation::UpdateEdge(edge) => storage
                .update_edge(space_name, edge)
                .map(|()| counts.edges_updated += 1)
                .map_err(|e| format!("update edge failed: {e}")),
            Mutation::DeleteVertex { tag, id } => storage
                .delete_vertex_with_edges(space_name, &tag, &id)
                .map(|()| counts.vertices_deleted += 1)
                .map_err(|e| format!("delete vertex failed: {e}")),
            Mutation::DeleteEdge {
                edge_type,
                src,
                dst,
                rank,
            } => storage
                .delete_edge(space_name, &src, &dst, &edge_type, rank)
                .map(|()| counts.edges_deleted += 1)
                .map_err(|e| format!("delete edge failed: {e}")),
            Mutation::Invalid(reason) => Err(reason),
        }
    }

    fn convert_vertex_data(&self, data: VertexData) -> Option<Vertex> {
        let vid_value = json_to_value(data.vid)?;
        let vid = value_to_vertex_id(&vid_value)?;

        if data.tag.is_empty() {
            return None;
        }

        let properties: std::collections::HashMap<String, Value> = data
            .properties
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (k, val)))
            .collect();

        Some(Vertex::new(
            vid,
            graphdb_core::vertex_edge_path::Tag::new(data.tag, properties),
        ))
    }

    fn convert_edge_data(&self, data: EdgeData) -> Option<Edge> {
        let src_vid_value = json_to_value(data.src_vid)?;
        let dst_vid_value = json_to_value(data.dst_vid)?;
        let src_vid = value_to_vertex_id(&src_vid_value)?;
        let dst_vid = value_to_vertex_id(&dst_vid_value)?;

        let props: std::collections::HashMap<String, Value> = data
            .properties
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (k, val)))
            .collect();

        Some(Edge::new(src_vid, dst_vid, data.edge_type, 0, props))
    }

    fn convert_update_edge_data(&self, data: &UpdateEdgeData) -> Option<Edge> {
        let src_vid = value_to_vertex_id(&json_to_value(data.src_vid.clone())?)?;
        let dst_vid = value_to_vertex_id(&json_to_value(data.dst_vid.clone())?)?;
        let props: std::collections::HashMap<String, Value> = data
            .properties
            .clone()
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (k, val)))
            .collect();
        let mut edge = Edge::new(src_vid, dst_vid, data.edge_type.clone(), 0, props);
        edge.ranking = data.rank;
        Some(edge)
    }

    /// One delete-vertex item fans out to one mutation per tag.
    fn convert_delete_vertex_data(&self, data: &DeleteVertexData) -> Option<Vec<Mutation>> {
        let id = value_to_vertex_id(&json_to_value(data.vid.clone())?)?;
        Some(
            data.tag_names
                .iter()
                .cloned()
                .map(|tag| Mutation::DeleteVertex { tag, id })
                .collect(),
        )
    }

    fn convert_delete_edge_data(&self, data: &DeleteEdgeData) -> Option<Mutation> {
        let src = value_to_vertex_id(&json_to_value(data.src_vid.clone())?)?;
        let dst = value_to_vertex_id(&json_to_value(data.dst_vid.clone())?)?;
        Some(Mutation::DeleteEdge {
            edge_type: data.edge_type.clone(),
            src,
            dst,
            rank: data.rank,
        })
    }
}

/// One buffered update/delete item with its storage call arguments resolved.
enum Mutation {
    UpdateVertex(Vertex),
    UpdateEdge(Edge),
    DeleteVertex {
        tag: String,
        id: VertexId,
    },
    DeleteEdge {
        edge_type: String,
        src: VertexId,
        dst: VertexId,
        rank: i64,
    },
    Invalid(String),
}

impl Mutation {
    fn item_type(&self) -> BatchItemType {
        match self {
            Mutation::UpdateVertex(_) | Mutation::UpdateEdge(_) => BatchItemType::Update,
            Mutation::DeleteVertex { .. } | Mutation::DeleteEdge { .. } => BatchItemType::Delete,
            Mutation::Invalid(reason) if reason.starts_with("delete") => BatchItemType::Delete,
            Mutation::Invalid(_) => BatchItemType::Update,
        }
    }
}

/// Counters for applied update/delete mutations.
#[derive(Default)]
struct MutationCounts {
    vertices_updated: usize,
    edges_updated: usize,
    vertices_deleted: usize,
    edges_deleted: usize,
}

fn value_to_vertex_id(value: &Value) -> Option<VertexId> {
    match value {
        Value::Int(i) => VertexId::try_from_int64(*i as i64).ok(),
        Value::BigInt(i) => VertexId::try_from_int64(*i).ok(),
        Value::String(s) => VertexId::try_from_string(s.as_str()).ok(),
        _ => None,
    }
}

fn json_to_value(json: serde_json::Value) -> Option<Value> {
    match json {
        serde_json::Value::Null => Some(Value::Null(graphdb_core::NullType::Null)),
        serde_json::Value::Bool(b) => Some(Value::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::BigInt(i))
            } else {
                n.as_f64().map(Value::Double)
            }
        }
        serde_json::Value::String(s) => Some(Value::string(s)),
        serde_json::Value::Array(arr) => {
            let values: Vec<Value> = arr.into_iter().filter_map(json_to_value).collect();
            Some(Value::list(graphdb_core::value::List::from(values)))
        }
        serde_json::Value::Object(data) => {
            let obj = serde_json::Value::Object(data);
            graphdb_core::value::Json::parse(&obj.to_string())
                .ok()
                .map(|j| Value::Json(Box::new(j)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_to_value() {
        // Test null
        assert_eq!(
            json_to_value(serde_json::Value::Null),
            Some(Value::Null(graphdb_core::NullType::Null))
        );

        // Test bool
        assert_eq!(
            json_to_value(serde_json::Value::Bool(true)),
            Some(Value::Bool(true))
        );

        // Test number
        assert_eq!(
            json_to_value(serde_json::json!(42)),
            Some(Value::BigInt(42))
        );

        // Test string
        assert_eq!(
            json_to_value(serde_json::json!("hello")),
            Some(Value::string("hello"))
        );
    }
}
