use linkrs_api::embedded::QueryResult as EmbeddedQueryResult;
use linkrs_api::embedded::{DatabaseConfig, GraphDatabase, Session as EmbeddedSession};
use linkrs_cli::client::types::QueryResult as ClientQueryResult;
use linkrs_cli::client::utils::error::{ClientError, Result as ClientResult};
use linkrs_cli::session::EmbeddedOps;
use linkrs_core::lock::{acquire_db_lock, DbFileLock};
use linkrs_storage::{GraphStorage, StorageReader};
use std::sync::Arc;

pub struct EmbeddedBackend {
    _db: Arc<GraphDatabase<GraphStorage>>,
    session: EmbeddedSession<GraphStorage>,
    current_space: Option<String>,
    read_only: bool,
    _lock: Option<DbFileLock>,
}

impl EmbeddedBackend {
    #[allow(dead_code)]
    pub fn open(path: &str) -> ClientResult<Self> {
        Self::open_with_options(path, false)
    }

    pub fn open_with_options(path: &str, read_only: bool) -> ClientResult<Self> {
        let lock = acquire_db_lock(path, read_only)
            .map_err(|e| ClientError::connection(e.to_string()))?;
        let config = DatabaseConfig::file(path).with_read_only(read_only);
        let db = GraphDatabase::open_with_config(config)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        let session = db
            .session()
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(Self {
            _db: Arc::new(db),
            session,
            current_space: None,
            read_only,
            _lock: lock,
        })
    }

    pub fn open_in_memory() -> ClientResult<Self> {
        let db = GraphDatabase::open_in_memory().map_err(|e| ClientError::Other(e.to_string()))?;
        let session = db
            .session()
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(Self {
            _db: Arc::new(db),
            session,
            current_space: None,
            read_only: false,
            _lock: None,
        })
    }
}

impl EmbeddedOps for EmbeddedBackend {
    fn switch_space(&mut self, space: &str) -> ClientResult<()> {
        self.session
            .use_space(space)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        self.current_space = Some(space.to_string());
        Ok(())
    }

    fn execute_query(&mut self, query: &str) -> ClientResult<ClientQueryResult> {
        let result = self
            .session
            .execute(query)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        if let Some(space) = self.session.current_space() {
            self.current_space = Some(space);
        }
        convert_result(result)
    }

    fn list_spaces(&self) -> ClientResult<Vec<String>> {
        self.session
            .list_spaces()
            .map_err(|e| ClientError::Other(e.to_string()))
    }

    fn current_space(&self) -> Option<String> {
        self.current_space
            .clone()
            .or_else(|| self.session.current_space())
    }

    fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn list_tags(&self) -> ClientResult<Vec<linkrs_cli::client::TagInfo>> {
        let current_space = self
            .current_space
            .clone()
            .or_else(|| self.session.current_space());
        let space = current_space
            .as_deref()
            .ok_or_else(|| ClientError::NoSpaceSelected)?;
        let storage = self._db.storage();
        let tags = storage
            .list_tags(space)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(tags
            .into_iter()
            .map(|tag| linkrs_cli::client::TagInfo {
                name: tag.tag_name,
                fields: tag
                    .properties
                    .into_iter()
                    .map(|prop| linkrs_cli::client::FieldInfo {
                        name: prop.name,
                        data_type: format!("{:?}", prop.data_type),
                        nullable: prop.nullable,
                        default_value: prop.default.map(|value| format!("{:?}", value)),
                    })
                    .collect(),
            })
            .collect())
    }

    fn list_edge_types(&self) -> ClientResult<Vec<linkrs_cli::client::EdgeTypeInfo>> {
        let current_space = self
            .current_space
            .clone()
            .or_else(|| self.session.current_space());
        let space = current_space
            .as_deref()
            .ok_or_else(|| ClientError::NoSpaceSelected)?;
        let storage = self._db.storage();
        let edge_types = storage
            .list_edge_types(space)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(edge_types
            .into_iter()
            .map(|edge_type| linkrs_cli::client::EdgeTypeInfo {
                name: edge_type.edge_type_name,
                fields: edge_type
                    .properties
                    .into_iter()
                    .map(|prop| linkrs_cli::client::FieldInfo {
                        name: prop.name,
                        data_type: format!("{:?}", prop.data_type),
                        nullable: prop.nullable,
                        default_value: prop.default.map(|value| format!("{:?}", value)),
                    })
                    .collect(),
            })
            .collect())
    }
}

fn convert_result(result: EmbeddedQueryResult) -> ClientResult<ClientQueryResult> {
    use std::collections::HashMap;
    let columns = result.columns().to_vec();
    let mut rows = Vec::with_capacity(result.len());
    for row in result.iter() {
        let mut map = HashMap::new();
        for col in &columns {
            if let Some(value) = row.get(col) {
                let json = serde_json::to_value(value).unwrap_or(serde_json::Value::Null);
                map.insert(col.clone(), json);
            }
        }
        rows.push(map);
    }
    let metadata = result.metadata();
    Ok(ClientQueryResult {
        columns,
        rows,
        row_count: metadata.rows_returned,
        execution_time_ms: metadata.execution_time.as_millis() as u64,
        rows_scanned: metadata.rows_scanned,
        trace_id: None,
        error: None,
    })
}
