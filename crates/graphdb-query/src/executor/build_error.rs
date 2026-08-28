use std::fmt;

use graphdb_core::error::QueryError;

/// Structured error type for physical plan construction failures.
///
/// Every variant carries a stable discriminant so callers can match on the
/// specific kind of error without parsing a free-text message.
#[derive(Debug, Clone)]
pub enum PlanBuildError {
    /// The plan node type has no physical executor implementation.
    UnsupportedNode {
        node_type: String,
        node_id: i64,
        detail: String,
    },
    /// A required configuration value is missing from the plan or context.
    MissingRequiredValue {
        node_type: String,
        node_id: i64,
        field: String,
        detail: String,
    },
    /// The runtime does not provide a capability that the plan requires.
    CapabilityUnavailable { capability: String, detail: String },
    /// The plan requires a transaction mode that does not match the current scope.
    InvalidTransactionMode {
        required: String,
        actual: String,
        detail: String,
    },
    /// An expression references a slot, parameter, or variable that cannot be bound.
    ExpressionBinding {
        node_type: String,
        node_id: i64,
        expression: String,
        detail: String,
    },
}

impl PlanBuildError {
    pub fn unsupported(
        node_type: impl Into<String>,
        node_id: i64,
        detail: impl Into<String>,
    ) -> Self {
        Self::UnsupportedNode {
            node_type: node_type.into(),
            node_id,
            detail: detail.into(),
        }
    }

    pub fn missing_value(
        node_type: impl Into<String>,
        node_id: i64,
        field: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::MissingRequiredValue {
            node_type: node_type.into(),
            node_id,
            field: field.into(),
            detail: detail.into(),
        }
    }

    pub fn capability(capability: impl Into<String>, detail: impl Into<String>) -> Self {
        Self::CapabilityUnavailable {
            capability: capability.into(),
            detail: detail.into(),
        }
    }

    pub fn expression(
        node_type: impl Into<String>,
        node_id: i64,
        expression: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::ExpressionBinding {
            node_type: node_type.into(),
            node_id,
            expression: expression.into(),
            detail: detail.into(),
        }
    }
}

impl fmt::Display for PlanBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedNode {
                node_type,
                node_id,
                detail,
            } => {
                write!(
                    f,
                    "Unsupported node '{}' (id={}): {}",
                    node_type, node_id, detail
                )
            }
            Self::MissingRequiredValue {
                node_type,
                node_id,
                field,
                detail,
            } => {
                write!(
                    f,
                    "Missing required value '{}' for node '{}' (id={}): {}",
                    field, node_type, node_id, detail
                )
            }
            Self::CapabilityUnavailable { capability, detail } => {
                write!(
                    f,
                    "Capability '{}' is not available: {}",
                    capability, detail
                )
            }
            Self::InvalidTransactionMode {
                required,
                actual,
                detail,
            } => {
                write!(
                    f,
                    "Transaction mode mismatch: required '{}', actual '{}': {}",
                    required, actual, detail
                )
            }
            Self::ExpressionBinding {
                node_type,
                node_id,
                expression,
                detail,
            } => {
                write!(
                    f,
                    "Expression binding error in node '{}' (id={}): '{}' - {}",
                    node_type, node_id, expression, detail
                )
            }
        }
    }
}

impl std::error::Error for PlanBuildError {}

impl From<PlanBuildError> for QueryError {
    fn from(e: PlanBuildError) -> Self {
        QueryError::execution(e.to_string())
    }
}
