//! Request Context Module - manages context information for query requests

use crate::core::ErrorCode;
use crate::core::Value;
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::session_manager::SessionInfo;

/// Request Parameters
#[derive(Debug, Clone)]
pub struct RequestParams {
    pub query: String,
    pub parameters: HashMap<String, Value>,
}

impl RequestParams {
    pub fn new(query: String) -> Self {
        Self {
            query,
            parameters: HashMap::new(),
        }
    }

    pub fn with_parameters(mut self, params: HashMap<String, Value>) -> Self {
        self.parameters = params;
        self
    }
}

/// response object
#[derive(Debug, Clone)]
pub struct Response {
    pub success: bool,
    pub error_code: ErrorCode,
    pub data: Option<Value>,
    pub error_message: Option<String>,
    pub execution_time_ms: u64,
    pub affected_rows: u64,
    pub warnings: Vec<String>,
}

impl Response {
    pub fn new(success: bool) -> Self {
        Self {
            success,
            error_code: if success {
                ErrorCode::Success
            } else {
                ErrorCode::Unknown
            },
            data: None,
            error_message: None,
            execution_time_ms: 0,
            affected_rows: 0,
            warnings: Vec::new(),
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }

    pub fn with_error(mut self, error: String) -> Self {
        self.error_message = Some(error);
        self.success = false;
        self.error_code = ErrorCode::ExecutionError;
        self
    }

    pub fn with_error_code(mut self, code: ErrorCode) -> Self {
        self.error_code = code;
        if code != ErrorCode::Success {
            self.success = false;
        }
        self
    }

    pub fn with_execution_time(mut self, time_ms: u64) -> Self {
        self.execution_time_ms = time_ms;
        self
    }

    pub fn with_affected_rows(mut self, rows: u64) -> Self {
        self.affected_rows = rows;
        self
    }

    pub fn with_warning(mut self, warning: String) -> Self {
        self.warnings.push(warning);
        self
    }

    pub fn is_success(&self) -> bool {
        self.success
    }

    pub fn get_data(&self) -> Option<&Value> {
        self.data.as_ref()
    }

    pub fn get_error(&self) -> Option<&String> {
        self.error_message.as_ref()
    }
}

/// request context
///
/// Manage the complete lifecycle of a query request, including:
/// 1. Session information management
/// 2. Request parameter management
/// 3. Response object management
#[derive(Debug)]
pub struct RequestContext {
    // session information
    session_info: Option<SessionInfo>,

    // Request Parameters
    request_params: Arc<RequestParams>,

    // Response Objects - Using RwLock to Support Internal Mutability
    response: Arc<RwLock<Response>>,

    // Inquiry start time
    query_start_time: Instant,
}

impl RequestContext {
    /// Creating a new request context
    pub fn new(session_info: Option<SessionInfo>, request_params: RequestParams) -> Self {
        Self {
            session_info,
            request_params: Arc::new(request_params),
            response: Arc::new(RwLock::new(Response::new(true))),
            query_start_time: Instant::now(),
        }
    }

    /// Creating a request context with session information
    pub fn with_session(
        query: String,
        session_id: &str,
        user_name: &str,
        client_ip: &str,
        client_port: u16,
    ) -> Result<Self, String> {
        let session_info =
            SessionInfo::from_params(session_id, user_name, None, client_ip, client_port)?;
        let request_params = RequestParams::new(query);
        Ok(Self::new(Some(session_info), request_params))
    }

    /// Creating a request context with parameters
    pub fn with_parameters(
        query: String,
        parameters: HashMap<String, Value>,
        session_id: &str,
        user_name: &str,
        client_ip: &str,
        client_port: u16,
    ) -> Result<Self, String> {
        let session_info =
            SessionInfo::from_params(session_id, user_name, None, client_ip, client_port)?;
        let request_params = RequestParams::new(query).with_parameters(parameters);
        Ok(Self::new(Some(session_info), request_params))
    }

