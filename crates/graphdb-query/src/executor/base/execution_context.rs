use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::execution_result::ExecutionResult;
use super::traits::SearchProvider;
use super::MemoryBudget;
use crate::executor::expression::functions::global_registry_ref;
use crate::executor::expression::functions::OwnedFunctionRef;
use crate::executor::streaming::pool::SharedScheduler;
use crate::optimizer::stats::feedback::history::QueryFeedbackHistory;
use crate::optimizer::JoinAlgorithm;
use crate::storage::QueryStorage;
use graphdb_core::types::expr::expression_context::ExpressionAnalysisContext;
use graphdb_core::Arena;
use graphdb_core::Value;
#[cfg(feature = "fulltext")]
use graphdb_fulltext::manager::FulltextIndexManager;
#[cfg(feature = "vector")]
use graphdb_sync::VectorSyncCoordinator;

/// Search-related runtime state bundled into a single sub-struct.
///
/// Extracted from `ExecutionContext` to reduce feature-flag pollution:
/// adding a new search backend only requires touching this struct and its
/// `Default` impl instead of every `ExecutionContext` constructor.
#[derive(Debug, Clone, Default)]
pub struct SearchContext {
    #[cfg(feature = "fulltext")]
    pub fulltext_manager: Option<Arc<FulltextIndexManager>>,
    #[cfg(feature = "vector")]
    pub vector_coordinator: Option<Arc<VectorSyncCoordinator>>,
    /// Unified search providers for discovery and enumeration.
    pub search_providers: Vec<Arc<dyn SearchProvider>>,
}

#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub results: Arc<RwLock<HashMap<String, ExecutionResult>>>,
    pub variables: Arc<RwLock<HashMap<String, graphdb_core::Value>>>,
    pub expression_context: Arc<ExpressionAnalysisContext>,
    /// Aggregated search backends (fulltext, vector, etc.).
    pub search: SearchContext,
    pub storage: Option<Arc<RwLock<dyn QueryStorage>>>,
    /// Snapshot handle pinned by the bound storage, when the storage was
    /// bound to a read/auto-commit operation context (storage boundary).
    ///
    /// Populated by the pipeline when it binds the per-query storage; lets
    /// the execution layer observe which snapshot the query reads at.
    pub bound_snapshot: Option<crate::storage::SnapshotHandle>,
    pub space_name: Option<String>,
    pub parameters: Arc<HashMap<String, graphdb_core::Value>>,
    /// Session variable snapshot for this query (resolves
    /// `Expression::SessionVariable`); captured once per statement.
    pub session_variables: Arc<HashMap<String, graphdb_core::Value>>,
    /// Per-query memory budget for blocking operators.
    pub memory_budget: MemoryBudget,
    /// Maximum intra-query workers. 1 = serial only.
    pub max_workers: usize,
    /// Server-assigned query ID for KILL QUERY / cancellation support.
    pub query_id: u64,
    /// Request-scoped cancellation token threaded to the runtime/registry.
    pub cancel_token: Option<crate::executor::streaming::query_registry::CancelToken>,
    /// Streaming chunk size (rows per chunk).
    pub chunk_size: usize,
    /// Maximum buffered chunks before back-pressure.
    pub max_buffered_chunks: usize,
    /// M6: Engine-level shared scheduler.
    pub shared_scheduler: Option<Arc<SharedScheduler>>,
    /// Optional thread-safe bumpalo arena for executor temporary allocations.
    pub arena: Option<Arc<parking_lot::Mutex<Arena>>>,
    /// Shared query feedback history for collecting execution statistics.
    ///
    /// Injected from the optimizer engine by the pipeline; the execution
    /// instance records estimated-vs-actual feedback here after execution.
    pub feedback_history: Option<Arc<QueryFeedbackHistory>>,
    /// Shared cross-query policy for the typed columnar chunk layout.
    ///
    /// Injected from the optimizer engine by the pipeline; each query merges
    /// its columnar hit/miss counts back into the policy at completion.
    pub columnar_policy: Option<Arc<crate::executor::streaming::chunk::ColumnarPolicy>>,
    /// Cost-based join algorithm decisions keyed by planner node id
    /// (CBO join-order decision channel).
    ///
    /// Populated by the optimizer from the join reorder walker and consumed
    /// by the arena builder when converting `InnerJoin`/`LeftJoin` nodes.
    /// Absent keys fall back to the default heuristic (hash join for valid
    /// equi keys).
    pub join_algorithms: HashMap<i64, JoinAlgorithm>,
    /// Transaction isolation level for this execution, when running inside
    /// an explicit transaction. Execution-time knob threaded from the
    /// API layer through [`crate::QueryContext`]; `None` = auto-commit.
    pub isolation_level: Option<graphdb_core::types::TransactionIsolationLevel>,
    /// Consistency requirement for secondary-index reads.
    /// `None` = eventual; `Some(cfg)` = read-your-writes with timeout and optional LSN.
    pub ryw_config: Option<graphdb_core::types::ReadYourWritesConfig>,
}

