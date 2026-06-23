use super::{StorageEngine, TransactionId};
use crate::core::{Direction, Edge, StorageError, Value, Vertex};
use bincode;
use lru::LruCache;
use rocksdb::{DB, ColumnFamilyDescriptor, Options, DBCompressionType, WriteBatchWithTransaction, Cache, BlockBasedOptions, IteratorMode};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// RocksDB storage implementation
#[derive(Debug)]
pub struct RocksDBStorage {
    db: DB,
    db_path: String,
    vertex_cache: Arc<Mutex<LruCache<Vec<u8>, Vertex>>>,
    edge_cache: Arc<Mutex<LruCache<Vec<u8>, Edge>>>,
    active_transactions: Arc<Mutex<HashMap<TransactionId, TransactionBatches>>>,
}

/// Transaction batches for all column families
struct TransactionBatches {
    nodes_batch: WriteBatchWithTransaction<false>,
    edges_batch: WriteBatchWithTransaction<false>,
    node_edge_index_batch: WriteBatchWithTransaction<false>,
    edge_type_index_batch: WriteBatchWithTransaction<false>,
    prop_index_batch: WriteBatchWithTransaction<false>,
}

impl std::fmt::Debug for TransactionBatches {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransactionBatches")
            .field("nodes_batch", &"<WriteBatch>")
            .field("edges_batch", &"<WriteBatch>")
            .field("node_edge_index_batch", &"<WriteBatch>")
            .field("edge_type_index_batch", &"<WriteBatch>")
            .field("prop_index_batch", &"<WriteBatch>")
            .finish()
    }
}

impl Clone for RocksDBStorage {
    fn clone(&self) -> Self {
        Self::new(&self.db_path).expect("Failed to clone RocksDBStorage")
    }
}

impl RocksDBStorage {
    pub fn new<P: AsRef<std::path::Path>>(path: P) -> Result<Self, StorageError> {
        let db_path = path.as_ref().to_string_lossy().to_string();

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);
        db_opts.set_compression_type(DBCompressionType::Lz4);
        db_opts.set_max_open_files(10000);

        let cache = Cache::new_lru_cache(512 * 1024 * 1024);
        let mut table_opts = BlockBasedOptions::default();
        table_opts.set_block_cache(&cache);

        let mut nodes_cf_opts = Options::default();
        nodes_cf_opts.set_write_buffer_size(64 * 1024 * 1024);
        nodes_cf_opts.set_max_write_buffer_number(16);
        nodes_cf_opts.set_compression_type(DBCompressionType::Lz4);

        let mut edges_cf_opts = Options::default();
        edges_cf_opts.set_write_buffer_size(128 * 1024 * 1024);
        edges_cf_opts.set_max_write_buffer_number(24);
        edges_cf_opts.set_compression_type(DBCompressionType::Snappy);

        let mut schema_cf_opts = Options::default();
        schema_cf_opts.set_write_buffer_size(16 * 1024 * 1024);
        schema_cf_opts.set_max_write_buffer_number(8);
        schema_cf_opts.set_compression_type(DBCompressionType::Lz4);

        let mut indexes_cf_opts = Options::default();
        indexes_cf_opts.set_write_buffer_size(32 * 1024 * 1024);
        indexes_cf_opts.set_max_write_buffer_number(16);
        indexes_cf_opts.set_compression_type(DBCompressionType::Lz4);

        let cfs = vec![
            ColumnFamilyDescriptor::new("nodes", nodes_cf_opts),
            ColumnFamilyDescriptor::new("edges", edges_cf_opts),
            ColumnFamilyDescriptor::new("schema", schema_cf_opts),
            ColumnFamilyDescriptor::new("indexes", indexes_cf_opts),
        ];

        let db = DB::open_cf_descriptors(&db_opts, &db_path, cfs)
            .map_err(|e| StorageError::DbError(e.to_string()))?;

