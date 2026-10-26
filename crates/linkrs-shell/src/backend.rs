use linkrs_api::embedded::{GraphDatabase, Session as EmbeddedSession};
use linkrs_api::embedded::QueryResult as EmbeddedQueryResult;
use linkrs_cli::client::utils::error::{ClientError, Result as ClientResult};
use linkrs_cli::client::types::QueryResult as ClientQueryResult;
use linkrs_cli::session::EmbeddedOps;
use linkrs_storage::GraphStorage;
use std::sync::Arc;

pub struct EmbeddedBackend {
    _db: Arc<GraphDatabase<GraphStorage>>,
    session: EmbeddedSession<GraphStorage>,
    current_space: Option<String>,
}

impl EmbeddedBackend {
    pub fn open(path: &str) -> ClientResult<Self> {
        let db = GraphDatabase::open(path)
            .map_err(|e| ClientError::Other(e.to_string()))?;
        let session = db
            .session()
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(Self {
            _db: Arc::new(db),
            session,
            current_space: None,
        })
    }

    pub fn open_in_memory() -> ClientResult<Self> {
        let db = GraphDatabase::open_in_memory()
            .map_err(|e| ClientError::Other(e.to_string()))?;
        let session = db
            .session()
            .map_err(|e| ClientError::Other(e.to_string()))?;
        Ok(Self {
            _db: Arc::new(db),
            session,
            current_space: None,
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
