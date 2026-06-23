//! Batch Task Manager

use crate::api::core::{BatchConfig, BatchOperation};
use crate::api::core::{CoreError, CoreResult};
use crate::api::server::batch::types::*;
use crate::core::types::VertexId;
use crate::core::{Edge, Value, Vertex};
use crate::storage::StorageClient;
use dashmap::DashMap;
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
            let mut task = self.tasks.get_mut(batch_id).expect("Task should exist");
            task.update_status(BatchStatus::Running);
        }

        // Get all buffered items
        let items = {
            let mut task = self.tasks.get_mut(batch_id).expect("Task should exist");
            task.take_buffered_items()
        };

        // Perform batch insertion using core API
        let result = self.process_items(items, space_name).await;

        // Update task status and results
        {
            let mut task = self.tasks.get_mut(batch_id).expect("Task should exist");

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

    /// Processing of batch items using core API
    async fn process_items(
        &self,
        items: Vec<BatchItem>,
        space_name: &str,
    ) -> CoreResult<BatchResultData> {
        // Convert BatchItem to core BatchItem
        let core_items: Vec<crate::api::core::BatchItem> = items
            .into_iter()
            .filter_map(|item| self.convert_to_core_item(item))
            .collect();

        // Create batch operation with core API
        let config = BatchConfig::new().with_continue_on_error(true);
        let mut operation = BatchOperation::new(config);
        operation.add_items(core_items);

        // Execute batch operation
        let mut storage = self.storage.write();
        let core_result = operation.execute_sync(&mut *storage, space_name)?;

        // Convert core result to server result
        Ok(BatchResultData {
            vertices_inserted: core_result.vertices_inserted,
            edges_inserted: core_result.edges_inserted,
            errors: core_result
                .errors
                .into_iter()
                .map(|e| BatchErrorData {
                    index: e.index,
                    item_type: match e.item_type {
                        crate::api::core::BatchItemType::Vertex => BatchItemType::Vertex,
                        crate::api::core::BatchItemType::Edge => BatchItemType::Edge,
                    },
                    error: e.message,
                })
                .collect(),
        })
    }

    /// Convert server BatchItem to core BatchItem
    fn convert_to_core_item(&self, item: BatchItem) -> Option<crate::api::core::BatchItem> {
        match item {
            BatchItem::Vertex(data) => self
                .convert_vertex_data(data)
                .map(crate::api::core::BatchItem::Vertex),
            BatchItem::Edge(data) => self
                .convert_edge_data(data)
                .map(crate::api::core::BatchItem::Edge),
        }
    }

    fn convert_vertex_data(&self, data: VertexData) -> Option<Vertex> {
        let vid_value = json_to_value(data.vid)?;
        let vid = value_to_vertex_id(&vid_value)?;

        let tags: Vec<crate::core::vertex_edge_path::Tag> = data
            .tags
            .into_iter()
            .map(|name| {
                crate::core::vertex_edge_path::Tag::new(name, std::collections::HashMap::new())
            })
            .collect();

        let properties: std::collections::HashMap<String, Value> = data
            .properties
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (k, val)))
            .collect();

        Some(Vertex::new_with_properties(vid, tags, properties))
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
}

fn value_to_vertex_id(value: &Value) -> Option<VertexId> {
    match value {
        Value::Int(i) => Some(VertexId::from_int64(*i as i64)),
        Value::BigInt(i) => Some(VertexId::from_int64(*i)),
        Value::String(s) => Some(VertexId::from_string(s)),
        _ => None,
    }
}

fn json_to_value(json: serde_json::Value) -> Option<Value> {
    match json {
        serde_json::Value::Null => Some(Value::Null(crate::core::NullType::Null)),
        serde_json::Value::Bool(b) => Some(Value::Bool(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Some(Value::BigInt(i))
            } else {
                n.as_f64().map(Value::Double)
            }
        }
        serde_json::Value::String(s) => Some(Value::String(s)),
        serde_json::Value::Array(arr) => {
            let values: Vec<Value> = arr.into_iter().filter_map(json_to_value).collect();
            Some(Value::list(crate::core::value::List::from(values)))
        }
        serde_json::Value::Object(_) => None, // Object types are not supported
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
            Some(Value::Null(crate::core::NullType::Null))
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
            Some(Value::String("hello".to_string()))
        );
    }
}
