use linkrs_client::config::ClientConfig;
use linkrs_client::http_client::HttpClient;
use linkrs_client::session::Session as ClientSession;
use linkrs_client::session::SessionManager as HttpClientSessionManager;
use linkrs_client::types::QueryResult;
use linkrs_client::utils::error::Result as ClientResult;
use linkrs_client::SpaceInfo;

/// Operations provided by an embedded backend.
///
/// Implemented by crates that link the database engine (for example the
/// `linkrs` shell) so the shared CLI code stays free of engine dependencies.
pub trait EmbeddedOps: Send + Sync {
    fn switch_space(&mut self, space: &str) -> ClientResult<()>;
    fn execute_query(&mut self, query: &str) -> ClientResult<QueryResult>;
    fn list_spaces(&self) -> ClientResult<Vec<String>>;
    fn current_space(&self) -> Option<String>;
    fn is_read_only(&self) -> bool {
        false
    }
    /// List tags of the current space. Default returns an unsupported error
    /// so embedded backends can opt in once schema introspection is available.
    fn list_tags(&self) -> ClientResult<Vec<linkrs_client::TagInfo>> {
        Err(linkrs_client::utils::error::ClientError::command(
            "embedded mode does not support: \\show_tags (no embedded schema introspection yet)",
        ))
    }
    /// List edge types of the current space.
    fn list_edge_types(&self) -> ClientResult<Vec<linkrs_client::EdgeTypeInfo>> {
        Err(linkrs_client::utils::error::ClientError::command(
            "embedded mode does not support: \\show_edges (no embedded schema introspection yet)",
        ))
    }
    /// Process-internal health payload.
    fn health_json(&self) -> ClientResult<serde_json::Value> {
        Err(linkrs_client::utils::error::ClientError::command(
            "embedded backend does not expose health",
        ))
    }
    /// Prometheus metrics text.
    fn metrics_text(&self) -> ClientResult<String> {
        Err(linkrs_client::utils::error::ClientError::command(
            "embedded backend does not expose metrics",
        ))
    }
}

/// Handle to an embedded backend stored inside a [`SessionManager`].
pub struct EmbeddedSession {
    pub ops: Box<dyn EmbeddedOps + Send + Sync>,
    pub client_session: ClientSession,
    pub dummy_client: HttpClient,
    pub current_space: Option<String>,
}

/// Unified session manager used by the CLI.
pub enum SessionManager {
    Http(HttpClientSessionManager),
    Embedded(EmbeddedSession),
}

impl SessionManager {
    pub fn new_http(host: &str, port: u16) -> ClientResult<Self> {
        Ok(Self::Http(HttpClientSessionManager::new_http(host, port)?))
    }

    pub fn with_config(config: ClientConfig) -> ClientResult<Self> {
        Ok(Self::Http(HttpClientSessionManager::with_config(config)?))
    }

    pub fn new_embedded(ops: Box<dyn EmbeddedOps + Send + Sync>) -> Self {
        let client_session = ClientSession::new(0, "embedded".to_string(), "local".to_string(), 0);
        let dummy_client = HttpClient::with_config(ClientConfig::new())
            .unwrap_or_else(|_| HttpClient::with_config(ClientConfig::default()).unwrap());
        Self::Embedded(EmbeddedSession {
            ops,
            client_session,
            dummy_client,
            current_space: None,
        })
    }

    pub async fn connect(&mut self, _username: &str, _password: &str) -> ClientResult<()> {
        match self {
            Self::Http(inner) => inner.connect(_username, _password).await,
            Self::Embedded(_) => Ok(()),
        }
    }

