//! Immutable configuration for unary (one-input) operators.

use graphdb_core::types::expr::Expression;

/// Immutable config for unary (one-input) operators.
#[derive(Debug, Clone)]
pub enum UnarySpec {
    Filter {
        predicate: Expression,
        /// Expression-level subqueries compiled for this filter;
        /// the materializer turns them into a per-operator `SubqueryExecutor`.
        subquery_runners: Vec<crate::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Project {
        output_expressions: Vec<Expression>,
        output_col_names: Vec<String>,
        /// Expression-level subqueries compiled for this project.
        subquery_runners: Vec<crate::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Limit {
        offset: u32,
        limit: u32,
    },
    Assign {
        assignments: Vec<(String, Expression)>,
        /// Expression-level subqueries compiled for this assign.
        subquery_runners: Vec<crate::executor::streaming::subquery::SubqueryRunnerSpec>,
    },
    Remove {
        columns_to_remove: Vec<String>,
    },
    Unwind {
        unwind_column: String,
        list_expression: Option<Expression>,
    },
    /// Storage-backed vertex property append.
    ///
    /// Evaluates `entity_expr` per input row to resolve the vertex id, reads
    /// the vertex (full or projected) from storage, and appends the property
    /// columns to the row.  With a non-empty `prop_names` the appended
    /// columns are the flat `{entity_var}.{prop}` names; with an empty list
    /// the whole `Value::Vertex` is appended under `entity_var`.
    AppendVertices {
        /// Space the vertex is read from.
        space_name: String,
        /// Binding variable of the appended vertex (flat-column prefix).
        entity_var: String,
        /// Expression resolved per row to the vertex id.
        entity_expr: Expression,
        /// Property names to read; empty reads the full vertex.
        prop_names: Vec<String>,
    },
    Sample {
        count: u64,
    },
    Flatten {
        group_pos: u32,
    },
}

impl UnarySpec {
    /// Expression-level subquery runner specs of this operator (empty for
    /// kinds that do not host subqueries). The materializer instantiates a
    /// per-operator `SubqueryExecutor` from these.
    pub fn subquery_runners(&self) -> &[crate::executor::streaming::subquery::SubqueryRunnerSpec] {
        match self {
            Self::Filter {
                subquery_runners, ..
            }
            | Self::Project {
                subquery_runners, ..
            }
            | Self::Assign {
                subquery_runners, ..
            } => subquery_runners,
            _ => &[],
        }
    }
}
