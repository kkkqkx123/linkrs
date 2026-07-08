//! Query validation types

/// Query validation result
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<ValidationError>,
    pub warnings: Vec<ValidationWarning>,
    pub estimated_cost: Option<u64>,
}

/// Validation error
#[derive(Debug, Clone)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub position: Option<usize>,
    pub line: Option<usize>,
    pub column: Option<usize>,
}

/// Validation warning
#[derive(Debug, Clone)]
pub struct ValidationWarning {
    pub code: String,
    pub message: String,
    pub suggestion: Option<String>,
}
