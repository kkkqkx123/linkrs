//! Schema Service

/// Schema service for extended schema operations
pub struct SchemaService;

impl SchemaService {
    /// Create a new schema service
    pub fn new() -> Self {
        Self
    }
}

impl Default for SchemaService {
    fn default() -> Self {
        Self::new()
    }
}