        let vertex_cache_size = std::num::NonZeroUsize::new(1000).expect("Failed to create NonZeroUsize for vertex cache");
        let edge_cache_size = std::num::NonZeroUsize::new(1000).expect("Failed to create NonZeroUsize for edge cache");
        let vertex_cache = Arc::new(Mutex::new(LruCache::new(vertex_cache_size)));
        let edge_cache = Arc::new(Mutex::new(LruCache::new(edge_cache_size)));
        let active_transactions = Arc::new(Mutex::new(HashMap::new()));

        Ok(Self {
            db,
            db_path,
            vertex_cache,
            edge_cache,
            active_transactions,
        })
    }

    fn rocksdb_error_to_storage_error(e: rocksdb::Error) -> StorageError {
        StorageError::DbError(e.to_string())
    }

    fn get_db_path(&self) -> &str {
        &self.db_path
    }

    fn get_cf(&self, name: &str) -> Result<&rocksdb::ColumnFamily, StorageError> {
        self.db.cf_handle(name)
            .ok_or_else(|| StorageError::DbError(format!("Column family '{}' not found", name)))
    }

    fn generate_id(&self) -> Value {
        let id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos() as u64;
        Value::Int(id as i64)
    }

    fn value_to_bytes(&self, value: &Value) -> Result<Vec<u8>, StorageError> {
        bincode::encode_to_vec(value, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))
    }

    fn value_from_bytes(&self, bytes: &[u8]) -> Result<Value, StorageError> {
        let (value, _): (Value, usize) = bincode::decode_from_slice(bytes, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        Ok(value)
    }

    fn vertex_to_bytes(&self, vertex: &Vertex) -> Result<Vec<u8>, StorageError> {
        bincode::encode_to_vec(vertex, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))
    }

    fn vertex_from_bytes(&self, bytes: &[u8]) -> Result<Vertex, StorageError> {
        let (vertex, _): (Vertex, usize) = bincode::decode_from_slice(bytes, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        Ok(vertex)
    }

    fn edge_to_bytes(&self, edge: &Edge) -> Result<Vec<u8>, StorageError> {
        bincode::encode_to_vec(edge, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))
    }

    fn edge_from_bytes(&self, bytes: &[u8]) -> Result<Edge, StorageError> {
        let (edge, _): (Edge, usize) = bincode::decode_from_slice(bytes, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        Ok(edge)
    }

    fn update_node_edge_index(
        &self,
        node_id: &Value,
        edge_key: &[u8],
        add: bool,
    ) -> Result<(), StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let node_id_bytes = self.value_to_bytes(node_id)?;
        let index_key = format!("node_edge_index:{:?}", node_id);
        let index_key_bytes = index_key.as_bytes();

        let mut edge_list = match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (result, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                result
            }
            None => Vec::new(),
        };

        if add {
            if !edge_list.contains(&edge_key.to_vec()) {
                edge_list.push(edge_key.to_vec());
            }
        } else {
            edge_list.retain(|key| key != edge_key);
        }

        let list_bytes = bincode::encode_to_vec(&edge_list, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;

        self.db
            .put_cf(indexes_cf, index_key_bytes, list_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        Ok(())
    }

    fn get_node_edge_keys(&self, node_id: &Value) -> Result<Vec<Vec<u8>>, StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let node_id_bytes = self.value_to_bytes(node_id)?;
        let index_key = format!("node_edge_index:{:?}", node_id);
        let index_key_bytes = index_key.as_bytes();

        match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (edge_key_list, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                Ok(edge_key_list)
            }
            None => Ok(Vec::new()),
        }
    }

    fn update_edge_type_index(
        &self,
        edge_type: &str,
        edge_key: &[u8],
        add: bool,
    ) -> Result<(), StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let edge_type_bytes = edge_type.as_bytes();
        let index_key = format!("edge_type_index:{}", edge_type);
        let index_key_bytes = index_key.as_bytes();

        let mut edge_list = match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (result, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                result
            }
            None => Vec::new(),
        };

        if add {
            if !edge_list.contains(&edge_key.to_vec()) {
                edge_list.push(edge_key.to_vec());
            }
        } else {
            edge_list.retain(|key| key != edge_key);
        }

        let list_bytes = bincode::encode_to_vec(&edge_list, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;

        self.db
            .put_cf(indexes_cf, index_key_bytes, list_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        Ok(())
    }

    fn get_edge_keys_by_type(&self, edge_type: &str) -> Result<Vec<Vec<u8>>, StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let edge_type_bytes = edge_type.as_bytes();
        let index_key = format!("edge_type_index:{}", edge_type);
        let index_key_bytes = index_key.as_bytes();

        match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (edge_key_list, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                Ok(edge_key_list)
            }
            None => Ok(Vec::new()),
        }
    }

    fn update_prop_index(
        &self,
        tag: &str,
        prop: &str,
        value: &Value,
        vertex_id: &Value,
        add: bool,
    ) -> Result<(), StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let index_key = format!("prop_index:{}:{}:{:?}", tag, prop, value);
        let index_key_bytes = index_key.as_bytes();
        let vertex_id_bytes = self.value_to_bytes(vertex_id)?;

        let mut vertex_list = match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (result, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                result
            }
            None => Vec::new(),
        };

        if add {
            if !vertex_list.contains(&vertex_id_bytes) {
                vertex_list.push(vertex_id_bytes);
            }
        } else {
            vertex_list.retain(|id| id != &vertex_id_bytes);
        }

        let list_bytes = bincode::encode_to_vec(&vertex_list, bincode::config::standard())
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;

        self.db
            .put_cf(indexes_cf, index_key_bytes, list_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        Ok(())
    }

    fn get_vertices_by_prop(
        &self,
        tag: &str,
        prop: &str,
        value: &Value,
    ) -> Result<Vec<Vertex>, StorageError> {
        let indexes_cf = self.get_cf("indexes")?;
        let index_key = format!("prop_index:{}:{}:{:?}", tag, prop, value);
        let index_key_bytes = index_key.as_bytes();

        match self
            .db
            .get_cf(indexes_cf, index_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(list_bytes) => {
                let (vertex_id_list, _): (Vec<Vec<u8>>, usize) = bincode::decode_from_slice(&list_bytes, bincode::config::standard())
                    .map_err(|e| StorageError::SerializationError(e.to_string()))?;
                
                let mut vertices = Vec::new();
                for vertex_id_bytes in vertex_id_list {
                    if let Some(vertex) = self.get_node_from_bytes(&vertex_id_bytes)? {
                        vertices.push(vertex);
                    }
                }
                Ok(vertices)
            }
            None => Ok(Vec::new()),
        }
    }

    fn get_node_from_bytes(&self, id_bytes: &[u8]) -> Result<Option<Vertex>, StorageError> {
        let nodes_cf = self.get_cf("nodes")?;
        match self
            .db
            .get_cf(nodes_cf, id_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(vertex_bytes) => {
                let vertex: Vertex = self.vertex_from_bytes(&vertex_bytes)?;
                Ok(Some(vertex))
            }
            None => Ok(None),
        }
    }

    fn get_edge_from_bytes(&self, edge_key_bytes: &[u8]) -> Result<Option<Edge>, StorageError> {
        let edges_cf = self.get_cf("edges")?;
        match self
            .db
            .get_cf(edges_cf, edge_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?
        {
            Some(edge_bytes) => {
                let edge: Edge = self.edge_from_bytes(&edge_bytes)?;
                Ok(Some(edge))
            }
            None => Ok(None),
        }
    }
}

