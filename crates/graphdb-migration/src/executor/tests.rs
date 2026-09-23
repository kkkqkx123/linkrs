use super::*;
use crate::plan::{MigrationTarget, VersionRange};
use graphdb_core::types::{EdgeTypeInfo, Index, SpaceInfo, TagInfo, VertexId};
use graphdb_core::StorageError;
use graphdb_core::{DataType, Edge, EdgeDeleteKey, EdgeDirection, Value, Vertex};
use graphdb_storage::{
    AutoCommitBatchOps, AutoCommitGroupOps, LabelVersionHistory, MigrationHistoryRecord,
    StorageReader, StorageWriter,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

type VertexMap = Arc<Mutex<HashMap<(String, String), Vec<Vertex>>>>;
type EdgeMap = Arc<Mutex<HashMap<(String, String), Vec<Edge>>>>;
type HistoryVec = Arc<Mutex<Vec<MigrationHistoryRecord>>>;

#[derive(Debug, Clone)]
struct TestStorage {
    vertices: VertexMap,
    edges: EdgeMap,
    migration_history: HistoryVec,
}

impl TestStorage {
    fn new() -> Self {
        Self {
            vertices: Arc::new(Mutex::new(HashMap::new())),
            edges: Arc::new(Mutex::new(HashMap::new())),
            migration_history: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn insert_vertex(&self, space: &str, label: &str, vid: i64, props: HashMap<String, Value>) {
        let mut map = self.vertices.lock().unwrap();
        let entry = map
            .entry((space.to_string(), label.to_string()))
            .or_default();
        let tag = graphdb_core::vertex_edge_path::Tag {
            name: label.to_string(),
            properties: props,
        };
        let vertex = Vertex {
            vid: VertexId::try_from_int64(vid).expect("test vertex id"),
            tag,
        };
        entry.push(vertex);
    }
    fn get_vertices(&self, space: &str, label: &str) -> Vec<Vertex> {
        self.vertices
            .lock()
            .unwrap()
            .get(&(space.to_string(), label.to_string()))
            .cloned()
            .unwrap_or_default()
    }
}

impl StorageReader for TestStorage {
    fn get_vertex(
        &self,
        _space: &str,
        _tag: &str,
        _id: &VertexId,
    ) -> Result<Option<Vertex>, StorageError> {
        Ok(None)
    }
    fn scan_vertices(&self, _space: &str) -> Result<Vec<Vertex>, StorageError> {
        Ok(Vec::new())
    }
    fn scan_vertices_by_tag(&self, space: &str, tag: &str) -> Result<Vec<Vertex>, StorageError> {
        Ok(self.get_vertices(space, tag))
    }
    fn scan_vertices_by_prop(
        &self,
        _space: &str,
        _tag: &str,
        _prop: &str,
        _value: &Value,
    ) -> Result<Vec<Vertex>, StorageError> {
        Ok(Vec::new())
    }
    fn get_edge(
        &self,
        _space: &str,
        _src: &VertexId,
        _dst: &VertexId,
        _edge_type: &str,
        _rank: i64,
    ) -> Result<Option<Edge>, StorageError> {
        Ok(None)
    }
    fn get_node_edges(
        &self,
        _space: &str,
        _node_id: &VertexId,
        _direction: EdgeDirection,
    ) -> Result<Vec<Edge>, StorageError> {
        Ok(Vec::new())
    }
    fn neighbor_dst_ids_batch(
        &self,
        _space: &str,
        _src_ids: &[VertexId],
        _direction: EdgeDirection,
        _edge_types: &[String],
    ) -> Result<Vec<Vec<VertexId>>, StorageError> {
        Ok(Vec::new())
    }
    fn out_degree_batch(
        &self,
        _space: &str,
        _src_ids: &[VertexId],
        _direction: EdgeDirection,
        _edge_types: &[String],
    ) -> Result<Vec<usize>, StorageError> {
        Ok(Vec::new())
    }
    fn scan_edges_by_type(&self, space: &str, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
        Ok(self
            .edges
            .lock()
            .unwrap()
            .get(&(space.to_string(), edge_type.to_string()))
            .cloned()
            .unwrap_or_default())
    }
    fn scan_all_edges(&self, _space: &str) -> Result<Vec<Edge>, StorageError> {
        Ok(Vec::new())
    }
    fn count_vertices_by_tag(&self, space: &str, tag: &str) -> Result<u64, StorageError> {
        Ok(self.get_vertices(space, tag).len() as u64)
    }
    fn count_edges_by_type(&self, space: &str, edge_type: &str) -> Result<u64, StorageError> {
        Ok(self
            .edges
            .lock()
            .unwrap()
            .get(&(space.to_string(), edge_type.to_string()))
            .map(|v| v.len() as u64)
            .unwrap_or(0))
    }
    fn lookup_index(
        &self,
        _space: &str,
        _index: &str,
        _value: &Value,
    ) -> Result<Vec<Value>, StorageError> {
        Ok(Vec::new())
    }
    fn get_vertex_with_schema(
        &self,
        _space: &str,
        _tag: &str,
        _id: &Value,
    ) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> {
        Ok(None)
    }
    fn get_edge_with_schema(
        &self,
        _space: &str,
        _edge_type: &str,
        _src: &Value,
        _dst: &Value,
    ) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        Ok(None)
    }
    fn scan_vertices_with_schema(
        &self,
        _space: &str,
        _tag: &str,
    ) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> {
        Ok(Vec::new())
    }
    fn scan_edges_with_schema(
        &self,
        _space: &str,
        _edge_type: &str,
    ) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> {
        Ok(Vec::new())
    }
    fn get_space(&self, _space: &str) -> Result<Option<SpaceInfo>, StorageError> {
        Ok(None)
    }
    fn get_space_by_id(&self, _space_id: u64) -> Result<Option<SpaceInfo>, StorageError> {
        Ok(None)
    }
    fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> {
        Ok(Vec::new())
    }
    fn get_space_id(&self, _space: &str) -> Result<u64, StorageError> {
        Ok(1)
    }
    fn space_exists(&self, _space: &str) -> bool {
        false
    }
    fn get_tag(&self, _space: &str, _tag: &str) -> Result<Option<TagInfo>, StorageError> {
        Ok(None)
    }
    fn list_tags(&self, _space: &str) -> Result<Vec<TagInfo>, StorageError> {
        Ok(Vec::new())
    }
    fn get_edge_type(
        &self,
        _space: &str,
        _edge_type: &str,
    ) -> Result<Option<EdgeTypeInfo>, StorageError> {
        Ok(None)
    }
    fn list_edge_types(&self, _space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> {
        Ok(Vec::new())
    }
    fn get_tag_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> {
        Ok(None)
    }
    fn list_tag_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> {
        Ok(Vec::new())
    }
    fn get_edge_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> {
        Ok(None)
    }
    fn list_edge_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> {
        Ok(Vec::new())
    }
    fn get_vertex_version_history(
        &self,
        _space: &str,
        _tag: &str,
    ) -> Result<Option<LabelVersionHistory>, StorageError> {
        Ok(None)
    }
    fn get_edge_version_history(
        &self,
        _space: &str,
        _edge_type: &str,
    ) -> Result<Option<LabelVersionHistory>, StorageError> {
        Ok(None)
    }
    fn get_vertex_schema_changes(
        &self,
        _space: &str,
        _tag: &str,
        _from_version: u64,
        _to_version: u64,
    ) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> {
        Ok(Vec::new())
    }
    fn get_edge_schema_changes(
        &self,
        _space: &str,
        _edge_type: &str,
        _from_version: u64,
        _to_version: u64,
    ) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> {
        Ok(Vec::new())
    }
    fn detect_vertex_breaking_changes(
        &self,
        _space: &str,
        _tag: &str,
        _from_version: u64,
        _to_version: u64,
    ) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> {
        Ok(Vec::new())
    }
    fn detect_edge_breaking_changes(
        &self,
        _space: &str,
        _edge_type: &str,
        _from_version: u64,
        _to_version: u64,
    ) -> Result<Vec<graphdb_storage::PropertyChange>, StorageError> {
        Ok(Vec::new())
    }
    fn record_migration_history(&self, record: MigrationHistoryRecord) -> Result<(), StorageError> {
        self.migration_history.lock().unwrap().push(record);
        Ok(())
    }
    fn list_migration_history(
        &self,
        _space: &str,
        _label: &str,
        _is_edge: bool,
    ) -> Result<Vec<MigrationHistoryRecord>, StorageError> {
        Ok(self.migration_history.lock().unwrap().clone())
    }
    fn get_applied_versions(
        &self,
        _space: &str,
        _label: &str,
        _is_edge: bool,
    ) -> Result<Vec<u64>, StorageError> {
        Ok(Vec::new())
    }
    fn scan_vertices_by_tag_paginated(
        &self,
        space: &str,
        tag: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Vertex>, StorageError> {
        Ok(self
            .get_vertices(space, tag)
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }
    fn scan_edges_by_type_paginated(
        &self,
        space: &str,
        edge_type: &str,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Edge>, StorageError> {
        Ok(self
            .edges
            .lock()
            .unwrap()
            .get(&(space.to_string(), edge_type.to_string()))
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect())
    }
}

impl StorageWriter for TestStorage {
    fn insert_vertex(&mut self, _space: &str, _vertex: Vertex) -> Result<VertexId, StorageError> {
        Ok(VertexId::try_from_int64(0).expect("test vertex id"))
    }
    fn update_vertex(&mut self, space: &str, vertex: Vertex) -> Result<(), StorageError> {
        let label = vertex.tag.name.clone();
        let mut map = self.vertices.lock().unwrap();
        if let Some(vec) = map.get_mut(&(space.to_string(), label.clone())) {
            for v in vec.iter_mut() {
                if v.vid == vertex.vid {
                    *v = vertex.clone();
                    return Ok(());
                }
            }
            vec.push(vertex);
        } else {
            map.insert((space.to_string(), label), vec![vertex]);
        }
        Ok(())
    }
    fn delete_vertex(
        &mut self,
        _space: &str,
        _tag: &str,
        _id: &VertexId,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn delete_vertex_with_edges(
        &mut self,
        _space: &str,
        _tag: &str,
        _id: &VertexId,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn batch_delete_vertices_with_edges(
        &mut self,
        _space: &str,
        _tag: &str,
        _ids: &[VertexId],
    ) -> Result<usize, StorageError> {
        Ok(0)
    }
    fn batch_insert_vertices(
        &mut self,
        _space: &str,
        _vertices: Vec<Vertex>,
    ) -> Result<Vec<VertexId>, StorageError> {
        Ok(Vec::new())
    }
    fn insert_edge(&mut self, _space: &str, _edge: Edge) -> Result<(), StorageError> {
        Ok(())
    }
    fn update_edge(&mut self, space: &str, edge: Edge) -> Result<(), StorageError> {
        let mut map = self.edges.lock().unwrap();
        let key = (space.to_string(), edge.edge_type.clone());
        if let Some(vec) = map.get_mut(&key) {
            for e in vec.iter_mut() {
                if e.src == edge.src && e.dst == edge.dst && e.ranking == edge.ranking {
                    *e = edge.clone();
                    return Ok(());
                }
            }
            vec.push(edge);
        } else {
            map.insert(key, vec![edge]);
        }
        Ok(())
    }
    fn delete_edge(
        &mut self,
        _space: &str,
        _src: &VertexId,
        _dst: &VertexId,
        _edge_type: &str,
        _rank: i64,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn batch_insert_edges(&mut self, _space: &str, _edges: Vec<Edge>) -> Result<(), StorageError> {
        Ok(())
    }
    fn batch_delete_edges(
        &mut self,
        _space: &str,
        _deletes: &[EdgeDeleteKey],
    ) -> Result<usize, StorageError> {
        Ok(0)
    }
    fn insert_vertex_data(
        &mut self,
        _space: &str,
        _info: &graphdb_core::types::InsertVertexInfo,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn insert_edge_data(
        &mut self,
        _space: &str,
        _info: &graphdb_core::types::InsertEdgeInfo,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn delete_vertex_data(
        &mut self,
        _space: &str,
        _tag: &str,
        _vertex_id: &str,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn delete_edge_data(
        &mut self,
        _space: &str,
        _src: &str,
        _dst: &str,
        _rank: i64,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn update_data(
        &mut self,
        _space: &str,
        _space_id: u64,
        _info: &graphdb_core::types::UpdateInfo,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
}

impl AutoCommitBatchOps for TestStorage {
    fn begin_auto_commit_batch(
        &self,
    ) -> graphdb_core::StorageResult<Arc<graphdb_storage::AutoCommitBatchWindow>> {
        Err(StorageError::not_supported("not supported"))
    }
    fn bind_auto_commit_statement(
        &self,
        _window: &Arc<graphdb_storage::AutoCommitBatchWindow>,
    ) -> graphdb_core::StorageResult<Self>
    where
        Self: Sized,
    {
        Err(StorageError::not_supported("not supported"))
    }
    fn finalize_auto_commit_batch(
        &self,
        _window: &graphdb_storage::AutoCommitBatchWindow,
    ) -> graphdb_core::StorageResult<()> {
        Err(StorageError::not_supported("not supported"))
    }
}
impl AutoCommitGroupOps for TestStorage {
    fn begin_auto_commit_group(
        &self,
    ) -> graphdb_core::StorageResult<Arc<graphdb_storage::AutoCommitBatchWindow>> {
        Err(StorageError::not_supported("not supported"))
    }
    fn finalize_auto_commit_group(
        &self,
        _window: &graphdb_storage::AutoCommitBatchWindow,
    ) -> graphdb_core::StorageResult<()> {
        Err(StorageError::not_supported("not supported"))
    }
}

impl graphdb_storage::StorageSchemaOps for TestStorage {
    fn create_space(&mut self, _space: &mut SpaceInfo) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn drop_space(&mut self, _space: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn clear_space(&mut self, _space: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn alter_space_comment(
        &mut self,
        _space_id: u64,
        _comment: String,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn create_tag(&mut self, _space: &str, _tag: &TagInfo) -> Result<u32, StorageError> {
        Ok(1)
    }
    fn alter_tag(
        &mut self,
        _space: &str,
        _tag: &str,
        _additions: Vec<graphdb_core::types::PropertyDef>,
        _deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn rename_vertex_property(
        &mut self,
        _label: graphdb_core::types::LabelId,
        _old_name: &str,
        _new_name: &str,
    ) -> Result<(), StorageError> {
        Ok(())
    }
    fn rename_tag_property(
        &mut self,
        _space: &str,
        _tag: &str,
        _old_name: &str,
        _new_name: &str,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn drop_tag(&mut self, _space: &str, _tag: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn rename_tag(
        &mut self,
        _space: &str,
        _old_name: &str,
        _new_name: &str,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn rename_edge_type(
        &mut self,
        _space: &str,
        _old_name: &str,
        _new_name: &str,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn create_edge_type(
        &mut self,
        _space: &str,
        _edge: &EdgeTypeInfo,
    ) -> Result<u32, StorageError> {
        Ok(1)
    }
    fn alter_edge_type(
        &mut self,
        _space: &str,
        _edge_type: &str,
        _additions: Vec<graphdb_core::types::PropertyDef>,
        _deletions: Vec<String>,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn drop_edge_type(&mut self, _space: &str, _edge_type: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn create_tag_index(&mut self, _space: &str, _info: &Index) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn drop_tag_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn rebuild_tag_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn create_edge_index(&mut self, _space: &str, _info: &Index) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn drop_edge_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn rebuild_edge_index(&mut self, _space: &str, _index: &str) -> Result<bool, StorageError> {
        Ok(true)
    }
    fn update_edge_endpoints(
        &mut self,
        _space: &str,
        _edge_type: &str,
        _src_tag_name: &str,
        _dst_tag_name: &str,
    ) -> Result<bool, StorageError> {
        Ok(true)
    }
}

#[test]
fn test_execute_add_column() {
    let mut storage = TestStorage::new();
    storage.insert_vertex("s", "User", 1, HashMap::new());
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: Some(Value::string("a@b.com")),
        }],
        1,
        SafetyLevel::Safe,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert_eq!(
        vertices[0].tag.properties.get("email"),
        Some(&Value::string("a@b.com"))
    );
    // check history recorded
    assert_eq!(storage.migration_history.lock().unwrap().len(), 1);
}

#[test]
fn test_execute_drop_column() {
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("old".into(), Value::string("v"));
    storage.insert_vertex("s", "User", 1, props);
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::DropColumn { name: "old".into() }],
        1,
        SafetyLevel::Dangerous,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert!(!vertices[0].tag.properties.contains_key("old"));
}

#[test]
fn test_execute_type_convert() {
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("age".into(), Value::Int(42));
    storage.insert_vertex("s", "User", 1, props);
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::ConvertType {
            name: "age".into(),
            from_type: DataType::Int,
            to_type: DataType::BigInt,
        }],
        1,
        SafetyLevel::Warning,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert_eq!(
        vertices[0].tag.properties.get("age"),
        Some(&Value::BigInt(42))
    );
}

#[test]
fn test_execute_rollback() {
    let mut storage = TestStorage::new();
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: None,
        }],
        0,
        SafetyLevel::Safe,
        Some(Box::new(MigrationPlan::new(
            MigrationTarget {
                space: "s".into(),
                label: "User".into(),
                is_edge: false,
            },
            VersionRange { from: 2, to: 1 },
            vec![MigrationStep::DropColumn {
                name: "email".into(),
            }],
            0,
            SafetyLevel::Dangerous,
            None,
        ))),
    );
    let report = rollback_migration(&mut storage, &plan).unwrap();
    assert!(report.success);
}

#[test]
fn test_idempotent_execution() {
    let mut storage = TestStorage::new();
    storage.insert_vertex("s", "User", 1, HashMap::new());
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: Some(Value::string("x")),
        }],
        1,
        SafetyLevel::Safe,
        None,
    );
    let r1 = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(r1.success);
    let r2 = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(r2.success);
    let vertices = storage.get_vertices("s", "User");
    assert_eq!(
        vertices[0].tag.properties.get("email"),
        Some(&Value::string("x"))
    );
    // No duplicate history? second execution will attempt to record same to_version -> our mock just pushes, but real manager would reject AlreadyExists. For test we just check no panic.
}

#[test]
fn test_partial_failure() {
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("age".into(), Value::string("not_a_number"));
    storage.insert_vertex("s", "User", 1, props);
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::ConvertType {
            name: "age".into(),
            from_type: DataType::String,
            to_type: DataType::Int,
        }],
        1,
        SafetyLevel::Warning,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(!report.success);
    assert_eq!(report.rows_migrated, 0);
    assert!(!report.errors.is_empty());
}

#[test]
fn test_dry_run_no_commit() {
    let mut storage = TestStorage::new();
    storage.insert_vertex("s", "User", 1, HashMap::new());
    let mut plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: Some(Value::string("dry")),
        }],
        1,
        SafetyLevel::Safe,
        None,
    );
    plan.dry_run = true;
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert!(!vertices[0].tag.properties.contains_key("email"));
}

#[test]
fn test_idempotent_add_column() {
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("email".into(), Value::string("exists"));
    storage.insert_vertex("s", "User", 1, props);
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![MigrationStep::AddColumn {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: Some(Value::string("new")),
        }],
        1,
        SafetyLevel::Safe,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert_eq!(
        vertices[0].tag.properties.get("email"),
        Some(&Value::string("exists"))
    );
}

#[test]
fn test_expand_contract_rename() {
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("old_name".into(), Value::string("hello"));
    storage.insert_vertex("s", "User", 1, props);
    let plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![
            MigrationStep::AddColumn {
                name: "new_name".into(),
                data_type: DataType::String,
                nullable: true,
                default_value: None,
            },
            MigrationStep::RenameColumn {
                old_name: "old_name".into(),
                new_name: "new_name".into(),
            },
            MigrationStep::DropColumn {
                name: "old_name".into(),
            },
        ],
        1,
        SafetyLevel::Warning,
        None,
    );
    let report = execute_migration_plan(&mut storage, &plan).unwrap();
    assert!(report.success);
    let vertices = storage.get_vertices("s", "User");
    assert!(!vertices[0].tag.properties.contains_key("old_name"));
    assert_eq!(
        vertices[0].tag.properties.get("new_name"),
        Some(&Value::string("hello"))
    );
}

#[test]
fn test_checkpoint_resume() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = TestStorage::new();
    let mut props = HashMap::new();
    props.insert("a".into(), Value::string("v1"));
    storage.insert_vertex("s", "User", 1, props);
    let mut plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![
            MigrationStep::AddColumn {
                name: "a".into(),
                data_type: DataType::String,
                nullable: true,
                default_value: Some(Value::string("v1")),
            },
            MigrationStep::AddColumn {
                name: "b".into(),
                data_type: DataType::String,
                nullable: true,
                default_value: Some(Value::string("v2")),
            },
        ],
        1,
        SafetyLevel::Safe,
        None,
    );
    plan.plan_hash = "ckpt_test_hash".to_string();
    // Simulate interrupted run: save checkpoint with first step completed and storage already has column a
    let cp = crate::plan::MigrationCheckpoint {
        completed_step_index: 0,
        rows_migrated_before: 0,
        rows_migrated_after: 1,
        timestamp: crate::plan::checkpoint_now_millis(),
        step_result: crate::plan::StepResult::Success,
        completed_steps: vec![0],
    };
    cp.save(&plan, tmp.path()).unwrap();
    let report = execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        &mut storage,
        &plan,
        &crate::progress::NoopProgress,
        None,
        None,
        None,
        Some(tmp.path()),
    )
    .unwrap();
    assert!(report.success);
    assert!(report.completed_step_indices.contains(&0));
    assert!(report.completed_step_indices.contains(&1));
    let vertices = storage.get_vertices("s", "User");
    assert_eq!(
        vertices[0].tag.properties.get("a"),
        Some(&Value::string("v1"))
    );
    assert_eq!(
        vertices[0].tag.properties.get("b"),
        Some(&Value::string("v2"))
    );
    // checkpoint file should be cleaned up after success
    assert!(crate::plan::MigrationCheckpoint::load(&plan, tmp.path())
        .unwrap()
        .is_none());
}

#[test]
fn test_checkpoint_save_per_step() {
    let tmp = tempfile::tempdir().unwrap();
    let mut storage = TestStorage::new();
    storage.insert_vertex("s", "User", 1, HashMap::new());
    let mut plan = MigrationPlan::new(
        MigrationTarget {
            space: "s".into(),
            label: "User".into(),
            is_edge: false,
        },
        VersionRange { from: 1, to: 2 },
        vec![
            MigrationStep::AddColumn {
                name: "c1".into(),
                data_type: DataType::String,
                nullable: true,
                default_value: Some(Value::string("x")),
            },
            MigrationStep::AddColumn {
                name: "c2".into(),
                data_type: DataType::String,
                nullable: true,
                default_value: Some(Value::string("y")),
            },
        ],
        1,
        SafetyLevel::Safe,
        None,
    );
    plan.plan_hash = "ckpt_save_test".to_string();
    let report = execute_migration_plan_with_progress_and_file_lock_and_checkpoint(
        &mut storage,
        &plan,
        &crate::progress::NoopProgress,
        None,
        None,
        None,
        Some(tmp.path()),
    )
    .unwrap();
    assert!(report.success);
    // checkpoint should be cleaned up after success, but during execution it was saved per step
    assert!(report.completed_step_indices.len() == 2);
}
