//! Type-safe path definitions for HTTP routes
//!
//! This module provides compile-time and runtime validation for route paths,
//! preventing issues like using `:param` syntax instead of `{param}` in axum 0.7+.

use regex::Regex;
use std::sync::OnceLock;

/// Validates that a path uses the correct axum 0.7+ syntax `{param}` instead of `:param`
pub fn validate_path(path: &str) -> Result<(), PathValidationError> {
    static OLD_SYNTAX_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = OLD_SYNTAX_REGEX
        .get_or_init(|| Regex::new(r":([a-zA-Z_][a-zA-Z0-9_]*)").expect("Invalid regex pattern"));

    if let Some(captures) = regex.captures(path) {
        let param_name = captures.get(1).map(|m| m.as_str()).unwrap_or("");
        return Err(PathValidationError::OldSyntax {
            path: path.to_string(),
            param: param_name.to_string(),
        });
    }

    Ok(())
}

/// Error type for path validation failures
#[derive(Debug, Clone, PartialEq)]
pub enum PathValidationError {
    OldSyntax { path: String, param: String },
}

impl std::fmt::Display for PathValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PathValidationError::OldSyntax { path, param } => {
                write!(
                    f,
                    "Path '{}' uses old syntax ':{}'. Use '{{{}}}' instead (axum 0.7+ syntax)",
                    path, param, param
                )
            }
        }
    }
}

impl std::error::Error for PathValidationError {}

/// A type-safe path that validates the format at construction time
#[derive(Debug, Clone, PartialEq)]
pub struct TypedPath(String);

impl TypedPath {
    /// Creates a new TypedPath, validating the format
    pub fn new(path: &str) -> Result<Self, PathValidationError> {
        validate_path(path)?;
        Ok(Self(path.to_string()))
    }

    /// Creates a new TypedPath without validation (use with caution)
    pub fn new_unchecked(path: &str) -> Self {
        Self(path.to_string())
    }

    /// Returns the path string
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Creates a path with a single parameter
    pub fn with_param(base: &str, param_name: &str) -> Self {
        Self(format!("{}/{{{}}}", base.trim_end_matches('/'), param_name))
    }

    /// Creates a path with multiple parameters
    pub fn with_params(base: &str, params: &[&str]) -> Self {
        let mut path = base.trim_end_matches('/').to_string();
        for param in params {
            path.push_str(&format!("/{{{}}}", param));
        }
        Self(path)
    }

    /// Appends a static segment to the path
    pub fn append(mut self, segment: &str) -> Self {
        self.0.push_str(segment);
        Self::new(&self.0).unwrap_or(self)
    }
}

impl AsRef<str> for TypedPath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl From<TypedPath> for String {
    fn from(path: TypedPath) -> Self {
        path.0
    }
}

/// Predefined path segments for common patterns
pub mod segments {
    use super::TypedPath;

    /// Creates a path with an `id` parameter: `/resource/{id}`
    pub fn with_id(resource: &str) -> TypedPath {
        TypedPath::with_param(resource, "id")
    }

    /// Creates a nested resource path: `/parent/{parent_id}/children`
    pub fn nested(parent: &str, parent_id: &str, child: &str) -> TypedPath {
        TypedPath(format!("/{}/{{{}}}/{}", parent, parent_id, child))
    }

    /// Creates a nested resource with its own id: `/parent/{id}/child/{child_id}`
    pub fn nested_with_id(parent: &str, child: &str) -> TypedPath {
        TypedPath(format!("/{}/{{id}}/{}/{{{}_id}}", parent, child, child))
    }
}

/// Macro for compile-time path validation
#[macro_export]
macro_rules! path {
    ($path:literal) => {{
        // Runtime validation in debug builds
        #[cfg(debug_assertions)]
        {
            use $crate::api::server::http::typed_path::validate_path;
            validate_path($path).expect("Invalid path format")
        }
        $path
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_path_valid() {
        assert!(validate_path("/sessions/{id}").is_ok());
        assert!(validate_path("/config/{section}/{key}").is_ok());
        assert!(validate_path("/static/path").is_ok());
    }

    #[test]
    fn test_validate_path_invalid() {
        let result = validate_path("/sessions/:id");
        assert!(matches!(
            result,
            Err(PathValidationError::OldSyntax { param, .. }) if param == "id"
        ));

        let result = validate_path("/config/:section/:key");
        assert!(matches!(
            result,
            Err(PathValidationError::OldSyntax { param, .. }) if param == "section"
        ));
    }

    #[test]
    fn test_typed_path_new() {
        let path = TypedPath::new("/sessions/{id}").unwrap();
        assert_eq!(path.as_str(), "/sessions/{id}");

        let result = TypedPath::new("/sessions/:id");
        assert!(result.is_err());
    }

    #[test]
    fn test_typed_path_with_param() {
        let path = TypedPath::with_param("/sessions", "id");
        assert_eq!(path.as_str(), "/sessions/{id}");
    }

    #[test]
    fn test_segments_with_id() {
        let path = segments::with_id("sessions");
        assert_eq!(path.as_str(), "sessions/{id}");
    }
}
