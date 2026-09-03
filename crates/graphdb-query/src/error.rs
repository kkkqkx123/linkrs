use std::fmt;

use crate::binder::BoundStatement;
use crate::executor::build_error::PlanBuildError;
use crate::optimizer::error::OptimizeError;
use crate::parser::core::error::ParseError;
use crate::planning::planner::PlannerError;

/// Classification of the statement being processed, used for
/// diagnostic context in pipeline errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementType {
    Match,
    Go,
    Lookup,
    Insert,
    Update,
    Delete,
    Merge,
    Create,
    Drop,
    Alter,
    Explain,
    Other,
}

impl StatementType {
    pub fn from_bound(bound: &BoundStatement) -> Self {
        match bound {
            BoundStatement::Match(_) => StatementType::Match,
            BoundStatement::Go(_) => StatementType::Go,
            BoundStatement::Lookup(_) => StatementType::Lookup,
            BoundStatement::Insert(_) => StatementType::Insert,
            BoundStatement::Update(_) => StatementType::Update,
            BoundStatement::Delete(_) => StatementType::Delete,
            BoundStatement::Merge(_) => StatementType::Merge,
            BoundStatement::Create(_) => StatementType::Create,
            BoundStatement::Drop(_) => StatementType::Drop,
            BoundStatement::Alter(_) => StatementType::Alter,
            _ => StatementType::Other,
        }
    }
}

impl fmt::Display for StatementType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatementType::Match => write!(f, "MATCH"),
            StatementType::Go => write!(f, "GO"),
            StatementType::Lookup => write!(f, "LOOKUP"),
            StatementType::Insert => write!(f, "INSERT"),
            StatementType::Update => write!(f, "UPDATE"),
            StatementType::Delete => write!(f, "DELETE"),
            StatementType::Merge => write!(f, "MERGE"),
            StatementType::Create => write!(f, "CREATE"),
            StatementType::Drop => write!(f, "DROP"),
            StatementType::Alter => write!(f, "ALTER"),
            StatementType::Explain => write!(f, "EXPLAIN"),
            StatementType::Other => write!(f, "OTHER"),
        }
    }
}

/// Pipeline phase where the error occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelinePhase {
    Parse,
    Bind,
    Plan,
    Optimize,
    Execute,
}

impl fmt::Display for PipelinePhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PipelinePhase::Parse => write!(f, "parse"),
            PipelinePhase::Bind => write!(f, "bind"),
            PipelinePhase::Plan => write!(f, "plan"),
            PipelinePhase::Optimize => write!(f, "optimize"),
            PipelinePhase::Execute => write!(f, "execute"),
        }
    }
}

/// Unified error type for the query pipeline.
///
/// Each variant preserves the originating error and adds pipeline-specific
/// context so that upstream callers can inspect *what* failed and *why*
/// without parsing free-text messages.
#[derive(Debug)]
pub enum QueryPipelineError {
    /// Parse-phase failure.
    Parse {
        source: Box<ParseError>,
        query_text: String,
    },
    /// Planning-phase failure.
    Planning {
        source: PlannerError,
        statement_type: StatementType,
        space_name: Option<String>,
    },
    /// Optimizer-phase failure.
    Optimization {
        source: OptimizeError,
        rule_name: Option<String>,
    },
    /// Physical plan construction failure.
    Execution {
        source: PlanBuildError,
        executor_type: String,
    },
    /// Generic pipeline-level error (catch-all for storage, binding, etc.)
    Pipeline {
        phase: PipelinePhase,
        message: String,
    },
}

impl fmt::Display for QueryPipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QueryPipelineError::Parse { source, query_text } => {
                write!(
                    f,
                    "Parse error in query {:?}: {}",
                    truncate(query_text, 120),
                    source
                )
            }
            QueryPipelineError::Planning {
                source,
                statement_type,
                space_name,
            } => {
                write!(
                    f,
                    "Planning error for {} statement{}: {}",
                    statement_type,
                    space_name
                        .as_deref()
                        .map(|s| format!(" in space '{}'", s))
                        .unwrap_or_default(),
                    source
                )
            }
            QueryPipelineError::Optimization { source, rule_name } => {
                write!(
                    f,
                    "Optimization error{}: {}",
                    rule_name
                        .as_deref()
                        .map(|r| format!(" (rule: {})", r))
                        .unwrap_or_default(),
                    source
                )
            }
            QueryPipelineError::Execution {
                source,
                executor_type,
            } => {
                write!(f, "Execution error ({}): {}", executor_type, source)
            }
            QueryPipelineError::Pipeline { phase, message } => {
                write!(f, "{} phase error: {}", phase, message)
            }
        }
    }
}

impl std::error::Error for QueryPipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            QueryPipelineError::Parse { source, .. } => Some(source),
            QueryPipelineError::Planning { source, .. } => Some(source),
            QueryPipelineError::Optimization { source, .. } => Some(source),
            QueryPipelineError::Execution { source, .. } => Some(source),
            QueryPipelineError::Pipeline { .. } => None,
        }
    }
}

impl From<ParseError> for QueryPipelineError {
    fn from(source: ParseError) -> Self {
        QueryPipelineError::Parse {
            source: Box::new(source),
            query_text: String::new(),
        }
    }
}

impl From<PlannerError> for QueryPipelineError {
    fn from(source: PlannerError) -> Self {
        QueryPipelineError::Planning {
            source,
            statement_type: StatementType::Other,
            space_name: None,
        }
    }
}

impl From<OptimizeError> for QueryPipelineError {
    fn from(source: OptimizeError) -> Self {
        QueryPipelineError::Optimization {
            source,
            rule_name: None,
        }
    }
}

impl From<PlanBuildError> for QueryPipelineError {
    fn from(source: PlanBuildError) -> Self {
        QueryPipelineError::Execution {
            source,
            executor_type: "physical_plan_builder".to_string(),
        }
    }
}

impl From<QueryPipelineError> for graphdb_core::error::DBError {
    fn from(err: QueryPipelineError) -> Self {
        use graphdb_core::error::query::QueryPhase;
        let phase = match &err {
            QueryPipelineError::Parse { .. } => QueryPhase::Parse,
            QueryPipelineError::Planning { .. } => QueryPhase::Plan,
            QueryPipelineError::Optimization { .. } => QueryPhase::Optimize,
            QueryPipelineError::Execution { .. } => QueryPhase::Execute,
            QueryPipelineError::Pipeline { phase, .. } => match phase {
                PipelinePhase::Parse => QueryPhase::Parse,
                PipelinePhase::Bind => QueryPhase::Validate,
                PipelinePhase::Plan => QueryPhase::Plan,
                PipelinePhase::Optimize => QueryPhase::Optimize,
                PipelinePhase::Execute => QueryPhase::Execute,
            },
        };
        graphdb_core::error::DBError::from(graphdb_core::error::QueryError::pipeline_error(
            phase,
            err.to_string(),
        ))
    }
}

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        format!("{}...", &s[..max_chars])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_error_display() {
        let err = QueryPipelineError::Pipeline {
            phase: PipelinePhase::Bind,
            message: "missing space name".to_string(),
        };
        assert!(err.to_string().contains("bind phase error"));
        assert!(err.to_string().contains("missing space name"));
    }

    #[test]
    fn test_statement_type_display() {
        assert_eq!(StatementType::Match.to_string(), "MATCH");
        assert_eq!(StatementType::Go.to_string(), "GO");
        assert_eq!(StatementType::Other.to_string(), "OTHER");
    }
}