/// Internal: build the non-search portion of an `ExecutionContext`.
fn new_base(expression_context: Arc<ExpressionAnalysisContext>) -> ExecutionContext {
    ExecutionContext {
        results: Arc::new(RwLock::new(HashMap::new())),
        variables: Arc::new(RwLock::new(HashMap::new())),
        expression_context,
        search: SearchContext::default(),
        storage: None,
        bound_snapshot: None,
        space_name: None,
        parameters: Arc::new(HashMap::new()),
        session_variables: Arc::new(HashMap::new()),
        memory_budget: MemoryBudget::default_budget(),
        max_workers: 1,
        query_id: 0,
        cancel_token: None,
        chunk_size: ExecutionContext::DEFAULT_CHUNK_SIZE,
        max_buffered_chunks: ExecutionContext::DEFAULT_MAX_BUFFERED_CHUNKS,
        shared_scheduler: None,
        arena: None,
        feedback_history: None,
        columnar_policy: None,
        join_algorithms: HashMap::new(),
        isolation_level: None,
        ryw_config: None,
    }
}

impl ExecutionContext {
    pub const DEFAULT_CHUNK_SIZE: usize = 2048;
    pub const DEFAULT_MAX_BUFFERED_CHUNKS: usize = 10;

    pub fn new(expression_context: Arc<ExpressionAnalysisContext>) -> Self {
        new_base(expression_context)
    }

    pub fn with_parameters(
        expression_context: Arc<ExpressionAnalysisContext>,
        parameters: HashMap<String, graphdb_core::Value>,
    ) -> Self {
        let mut ctx = new_base(expression_context);
        ctx.parameters = Arc::new(parameters);
        ctx
    }

    /// Create a context with memory limit configured from a config value.
    ///
    /// When `max_memory_per_query` is non-zero it overrides the default
    /// 512 MiB budget; otherwise the default is used.
    pub fn with_memory_limit(
        expression_context: Arc<ExpressionAnalysisContext>,
        max_memory_per_query: u64,
    ) -> Self {
        let budget = if max_memory_per_query > 0 {
            MemoryBudget::new(max_memory_per_query as usize)
        } else {
            MemoryBudget::default_budget()
        };
        let mut ctx = new_base(expression_context);
        ctx.memory_budget = budget;
        ctx
    }

    pub fn set_result(&self, name: String, result: ExecutionResult) {
        self.results.write().insert(name, result);
    }

    pub fn get_result(&self, name: &str) -> Option<ExecutionResult> {
        self.results.write().get(name).cloned()
    }

    pub fn set_variable(&self, name: String, value: graphdb_core::Value) {
        self.variables.write().insert(name, value);
    }

    pub fn get_variable(&self, name: &str) -> Option<graphdb_core::Value> {
        self.variables.write().get(name).cloned()
    }

    pub fn expression_context(&self) -> &Arc<ExpressionAnalysisContext> {
        &self.expression_context
    }

    pub fn get_param(&self, name: &str) -> Option<&graphdb_core::Value> {
        self.parameters.get(name)
    }

    pub fn current_space_id(&self) -> Option<u64> {
        self.variables
            .write()
            .get("space_id")
            .and_then(|v| match v {
                Value::Int(id) => Some(*id as u64),
                _ => None,
            })
    }

    pub fn set_space_id(&self, space_id: u64) {
        self.variables
            .write()
            .insert("space_id".to_string(), Value::Int(space_id as i32));
    }
}

impl Default for ExecutionContext {
    fn default() -> Self {
        let mut ctx = new_base(Arc::new(ExpressionAnalysisContext::new()));
        // Default constructors use default chunk size / max buffered chunks.
        ctx.chunk_size = Self::DEFAULT_CHUNK_SIZE;
        ctx.max_buffered_chunks = Self::DEFAULT_MAX_BUFFERED_CHUNKS;
        ctx
    }
}

impl crate::executor::expression::evaluator::traits::ExpressionContext for ExecutionContext {
    fn get_variable(&self, name: &str) -> Option<Value> {
        self.variables.write().get(name).cloned()
    }

    fn set_variable(&mut self, name: String, value: Value) {
        self.variables.write().insert(name, value);
    }

    fn get_function(&self, name: &str) -> Option<OwnedFunctionRef> {
        let registry = global_registry_ref();
        registry
            .get_builtin(name)
            .map(|f| OwnedFunctionRef::Builtin(f.clone()))
            .or_else(|| {
                registry
                    .get_custom(name)
                    .map(|f| OwnedFunctionRef::Custom(f.clone()))
            })
    }
}
