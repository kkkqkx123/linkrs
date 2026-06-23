use crate::client::{ClientConfig, HttpClient, QueryResult};
use crate::session::variables::VariableStore;
use crate::utils::error::{CliError, Result};

#[derive(Debug, Clone)]
pub struct Session {
    pub session_id: i64,
    pub username: String,
    pub current_space: Option<String>,
    pub host: String,
    pub port: u16,
    pub connected: bool,
    pub variable_store: VariableStore,
}

impl Session {
    pub fn new(session_id: i64, username: String, host: String, port: u16) -> Self {
        Self {
            session_id,
            username,
            current_space: None,
            host,
            port,
            connected: true,
            variable_store: VariableStore::new(),
        }
    }

    pub fn prompt(&self) -> String {
        if !self.connected {
            return "graphdb=# ".to_string();
        }

        let user_part = &self.username;
        let space_part = self.current_space.as_deref().unwrap_or("");

        if space_part.is_empty() {
            format!("graphdb({})=# ", user_part)
        } else {
            format!("graphdb({}:{})=# ", user_part, space_part)
        }
    }

    pub fn continuation_prompt(&self) -> String {
        if !self.connected {
            return "graphdb-# ".to_string();
        }

        let user_part = &self.username;
        let space_part = self.current_space.as_deref().unwrap_or("");

        if space_part.is_empty() {
            format!("graphdb({})-# ", user_part)
        } else {
            format!("graphdb({}:{})-# ", user_part, space_part)
        }
    }

    pub fn set_variable(&mut self, name: String, value: String) -> crate::utils::error::Result<()> {
        self.variable_store.set(name, value)
    }

    pub fn get_variable(&self, name: &str) -> Option<&String> {
        self.variable_store.get(name)
    }

    pub fn remove_variable(&mut self, name: &str) -> bool {
        self.variable_store.remove(name)
    }

    pub fn substitute_variables(&self, input: &str) -> crate::utils::error::Result<String> {
        self.variable_store.substitute(input)
    }

    pub fn conninfo(&self) -> String {
        let mut info = Vec::new();
        info.push(format!("Host: {}:{}", self.host, self.port));
        info.push(format!("Username: {}", self.username));
        info.push(format!(
            "Space: {}",
            self.current_space.as_deref().unwrap_or("(none)")
        ));
        info.push(format!("Session ID: {}", self.session_id));
        info.push(format!("Connected: {}", self.connected));
        info.join("\n")
    }

    #[deprecated(note = "Use variable_store directly")]
    pub fn variables(&self) -> &std::collections::HashMap<String, String> {
        self.variable_store.user_variables()
    }
}

pub struct SessionManager {
    client: HttpClient,
    session: Option<Session>,
    config: ClientConfig,
}

impl SessionManager {
    /// Create a new SessionManager with HTTP connection
    pub fn new_http(host: &str, port: u16) -> Result<Self> {
        let config = ClientConfig::new().with_host(host).with_port(port);
        let client = HttpClient::with_config(config.clone())?;

        Ok(Self {
            client,
            session: None,
            config,
        })
    }

    /// Create a new SessionManager with custom configuration
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let client = HttpClient::with_config(config.clone())?;

        Ok(Self {
            client,
            session: None,
            config,
        })
    }

    /// Create a new SessionManager (legacy method, defaults to HTTP)
    pub fn new(host: &str, port: u16) -> Self {
        Self::new_http(host, port).expect("Failed to create HTTP client")
    }

    /// Connect to the database
    pub async fn connect(&mut self, username: &str, password: &str) -> Result<()> {
        // Update credentials in config
        self.config.username = username.to_string();
        self.config.password = password.to_string();

        // Re-create client with new credentials
        self.client = HttpClient::with_config(self.config.clone())?;

        let session_info = self.client.connect().await?;

        let session = Session::new(
            session_info.session_id,
            session_info.username,
            session_info.host,
            session_info.port,
        );

        self.session = Some(session);
        Ok(())
    }

    /// Connect with specific host and port
    pub async fn connect_with_host(
        &mut self,
        host: &str,
        port: u16,
        username: &str,
        password: &str,
    ) -> Result<()> {
        self.config.host = host.to_string();
        self.config.port = port;
        self.connect(username, password).await
    }

    /// Disconnect from the database
    pub async fn disconnect(&mut self) -> Result<()> {
        if self.session.is_none() {
            return Err(CliError::NotConnected);
        }

        self.client.disconnect().await?;
        self.session = None;
        Ok(())
    }

    /// Switch to a different space
    pub async fn switch_space(&mut self, space: &str) -> Result<()> {
        let session = self.session.as_mut().ok_or(CliError::NotConnected)?;

        self.client.switch_space(space).await?;
        session.current_space = Some(space.to_string());

        Ok(())
    }

    /// Execute a query with variable substitution
    pub async fn execute_query(&self, query: &str) -> Result<QueryResult> {
        let session = self.session.as_ref().ok_or(CliError::NotConnected)?;

        let substituted = session.substitute_variables(query)?;
        self.client
            .execute_query(&substituted, session.session_id)
            .await
    }

    /// Execute a query without variable substitution
    pub async fn execute_query_raw(&self, query: &str) -> Result<QueryResult> {
        let session = self.session.as_ref().ok_or(CliError::NotConnected)?;
        self.client
            .execute_query_raw(query, session.session_id)
            .await
    }

    /// Check server/database health
    pub async fn health_check(&self) -> Result<bool> {
        self.client.health_check().await
    }

    /// List all available spaces
    pub async fn list_spaces(&self) -> Result<Vec<crate::client::SpaceInfo>> {
        self.client.list_spaces().await
    }

    /// List all tags in current space
    pub async fn list_tags(&self) -> Result<Vec<crate::client::TagInfo>> {
        let session = self.session.as_ref().ok_or(CliError::NotConnected)?;
        let space = session
            .current_space
            .as_deref()
            .ok_or(CliError::NoSpaceSelected)?;
        self.client.list_tags(space).await
    }

    /// List all edge types in current space
    pub async fn list_edge_types(&self) -> Result<Vec<crate::client::EdgeTypeInfo>> {
        let session = self.session.as_ref().ok_or(CliError::NotConnected)?;
        let space = session
            .current_space
            .as_deref()
            .ok_or(CliError::NoSpaceSelected)?;
        self.client.list_edge_types(space).await
    }

    /// Get current session reference
    pub fn session(&self) -> Option<&Session> {
        self.session.as_ref()
    }

    /// Get mutable session reference
    pub fn session_mut(&mut self) -> Option<&mut Session> {
        self.session.as_mut()
    }

    /// Check if connected
    pub fn is_connected(&self) -> bool {
        self.session.is_some() && self.client.is_connected()
    }

    /// Get current space name
    pub fn current_space(&self) -> Option<&str> {
        self.session
            .as_ref()
            .and_then(|s| s.current_space.as_deref())
    }

    /// Get client reference
    pub fn client(&self) -> &HttpClient {
        &self.client
    }

    /// Get connection string
    pub fn connection_string(&self) -> String {
        self.client.connection_string()
    }
}
