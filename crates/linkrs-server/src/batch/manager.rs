//! Batch Task Manager

use crate::batch::types::*;
use crate::storage::StorageClient;
use dashmap::DashMap;
use linkrs_api::api_core::{
    BulkInsertConfig, BulkInsertItem, BulkInsertItemType, BulkInsertOperation,
};
use linkrs_api::api_core::{CoreError, CoreResult};
use linkrs_core::types::VertexId;
use linkrs_core::{Edge, Value, Vertex};
use parking_lot::RwLock;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// Retention and capacity limits for batch tasks.
#[derive(Debug, Clone)]
pub struct BatchManagerConfig {
    /// How long finished tasks stay queryable, in seconds.
    pub task_ttl_secs: u64,
    /// Maximum live tasks; the oldest finished task is evicted past this.
    pub max_tasks: usize,
}

impl Default for BatchManagerConfig {
    fn default() -> Self {
        Self {
            task_ttl_secs: 3600,
            max_tasks: 1024,
        }
    }
}

/// Batch Task Manager
pub struct BatchManager<S: StorageClient + Clone + 'static> {
    /// Store all batch jobs
    tasks: Arc<DashMap<BatchId, BatchTask>>,
    /// Storage Client
    storage: Arc<RwLock<S>>,
    /// Retention and capacity limits
    config: BatchManagerConfig,
    /// Optional directory for task snapshots; unset disables persistence.
    snapshot_dir: RwLock<Option<PathBuf>>,
}