    pub async fn connect_with_host(
        &mut self,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> ClientResult<()> {
        match self {
            Self::Http(inner) => {
                inner
                    .connect_with_host(host, port, username, password)
                    .await
            }
            Self::Embedded(_) => Ok(()),
        }
    }

    pub async fn disconnect(&mut self) -> ClientResult<()> {
        match self {
            Self::Http(inner) => inner.disconnect().await,
            Self::Embedded(_) => Ok(()),
        }
    }

    pub async fn switch_space(&mut self, space: &str) -> ClientResult<()> {
        match self {
            Self::Http(inner) => inner.switch_space(space).await,
            Self::Embedded(embedded) => {
                embedded.ops.switch_space(space)?;
                embedded.current_space = Some(space.to_string());
                embedded.client_session.current_space = Some(space.to_string());
                Ok(())
            }
        }
    }

    pub async fn execute_query(&mut self, query: &str) -> ClientResult<QueryResult> {
        match self {
            Self::Http(inner) => inner.execute_query(query).await,
            Self::Embedded(embedded) => embedded.ops.execute_query(query),
        }
    }

    pub async fn execute_query_raw(&mut self, query: &str) -> ClientResult<QueryResult> {
        match self {
            Self::Http(inner) => inner.execute_query_raw(query).await,
            Self::Embedded(embedded) => embedded.ops.execute_query(query),
        }
    }

    pub async fn execute_batch(&mut self, statements: &[String]) -> ClientResult<Vec<QueryResult>> {
        match self {
            Self::Http(inner) => inner.execute_batch(statements).await,
            Self::Embedded(embedded) => {
                let mut outcomes = Vec::with_capacity(statements.len());
                for statement in statements {
                    outcomes.push(embedded.ops.execute_query(statement)?);
                }
                Ok(outcomes)
            }
        }
    }

    pub async fn health_check(&self) -> ClientResult<bool> {
        match self {
            Self::Http(inner) => inner.health_check().await,
            Self::Embedded(_) => Ok(true),
        }
    }

    pub fn embedded_health(&self) -> ClientResult<serde_json::Value> {
        match self {
            Self::Http(_) => Err(linkrs_client::utils::error::ClientError::command(
                "health JSON is only available in embedded mode",
            )),
            Self::Embedded(embedded) => embedded.ops.health_json(),
        }
    }

    pub fn embedded_metrics(&self) -> ClientResult<String> {
        match self {
            Self::Http(_) => Err(linkrs_client::utils::error::ClientError::command(
                "metrics text is only available in embedded mode",
            )),
            Self::Embedded(embedded) => embedded.ops.metrics_text(),
        }
    }

    pub async fn list_spaces(&self) -> ClientResult<Vec<SpaceInfo>> {
        match self {
            Self::Http(inner) => inner.list_spaces().await,
            Self::Embedded(embedded) => {
                let spaces = embedded.ops.list_spaces()?;
                Ok(spaces
                    .into_iter()
                    .map(|name| SpaceInfo {
                        id: 0,
                        name,
                        vid_type: String::new(),
                        comment: None,
                    })
                    .collect())
            }
        }
    }

    pub async fn list_tags(&self) -> ClientResult<Vec<linkrs_client::TagInfo>> {
        match self {
            Self::Http(inner) => inner.list_tags().await,
            Self::Embedded(embedded) => embedded.ops.list_tags(),
        }
    }

    pub async fn list_edge_types(&self) -> ClientResult<Vec<linkrs_client::EdgeTypeInfo>> {
        match self {
            Self::Http(inner) => inner.list_edge_types().await,
            Self::Embedded(embedded) => embedded.ops.list_edge_types(),
        }
    }

    pub async fn list_functions(&self) -> ClientResult<Vec<String>> {
        match self {
            Self::Http(inner) => inner.list_functions().await,
            Self::Embedded(_) => Err(linkrs_client::utils::error::ClientError::command(
                "embedded mode does not support: \\show_functions (server only)",
            )),
        }
    }

    pub fn session(&self) -> Option<&ClientSession> {
        match self {
            Self::Http(inner) => inner.session(),
            Self::Embedded(embedded) => Some(&embedded.client_session),
        }
    }

    pub fn session_mut(&mut self) -> Option<&mut ClientSession> {
        match self {
            Self::Http(inner) => inner.session_mut(),
            Self::Embedded(embedded) => Some(&mut embedded.client_session),
        }
    }

    pub fn is_connected(&self) -> bool {
        match self {
            Self::Http(inner) => inner.is_connected(),
            Self::Embedded(_) => true,
        }
    }

    pub fn current_space(&self) -> Option<&str> {
        match self {
            Self::Http(inner) => inner.current_space(),
            Self::Embedded(embedded) => embedded.current_space.as_deref(),
        }
    }

    pub fn client(&self) -> &HttpClient {
        match self {
            Self::Http(inner) => inner.client(),
            Self::Embedded(embedded) => &embedded.dummy_client,
        }
    }

    pub fn connection_string(&self) -> String {
        match self {
            Self::Http(inner) => inner.connection_string(),
            Self::Embedded(_) => "embedded:local".to_string(),
        }
    }

    pub fn is_embedded(&self) -> bool {
        matches!(self, Self::Embedded(_))
    }

    pub fn is_read_only(&self) -> bool {
        match self {
            Self::Http(_) => false,
            Self::Embedded(embedded) => embedded.ops.is_read_only(),
        }
    }

    pub fn require_http(&self, command: &str) -> ClientResult<()> {
        if self.is_embedded() {
            return Err(linkrs_client::utils::error::ClientError::command(format!(
                "embedded mode does not support: {}",
                command
            )));
        }
        Ok(())
    }

    pub fn prompt(&self) -> String {
        let base = self
            .session()
            .map(|s| s.prompt())
            .unwrap_or_else(|| "linkrs=# ".to_string());
        if self.is_embedded() && self.is_read_only() {
            base.replacen("# ", "(ro)# ", 1)
        } else {
            base
        }
    }

    pub fn continuation_prompt(&self) -> String {
        self.session()
            .map(|s| s.continuation_prompt())
            .unwrap_or_else(|| "linkrs-# ".to_string())
    }
}
