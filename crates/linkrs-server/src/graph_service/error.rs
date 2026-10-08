use linkrs_core::types::Position;

/// Error type for graph service operations, carrying an optional source
/// position for parse errors so the HTTP layer can report it to clients.
#[derive(Debug, Clone)]
pub struct GraphServiceError {
    pub message: String,
    pub position: Option<Position>,
}

impl std::fmt::Display for GraphServiceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for GraphServiceError {}

impl GraphServiceError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            position: None,
        }
    }

    pub fn with_position(message: impl Into<String>, position: Option<Position>) -> Self {
        Self {
            message: message.into(),
            position: position.filter(|p| p.is_valid()),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn position(&self) -> Option<Position> {
        self.position
    }

    pub fn from_core_error(e: linkrs_api::api_core::CoreError) -> Self {
        let message = e.to_string();
        let position = e.error_position().filter(|p| p.is_valid());
        let position = match (position, e.error_offset()) {
            (Some(pos), _) => Some(pos),
            (None, _) => None,
        };
        Self { message, position }
    }

    pub fn from_core_error_with_query(e: linkrs_api::api_core::CoreError, query: &str) -> Self {
        let message = e.to_string();
        if let Some(pos) = e.error_position().filter(|p| p.is_valid()) {
            return Self {
                message,
                position: Some(pos),
            };
        }
        if let Some(offset) = e.error_offset() {
            return Self {
                message,
                position: offset_to_position(query, offset),
            };
        }
        Self {
            message,
            position: None,
        }
    }
}

impl From<String> for GraphServiceError {
    fn from(message: String) -> Self {
        Self::new(message)
    }
}

impl From<&str> for GraphServiceError {
    fn from(message: &str) -> Self {
        Self::new(message)
    }
}

/// Convert a byte offset in query text to a 1-based line and column.
pub(crate) fn offset_to_position(query: &str, offset: usize) -> Option<Position> {
    let offset = offset.min(query.len());
    let prefix = &query[..offset];
    let line = prefix.bytes().filter(|b| *b == b'\n').count() + 1;
    let column = prefix
        .rsplit('\n')
        .next()
        .map(|s| s.chars().count() + 1)
        .unwrap_or(1);
    let position = Position::new(line, column);
    position.is_valid().then_some(position)
}
