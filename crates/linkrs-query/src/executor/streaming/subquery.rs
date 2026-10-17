//! Expression-level subquery execution.
//!
//! EXISTS / IN subqueries at non-conjunctive expression positions (WHERE
//! residual, RETURN, HAVING, WITH assignments, ...) are compiled at planning
//! time by `plan_expression_subqueries` into standalone sub-plans. Each
//! compiled subquery becomes a [`SubqueryRunnerSpec`] on the hosting
//! Filter/Project/Assign arena spec; the materializer instantiates a
//! per-operator [`SubqueryExecutor`] from those specs.
//!
//! Execution model:
//! - Every operator instance owns exactly one [`SubqueryExecutor`]; parallel
//!   partitions never share mutable runner state.
//! - A runner materializes its sub-plan **once** on first use and re-runs it
//!   per row via the reset protocol (`StreamingExecutor::reset`), never
//!   rebuilding the pipeline.
//! - Correlated runners receive the current row as a private correlation
//!   frame (`StreamingExecutor::inject_correlation_frame`); the frame is
//!   injected into the sub-plan's `Argument` source slot.
//! - Non-correlated runners evaluate once and cache the result (EXISTS →
//!   bool, IN → `HashSet<Value>` with NULLs removed).
//!
//! NULL semantics: a NULL left operand or NULL result values never
//! match, consistent with the conjunctive `keys_match` path.

mod env;
mod executor;
mod runner;

#[cfg(test)]
mod tests;

pub use env::EvalEnv;
pub use executor::SubqueryExecutor;
pub use runner::{GroupJoinSpec, SubqueryRunner, SubqueryRunnerSpec};
