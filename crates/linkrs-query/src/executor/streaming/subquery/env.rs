//! Per-evaluation environment threaded into batch expression evaluation.

use std::collections::HashMap;
use std::sync::Arc;

use linkrs_core::Value;

use super::executor::SubqueryExecutor;

/// Bundle of per-evaluation-environment state threaded into batch expression
/// evaluation. Replaces the old standalone `params` argument so
/// future operator kinds only need to add their facility to the env.
#[derive(Debug, Clone, Default)]
pub struct EvalEnv {
    /// Query parameter values (`@name` references).
    pub params: Option<Arc<HashMap<String, Value>>>,
    /// Session variable snapshot (`$name` references), captured once per
    /// statement at the API layer.
    pub session_variables: Option<Arc<HashMap<String, Value>>>,
    /// Expression-level subquery executor of the hosting operator, if any.
    pub subquery_executor: Option<Arc<SubqueryExecutor>>,
    /// Sequence catalog manager for `curr_val` / `next_val`, shared
    /// engine-wide. Populated from the execution runtime at operator open.
    pub sequence_manager: Option<Arc<linkrs_core::metadata::SequenceManager>>,
}

impl EvalEnv {
    /// Build an env carrying only parameter values.
    pub fn from_params(params: Option<Arc<HashMap<String, Value>>>) -> Self {
        Self {
            params,
            session_variables: None,
            subquery_executor: None,
            sequence_manager: None,
        }
    }
}
