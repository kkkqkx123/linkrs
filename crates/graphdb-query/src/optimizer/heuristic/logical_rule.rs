//! Logical rewrite rule trait.
//!
//! Heuristic rules operating on the pure logical tree
//! (`LogicalNodeEnum`). This mirrors the physical `RewriteRule`
//! (`rule.rs`) but targets logical operators only, keeping logical and
//! physical optimization boundaries explicit.

use std::sync::Arc;

use crate::optimizer::error::OptimizeResult;
use crate::optimizer::stats::StatisticsManager;
use crate::planning::plan::logical::LogicalNodeEnum;

/// Context shared by logical rewrite rules.
pub struct LogicalRuleContext {
    /// Statistics manager for selectivity / cardinality lookups.
    pub stats: Arc<StatisticsManager>,
    /// Whether any rule has changed the tree so far.
    pub changed: bool,
}

impl LogicalRuleContext {
    /// Create a context backed by a fresh statistics manager.
    pub fn new() -> Self {
        Self {
            stats: Arc::new(StatisticsManager::new()),
            changed: false,
        }
    }

    /// Create a context sharing the given statistics manager.
    pub fn with_stats(stats: Arc<StatisticsManager>) -> Self {
        Self {
            stats,
            changed: false,
        }
    }

    /// Record that the tree was changed.
    pub fn mark_changed(&mut self) {
        self.changed = true;
    }
}

impl Default for LogicalRuleContext {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for LogicalRuleContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogicalRuleContext")
            .field("changed", &self.changed)
            .finish_non_exhaustive()
    }
}

/// Rewrite rule operating on `LogicalNodeEnum`.
///
/// Rules identify a local logical pattern and rewrite it in place,
/// returning `true` when the node was changed.
pub trait LogicalRule: std::fmt::Debug + Send + Sync {
    /// Rule name for diagnostics.
    fn name(&self) -> &str;

    /// Apply the rule to `node`, returning `true` on change.
    fn apply(
        &self,
        node: &mut LogicalNodeEnum,
        ctx: &mut LogicalRuleContext,
    ) -> OptimizeResult<bool>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct NoopRule;

    impl LogicalRule for NoopRule {
        fn name(&self) -> &str {
            "NoopRule"
        }

        fn apply(
            &self,
            _node: &mut LogicalNodeEnum,
            _ctx: &mut LogicalRuleContext,
        ) -> OptimizeResult<bool> {
            Ok(false)
        }
    }

    #[test]
    fn logical_rule_context_defaults() {
        let ctx = LogicalRuleContext::new();
        assert!(!ctx.changed);
        let rule = NoopRule;
        assert_eq!(rule.name(), "NoopRule");
    }
}