impl StorageEngine for RocksDBStorage {
    fn insert_node(&mut self, vertex: Vertex) -> Result<Value, StorageError> {
        let id = self.generate_id();
        let vertex_with_id = Vertex::new(id.clone(), vertex.tags);

        let vertex_bytes = self.vertex_to_bytes(&vertex_with_id)?;
        let id_bytes = self.value_to_bytes(&id)?;

        let nodes_cf = self.get_cf("nodes")?;
        self.db
            .put_cf(nodes_cf, id_bytes, vertex_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        Ok(id)
    }

    fn get_node(&self, id: &Value) -> Result<Option<Vertex>, StorageError> {
        let id_bytes = self.value_to_bytes(id)?;

        {
            let mut cache = self.vertex_cache.lock().expect("Failed to lock vertex cache");
            if let Some(vertex) = cache.get(&id_bytes) {
                return Ok(Some(vertex.clone()));
            }
        }

        match self.get_node_from_bytes(&id_bytes)? {
            Some(vertex) => {
                {
                    let mut cache = self.vertex_cache.lock().expect("Failed to lock vertex cache");
                    cache.put(id_bytes.clone(), vertex.clone());
                }
                Ok(Some(vertex))
            }
            None => Ok(None),
        }
    }

    fn update_node(&mut self, vertex: Vertex) -> Result<(), StorageError> {
        if matches!(*vertex.vid, Value::Null(_)) {
            return Err(StorageError::NodeNotFound(Value::Null(Default::default())));
        }

        let vertex_bytes = self.vertex_to_bytes(&vertex)?;
        let id_bytes = self.value_to_bytes(&vertex.vid)?;

        let nodes_cf = self.get_cf("nodes")?;
        self.db
            .put_cf(nodes_cf, &id_bytes, vertex_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        {
            let mut cache = self.vertex_cache.lock().expect("Failed to lock vertex cache");
            cache.put(id_bytes, vertex);
        }

        Ok(())
    }

    fn delete_node(&mut self, id: &Value) -> Result<(), StorageError> {
        let edges_to_delete = self.get_node_edges(id, Direction::Both)?;
        for edge in edges_to_delete {
            self.delete_edge(&edge.src, &edge.dst, &edge.edge_type)?;
        }

        let id_bytes = self.value_to_bytes(id)?;
        let nodes_cf = self.get_cf("nodes")?;
        self.db
            .delete_cf(nodes_cf, &id_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        let indexes_cf = self.get_cf("indexes")?;
        let index_key = format!("node_edge_index:{:?}", id);
        self.db
            .delete_cf(indexes_cf, index_key.as_bytes())
            .map_err(Self::rocksdb_error_to_storage_error)?;

        {
            let mut cache = self.vertex_cache.lock().expect("Failed to lock vertex cache");
            cache.pop(&id_bytes);
        }

        Ok(())
    }

    fn scan_all_vertices(&self) -> Result<Vec<Vertex>, StorageError> {
        let nodes_cf = self.get_cf("nodes")?;
        let mut vertices = Vec::new();

        let iter = self.db.iterator_cf(nodes_cf, IteratorMode::Start);
        for item in iter {
            let (_, vertex_bytes) = item.map_err(Self::rocksdb_error_to_storage_error)?;
            let vertex: Vertex = self.vertex_from_bytes(&vertex_bytes)?;
            vertices.push(vertex);
        }

        Ok(vertices)
    }

    fn scan_vertices_by_tag(&self, tag: &str) -> Result<Vec<Vertex>, StorageError> {
        let all_vertices = self.scan_all_vertices()?;
        let filtered_vertices = all_vertices
            .into_iter()
            .filter(|vertex| vertex.tags.iter().any(|vertex_tag| vertex_tag.name == tag))
            .collect();

        Ok(filtered_vertices)
    }

    fn scan_vertices_by_prop(&self, tag: &str, prop: &str, value: &Value) -> Result<Vec<Vertex>, StorageError> {
        self.get_vertices_by_prop(tag, prop, value)
    }

    fn insert_edge(&mut self, edge: Edge) -> Result<(), StorageError> {
        let edge_key = format!("{:?}_{:?}_{}", edge.src, edge.dst, edge.edge_type);
        let edge_key_bytes = edge_key.as_bytes().to_vec();

        let edge_bytes = self.edge_to_bytes(&edge)?;

        let edges_cf = self.get_cf("edges")?;
        self.db
            .put_cf(edges_cf, &edge_key_bytes, edge_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        self.update_node_edge_index(&edge.src, &edge_key_bytes, true)?;
        self.update_node_edge_index(&edge.dst, &edge_key_bytes, true)?;
        self.update_edge_type_index(&edge.edge_type, &edge_key_bytes, true)?;

        for (prop_name, prop_value) in &edge.props {
            self.update_prop_index(&edge.edge_type, prop_name, prop_value, &edge.src, true)?;
        }

        Ok(())
    }

    fn get_edge(
        &self,
        src: &Value,
        dst: &Value,
        edge_type: &str,
    ) -> Result<Option<Edge>, StorageError> {
        let edge_key = format!("{:?}_{:?}_{}", src, dst, edge_type);
        let edge_key_bytes = edge_key.as_bytes().to_vec();

        {
            let mut cache = self.edge_cache.lock().expect("Failed to lock edge cache");
            if let Some(edge) = cache.get(&edge_key_bytes) {
                return Ok(Some(edge.clone()));
            }
        }

        match self.get_edge_from_bytes(&edge_key_bytes)? {
            Some(edge) => {
                {
                    let mut cache = self.edge_cache.lock().expect("Failed to lock edge cache");
                    cache.put(edge_key_bytes.clone(), edge.clone());
                }
                Ok(Some(edge))
            }
            None => Ok(None),
        }
    }

    fn get_node_edges(
        &self,
        node_id: &Value,
        direction: Direction,
    ) -> Result<Vec<Edge>, StorageError> {
        self.get_node_edges_filtered(node_id, direction, None)
    }

    fn get_node_edges_filtered(
        &self,
        node_id: &Value,
        direction: Direction,
        filter: Option<Box<dyn Fn(&Edge) -> bool + Send + Sync>>,
    ) -> Result<Vec<Edge>, StorageError> {
        let edge_keys = self.get_node_edge_keys(node_id)?;
        let mut edges = Vec::new();

        for edge_key_bytes in edge_keys {
            if let Some(edge) = self.get_edge_from_bytes(&edge_key_bytes)? {
                match direction {
                    Direction::Out if *edge.src == *node_id => edges.push(edge),
                    Direction::In if *edge.dst == *node_id => edges.push(edge),
                    Direction::Both => edges.push(edge),
                    _ => continue,
                }
            }
        }

        if let Some(filter_fn) = filter {
            edges = edges.into_iter().filter(|e| filter_fn(e)).collect();
        }

        Ok(edges)
    }

    fn delete_edge(
        &mut self,
        src: &Value,
        dst: &Value,
        edge_type: &str,
    ) -> Result<(), StorageError> {
        let edge_key = format!("{:?}_{:?}_{}", src, dst, edge_type);
        let edge_key_bytes = edge_key.as_bytes().to_vec();

        let edges_cf = self.get_cf("edges")?;
        self.db
            .delete_cf(edges_cf, &edge_key_bytes)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        self.update_node_edge_index(&src, &edge_key_bytes, false)?;
        self.update_node_edge_index(&dst, &edge_key_bytes, false)?;
        self.update_edge_type_index(edge_type, &edge_key_bytes, false)?;

        Ok(())
    }

    fn scan_edges_by_type(&self, edge_type: &str) -> Result<Vec<Edge>, StorageError> {
        let edge_keys = self.get_edge_keys_by_type(edge_type)?;
        let mut edges = Vec::new();

        for edge_key_bytes in edge_keys {
            if let Some(edge) = self.get_edge_from_bytes(&edge_key_bytes)? {
                edges.push(edge);
            }
        }

        Ok(edges)
    }

    fn scan_all_edges(&self) -> Result<Vec<Edge>, StorageError> {
        let edges_cf = self.get_cf("edges")?;
        let mut edges = Vec::new();

        let iter = self.db.iterator_cf(edges_cf, IteratorMode::Start);
        for item in iter {
            let (_, edge_bytes) = item.map_err(Self::rocksdb_error_to_storage_error)?;
            let edge: Edge = self.edge_from_bytes(&edge_bytes)?;
            edges.push(edge);
        }

        Ok(edges)
    }

    fn batch_insert_nodes(&mut self, vertices: Vec<Vertex>) -> Result<Vec<Value>, StorageError> {
        let mut ids = Vec::new();
        let nodes_cf = self.get_cf("nodes")?;

        for vertex in vertices {
            let id = self.generate_id();
            let vertex_with_id = Vertex::new(id.clone(), vertex.tags);
            let vertex_bytes = self.vertex_to_bytes(&vertex_with_id)?;
            let id_bytes = self.value_to_bytes(&id)?;

            self.db
                .put_cf(nodes_cf, id_bytes, vertex_bytes)
                .map_err(Self::rocksdb_error_to_storage_error)?;

            ids.push(id);
        }

        Ok(ids)
    }

    fn batch_insert_edges(&mut self, edges: Vec<Edge>) -> Result<(), StorageError> {
        let edges_cf = self.get_cf("edges")?;

        for edge in edges {
            let edge_key = format!("{:?}_{:?}_{}", edge.src, edge.dst, edge.edge_type);
            let edge_key_bytes = edge_key.as_bytes().to_vec();

            let edge_bytes = self.edge_to_bytes(&edge)?;

            self.db
                .put_cf(edges_cf, &edge_key_bytes, edge_bytes)
                .map_err(Self::rocksdb_error_to_storage_error)?;

            self.update_node_edge_index(&edge.src, &edge_key_bytes, true)?;
            self.update_node_edge_index(&edge.dst, &edge_key_bytes, true)?;
            self.update_edge_type_index(&edge.edge_type, &edge_key_bytes, true)?;

            for (prop_name, prop_value) in &edge.props {
                self.update_prop_index(&edge.edge_type, prop_name, prop_value, &edge.src, true)?;
            }
        }

        Ok(())
    }

    fn begin_transaction(&mut self) -> Result<TransactionId, StorageError> {
        let tx_id = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards")
            .as_nanos() as u64;

        let batches = TransactionBatches {
            nodes_batch: WriteBatchWithTransaction::default(),
            edges_batch: WriteBatchWithTransaction::default(),
            node_edge_index_batch: WriteBatchWithTransaction::default(),
            edge_type_index_batch: WriteBatchWithTransaction::default(),
            prop_index_batch: WriteBatchWithTransaction::default(),
        };

        {
            let mut transactions = self.active_transactions.lock().expect("Failed to lock transactions");
            transactions.insert(tx_id, batches);
        }

        Ok(tx_id)
    }

    fn commit_transaction(&mut self, tx_id: TransactionId) -> Result<(), StorageError> {
        let batches = {
            let mut transactions = self.active_transactions.lock().expect("Failed to lock transactions");
            transactions.remove(&tx_id)
                .ok_or_else(|| StorageError::TransactionNotFound(tx_id))?
        };

        self.db.write(batches.nodes_batch)
            .map_err(Self::rocksdb_error_to_storage_error)?;
        self.db.write(batches.edges_batch)
            .map_err(Self::rocksdb_error_to_storage_error)?;
        self.db.write(batches.node_edge_index_batch)
            .map_err(Self::rocksdb_error_to_storage_error)?;
        self.db.write(batches.edge_type_index_batch)
            .map_err(Self::rocksdb_error_to_storage_error)?;
        self.db.write(batches.prop_index_batch)
            .map_err(Self::rocksdb_error_to_storage_error)?;

        Ok(())
    }

    fn rollback_transaction(&mut self, tx_id: TransactionId) -> Result<(), StorageError> {
        {
            let mut transactions = self.active_transactions.lock().expect("Failed to lock transactions");
            transactions.remove(&tx_id)
                .ok_or_else(|| StorageError::TransactionNotFound(tx_id));
        }

        Ok(())
    }
}