    /// Get session information
    pub fn session_info(&self) -> Option<&SessionInfo> {
        self.session_info.as_ref()
    }

    /// Get request parameters
    pub fn request_params(&self) -> &RequestParams {
        &self.request_params
    }

    /// Get query string
    pub fn query(&self) -> &str {
        &self.request_params.query
    }

    /// Getting Parameters
    pub fn parameters(&self) -> &HashMap<String, Value> {
        &self.request_params.parameters
    }

    /// Get Response
    pub fn response(&self) -> Response {
        self.response.read().clone()
    }

    /// Setting the response
    pub fn set_response(&self, response: Response) {
        let mut guard = self.response.write();
        *guard = response;
    }

    /// Get Session ID
    pub fn session_id(&self) -> Option<i64> {
        self.session_info.as_ref().map(|s| s.session_id)
    }

    /// Get User Name
    pub fn user_name(&self) -> Option<&str> {
        self.session_info.as_ref().map(|s| s.user_name.as_str())
    }

    /// Get the name of the graph space
    pub fn space_name(&self) -> Option<&str> {
        self.session_info
            .as_ref()
            .and_then(|s| s.space_name.as_deref())
    }

    /// Setting the name of the map space
    pub fn set_space_name(&mut self, space_name: String) {
        if let Some(ref mut session) = self.session_info {
            session.space_name = Some(space_name);
        }
    }

    /// Setting Response Errors
    pub fn set_response_error(&self, error: String) {
        let mut guard = self.response.write();
        guard.success = false;
        guard.error_code = ErrorCode::ExecutionError;
        guard.error_message = Some(error);
    }

    /// Setting Response Errors with Error Codes
    pub fn set_response_error_with_code(&self, error: String, code: ErrorCode) {
        let mut guard = self.response.write();
        guard.success = false;
        guard.error_code = code;
        guard.error_message = Some(error);
    }

    /// Adding a Warning Message
    pub fn add_warning(&self, warning: String) {
        let mut guard = self.response.write();
        guard.warnings.push(warning);
    }

    /// Setting the execution time
    pub fn set_execution_time(&self) {
        let elapsed = self.query_start_time.elapsed().as_millis() as u64;
        let mut guard = self.response.write();
        guard.execution_time_ms = elapsed;
    }

    /// Get execution time in milliseconds
    pub fn elapsed_ms(&self) -> u64 {
        self.query_start_time.elapsed().as_millis() as u64
    }

    /// Getting Parameters
    pub fn get_parameter(&self, param: &str) -> Option<Value> {
        self.request_params.parameters.get(param).cloned()
    }

    /// Update session last access time
    pub fn touch_session(&mut self) {
        if let Some(ref mut session) = self.session_info {
            session.touch();
        }
    }
}

impl Default for RequestContext {
    fn default() -> Self {
        Self {
            session_info: None,
            request_params: Arc::new(RequestParams::new(String::new())),
            response: Arc::new(RwLock::new(Response::new(true))),
            query_start_time: Instant::now(),
        }
    }
}

impl Clone for RequestContext {
    fn clone(&self) -> Self {
        Self {
            session_info: self.session_info.clone(),
            request_params: self.request_params.clone(),
            response: self.response.clone(),
            query_start_time: self.query_start_time,
        }
    }
}

/// Creating a QueryRequestContext from a ClientSession
///
/// This conversion function ensures that the session information from the api layer is correctly passed to the query layer
pub fn build_query_request_context(
    session: &super::ClientSession,
    query: String,
    parameters: std::collections::HashMap<String, crate::core::Value>,
) -> crate::query::QueryRequestContext {
    use crate::query::QueryRequestContext;

    QueryRequestContext {
        session_id: Some(session.id()),
        user_name: Some(session.user()),
        space_name: session.space_name(),
        query,
        parameters,
    }
}