impl<S: StorageClient + Clone + 'static> BatchManager<S> {
    /// Creating a new batch task manager
    pub fn new(storage: Arc<RwLock<S>>) -> Self {
        Self::new_with_config(storage, BatchManagerConfig::default())
    }

    /// Creating a new batch task manager with explicit limits
    pub fn new_with_config(storage: Arc<RwLock<S>>, config: BatchManagerConfig) -> Self {
        Self {
            tasks: Arc::new(DashMap::new()),
            storage,
            config,
            snapshot_dir: RwLock::new(None),
        }
    }

    /// Enable disk snapshots under `dir`, creating it when missing.
    pub fn set_snapshot_dir(&self, dir: PathBuf) -> CoreResult<()> {
        std::fs::create_dir_all(&dir)
            .map_err(|e| CoreError::Internal(format!("Cannot create batch snapshot dir: {e}")))?;
        *self.snapshot_dir.write() = Some(dir);
        Ok(())
    }

    fn snapshot_path(&self, batch_id: &str) -> Option<PathBuf> {
        self.snapshot_dir
            .read()
            .as_ref()
            .map(|dir| dir.join(format!("{batch_id}.json")))
    }

    fn save_snapshot(&self, task: &BatchTask) -> CoreResult<()> {
        let Some(path) = self.snapshot_path(&task.id) else {
            return Ok(());
        };
        let data = serde_json::to_string(task)
            .map_err(|e| CoreError::Internal(format!("Cannot serialize batch task: {e}")))?;
        std::fs::write(&path, data)
            .map_err(|e| CoreError::Internal(format!("Cannot write batch snapshot: {e}")))?;
        Ok(())
    }

    fn delete_snapshot(&self, batch_id: &str) -> CoreResult<()> {
        let Some(path) = self.snapshot_path(batch_id) else {
            return Ok(());
        };
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::Internal(format!(
                "Cannot delete batch snapshot: {e}"
            ))),
        }
    }

    /// Reload persisted tasks, marking tasks that were running at shutdown
    /// as failed. Corrupt files are quarantined beside the original and
    /// skipped so one bad file cannot refuse startup.
    pub fn load_snapshots(&self) -> CoreResult<usize> {
        let dir = self.snapshot_dir.read().clone();
        let Some(dir) = dir else {
            return Ok(0);
        };
        let entries = std::fs::read_dir(&dir)
            .map_err(|e| CoreError::Internal(format!("Cannot list batch snapshots: {e}")))?;
        let mut loaded = 0;
        for entry in entries {
            let path = entry
                .map_err(|e| {
                    CoreError::Internal(format!("Cannot read batch snapshot entry: {e}"))
                })?
                .path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let data = std::fs::read(&path)
                .map_err(|e| CoreError::Internal(format!("Cannot read batch snapshot: {e}")))?;
            match serde_json::from_slice::<BatchTask>(&data) {
                Ok(mut task) => {
                    if task.status == BatchStatus::Running {
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
                                error: "batch interrupted by server restart".to_string(),
                            }],
                        });
                    }
                    self.tasks.insert(task.id.clone(), task);
                    loaded += 1;
                }
                Err(_) => {
                    let mut quarantined = path.clone();
                    quarantined.set_extension("corrupt");
                    let _ = std::fs::rename(&path, &quarantined);
                }
            }
        }
        Ok(loaded)
    }

    /// Drop finished tasks older than the configured retention window.
    pub fn reap_expired(&self) -> CoreResult<usize> {
        let cutoff =
            chrono::Utc::now() - chrono::Duration::seconds(self.config.task_ttl_secs as i64);
        let stale: Vec<BatchId> = self
            .tasks
            .iter()
            .filter(|entry| {
                matches!(
                    entry.value().status,
                    BatchStatus::Completed | BatchStatus::Failed | BatchStatus::Cancelled
                ) && entry.value().updated_at < cutoff
            })
            .map(|entry| entry.key().clone())
            .collect();
        for id in &stale {
            self.tasks.remove(id);
            self.delete_snapshot(id)?;
        }
        Ok(stale.len())
    }

    /// Make room for one more task by evicting the oldest finished task.
    /// Fails when every live task is still unfinished.
    fn evict_oldest_finished(&self) -> CoreResult<()> {
        if self.tasks.len() < self.config.max_tasks.max(1) {
            return Ok(());
        }
        let oldest = self
            .tasks
            .iter()
            .filter(|entry| {
                matches!(
                    entry.value().status,
                    BatchStatus::Completed | BatchStatus::Failed | BatchStatus::Cancelled
                )
            })
            .min_by_key(|entry| entry.value().updated_at)
            .map(|entry| entry.key().clone());
        match oldest {
            Some(id) => {
                self.tasks.remove(&id);
                self.delete_snapshot(&id)?;
                Ok(())
            }
            None => Err(CoreError::Internal("too many batch tasks".to_string())),
        }
    }

    /// Creating Batch Tasks
    pub fn create_task(
        &self,
        space_id: u64,
        batch_type: BatchType,
        batch_size: usize,
    ) -> CoreResult<BatchTask> {
        self.reap_expired()?;
        self.evict_oldest_finished()?;

        let batch_id = Uuid::new_v4().to_string();
        let task = BatchTask::new(batch_id.clone(), space_id, batch_type, batch_size.max(1));

        self.save_snapshot(&task)?;

        self.tasks.insert(batch_id.clone(), task.clone());

        Ok(task)
    }

    /// Get Batch Tasks
    pub fn get_task(&self, batch_id: &str) -> Option<BatchTask> {
        self.tasks.get(batch_id).map(|t| t.clone())
    }

    /// Adding Batch Items
    pub fn add_items(&self, batch_id: &str, items: Vec<BatchItem>) -> CoreResult<usize> {
        let snapshot = {
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
            (count, task.value().clone())
        };
        self.save_snapshot(&snapshot.1)?;
        Ok(snapshot.0)
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

        // Get all buffered items alongside the configured shard size
        let (items, batch_size) = {
            let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
                CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
            })?;
            (task.take_buffered_items(), task.batch_size)
        };

        // Perform batch insertion using core API; the task id scopes
        // cooperative cancellation checks between shards.
        let result = self
            .execute_items(space_name, items, batch_size, Some(batch_id))
            .await;

        // Update task status and results; a concurrent cancel wins over the
        // outcome and keeps the Cancelled status with partial counts.
        let snapshot = {
            let mut task = self.tasks.get_mut(batch_id).ok_or_else(|| {
                CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
            })?;
            let cancelled = task.status == BatchStatus::Cancelled;

            match &result {
                Ok(data) => {
                    if !cancelled {
                        let status = if data.errors.is_empty() {
                            BatchStatus::Completed
                        } else {
                            BatchStatus::Failed
                        };
                        task.update_status(status);
                    }
                    let succeeded = data.vertices_inserted
                        + data.edges_inserted
                        + data.vertices_updated
                        + data.edges_updated
                        + data.vertices_deleted
                        + data.edges_deleted;
                    task.update_progress(succeeded, data.errors.len());
                    task.set_result(data.clone());
                }
                Err(e) => {
                    if !cancelled {
                        task.update_status(BatchStatus::Failed);
                    }
                    let total = task.progress.total;
                    task.update_progress(0, total);
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
            task.value().clone()
        };
        self.save_snapshot(&snapshot)?;

        result
    }

    /// Cancel Batch Tasks
    ///
    /// Cancellation is cooperative: a running execution stops between
    /// shards and keeps the Cancelled status with partial counts.
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
    ///
    /// A running task still owns its execution outcome, so deletion is
    /// refused while it runs; cancel it first.
    pub fn remove_task(&self, batch_id: &str) -> CoreResult<()> {
        let running = self.tasks.get(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;
        if running.status == BatchStatus::Running {
            return Err(CoreError::InvalidParameter(format!(
                "Cannot delete running batch task: {}",
                batch_id
            )));
        }
        drop(running);
        self.tasks.remove(batch_id).ok_or_else(|| {
            CoreError::InvalidParameter(format!("Batch task does not exist: {}", batch_id))
        })?;
        self.delete_snapshot(batch_id)?;
        Ok(())
    }

    /// Execute batch items without a task lifecycle: inserts go through the
    /// core bulk insert path in shards, updates/deletes run as direct
    /// storage calls in shard-sized groups, all continue on error. When
    /// `cancel_scope` names a task, execution stops between shards once
    /// that task is cancelled, returning partial counts.
    pub(crate) async fn execute_items(
        &self,
        space_name: &str,
        items: Vec<BatchItem>,
        batch_size: usize,
        cancel_scope: Option<&str>,
    ) -> CoreResult<BatchResultData> {
        // Partition by kind, keeping original indices for error reporting.
        let mut insert_items: Vec<(usize, BulkInsertItem)> = Vec::new();
        let mut mutations: Vec<(usize, Mutation)> = Vec::new();
        for (index, item) in items.into_iter().enumerate() {
            match item {
                BatchItem::Vertex(data) => match self.convert_vertex_data(data) {
                    Some(core) => insert_items.push((index, BulkInsertItem::Vertex(core))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid(
                            BatchItemType::Vertex,
                            "vertex has an invalid vid or empty tag".into(),
                        ),
                    )),
                },
                BatchItem::Edge(data) => match self.convert_edge_data(data) {
                    Some(core) => insert_items.push((index, BulkInsertItem::Edge(core))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid(
                            BatchItemType::Edge,
                            "edge has invalid endpoint ids".into(),
                        ),
                    )),
                }
                BatchItem::UpdateVertex(data) => match self.convert_vertex_data(data) {
                    Some(vertex) => mutations.push((index, Mutation::UpdateVertex(vertex))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid(
                            BatchItemType::Update,
                            "update_vertex has an invalid vid or empty tag".into(),
                        ),
                    )),
                },
                BatchItem::UpdateEdgeData(data) => match self.convert_update_edge_data(&data) {
                    Some(edge) => mutations.push((index, Mutation::UpdateEdge(edge))),
                    None => mutations.push((
                        index,
                        Mutation::Invalid(
                            BatchItemType::Update,
                            "update_edge has invalid endpoint ids".into(),
                        ),
                    )),
                },
                BatchItem::DeleteVertex(data) => match self.convert_delete_vertex_data(&data) {
                    Some(ops) => {
                        if ops.is_empty() {
                            mutations.push((
                                index,
                                Mutation::Invalid(
                                    BatchItemType::Delete,
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
                        Mutation::Invalid(
                            BatchItemType::Delete,
                            "delete_vertex has an invalid vid".into(),
                        ),
                    )),
                },
                BatchItem::DeleteEdgeData(data) => match self.convert_delete_edge_data(&data) {
                    Some(op) => mutations.push((index, op)),
                    None => mutations.push((
                        index,
                        Mutation::Invalid(
                            BatchItemType::Delete,
                            "delete_edge has invalid endpoint ids".into(),
                        ),
                    )),
                },
            }
        }

        let cancelled = |scope: Option<&str>| match scope {
            Some(id) => self
                .tasks
                .get(id)
                .is_some_and(|task| task.status == BatchStatus::Cancelled),
            None => false,
        };

        // Inserts go through the core bulk insert path in shards of the
        // configured batch size, keeping peak memory and the write hold
        // bounded. The core path reports one index per kind chunk, so each
        // shard's errors rebase onto the shard's first global index. The
        // storage guard is held per shard so cancellation and other writers
        // are not blocked by the whole run.
        let shard = batch_size.max(1);
        let mut vertices_inserted = 0usize;
        let mut edges_inserted = 0usize;
        let mut insert_errors: Vec<BatchErrorData> = Vec::new();
        let mut shard_start = 0;
        while shard_start < insert_items.len() {
            if cancelled(cancel_scope) {
                break;
            }
            let shard_end = (shard_start + shard).min(insert_items.len());
            let chunk = &insert_items[shard_start..shard_end];
            let base = chunk.first().map(|(index, _)| *index).unwrap_or(0);
            let config = BulkInsertConfig::new().with_continue_on_error(true);
            let mut operation = BulkInsertOperation::new(config);
            operation.add_items(
                chunk
                    .iter()
                    .map(|(_, item)| item.clone())
                    .collect(),
            );
            let mut storage = self.storage.write();
            let core_result = operation.execute_sync(&mut *storage, space_name)?;
            drop(storage);
            vertices_inserted += core_result.vertices_inserted;
            edges_inserted += core_result.edges_inserted;
            insert_errors.extend(core_result.errors.into_iter().map(|e| BatchErrorData {
                index: base + e.index,
                item_type: match e.item_type {
                    BulkInsertItemType::Vertex => BatchItemType::Vertex,
                    BulkInsertItemType::Edge => BatchItemType::Edge,
                },
                error: e.message,
            }));
            shard_start = shard_end;
        }

        // Updates/deletes run as direct storage calls in shard-sized groups
        // with the same cancellation checks between groups.
        let mut mutations_result = MutationCounts::default();
        let mut mutation_errors: Vec<BatchErrorData> = Vec::new();
        let mut pending = mutations;
        while !pending.is_empty() {
            if cancelled(cancel_scope) {
                break;
            }
            let take = shard.min(pending.len());
            let group: Vec<(usize, Mutation)> = pending.drain(..take).collect();
            let mut storage = self.storage.write();
            for (index, mutation) in group {
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
        }

        // Convert core result to server result
        Ok(BatchResultData {
            vertices_inserted,
            edges_inserted,
            vertices_updated: mutations_result.vertices_updated,
            edges_updated: mutations_result.edges_updated,
            vertices_deleted: mutations_result.vertices_deleted,
            edges_deleted: mutations_result.edges_deleted,
            errors: insert_errors.into_iter().chain(mutation_errors).collect(),
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
            Mutation::Invalid(_, reason) => Err(reason),
        }
    }

    fn convert_vertex_data(&self, data: VertexData) -> Option<Vertex> {
        let vid_value = json_to_value(data.vid)?;
        let vid = value_to_vertex_id(&vid_value)?;

        if data.tag.is_empty() {
            return None;
        }

        let properties: std::collections::HashMap<Arc<str>, Value> = data
            .properties
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (Arc::from(k.as_str()), val)))
            .collect();

        Some(Vertex::new(
            vid,
            linkrs_core::vertex_edge_path::Tag::new(data.tag, properties),
        ))
    }

    fn convert_edge_data(&self, data: EdgeData) -> Option<Edge> {
        let src_vid_value = json_to_value(data.src_vid)?;
        let dst_vid_value = json_to_value(data.dst_vid)?;
        let src_vid = value_to_vertex_id(&src_vid_value)?;
        let dst_vid = value_to_vertex_id(&dst_vid_value)?;

        let props: std::collections::HashMap<Arc<str>, Value> = data
            .properties
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (Arc::from(k.as_str()), val)))
            .collect();

        Some(Edge::new(src_vid, dst_vid, data.edge_type, 0, props))
    }

    fn convert_update_edge_data(&self, data: &UpdateEdgeData) -> Option<Edge> {
        let src_vid = value_to_vertex_id(&json_to_value(data.src_vid.clone())?)?;
        let dst_vid = value_to_vertex_id(&json_to_value(data.dst_vid.clone())?)?;
        let props: std::collections::HashMap<Arc<str>, Value> = data
            .properties
            .clone()
            .into_iter()
            .filter_map(|(k, v)| json_to_value(v).map(|val| (Arc::from(k.as_str()), val)))
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
    Invalid(BatchItemType, String),
}

impl Mutation {
    fn item_type(&self) -> BatchItemType {
        match self {
            Mutation::UpdateVertex(_) | Mutation::UpdateEdge(_) => BatchItemType::Update,
            Mutation::DeleteVertex { .. } | Mutation::DeleteEdge { .. } => BatchItemType::Delete,
            Mutation::Invalid(item_type, _) => *item_type,
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
        serde_json::Value::Null => Some(Value::Null(linkrs_core::NullType::Null)),
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
            Some(Value::list(linkrs_core::value::List::from(values)))
        }
        serde_json::Value::Object(data) => {
            let obj = serde_json::Value::Object(data);
            linkrs_core::value::Json::parse(&obj.to_string())
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
            Some(Value::Null(linkrs_core::NullType::Null))
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
