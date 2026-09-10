//! Experimental query-pipeline extension points.
//!
//! Unstable API: trait signatures are intentionally small (single method
//! with a default `None`) and may evolve without the usual compatibility
//! guarantees while the pipeline integration matures.
//!
//! Mirrors the ladybug four-piece set (`TransformerExtension` /
//! `BinderExtension` / `PlannerExtension` / `MapperExtension`) with this
//! project's own types. All hooks are opt-in: an empty registry leaves the
//! builtin pipeline untouched.
//!
//! Semantics match ladybug: extensions are tried in registration order, the
//! first `Some` wins, `None` falls through to the builtin logic. A panicking
//! extension is isolated with `catch_unwind`, logged, and treated as `None`
//! so a broken extension can never break queries.
//!
//! This module is experimental: trait signatures are intentionally small
//! (single method + default `None`) and may evolve.

use std::sync::Arc;

use graphdb_core::error::DBResult;
use parking_lot::RwLock;

use crate::binder::BoundStatement;
use crate::parser::ast::stmt::Ast;
use crate::planning::plan::ExecutionPlan;
use crate::planning::plan::SubPlan;

/// Input context for [`BinderExtension::bind`].
#[derive(Debug, Clone, Default)]
pub struct BinderExtensionContext {
    pub space_name: Option<String>,
    pub space_id: u64,
}

/// Rewrite the raw query text before parsing.
///
/// Return `None` to decline (builtin parsing proceeds).
pub trait ParserExtension: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn transform(&self, _query_text: &str) -> Option<DBResult<String>> {
        None
    }
}

/// Bind a parsed AST into a [`BoundStatement`], bypassing the builtin binder.
///
/// Return `None` to decline (builtin binding proceeds).
pub trait BinderExtension: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn bind(
        &self,
        _ast: &Arc<Ast>,
        _ctx: &BinderExtensionContext,
    ) -> Option<DBResult<BoundStatement>> {
        None
    }
}

/// Plan a bound statement into a [`SubPlan`], bypassing builtin planners.
///
/// Return `None` to decline (builtin planning proceeds).
pub trait PlannerExtension: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn plan(&self, _bound: &BoundStatement) -> Option<DBResult<SubPlan>> {
        None
    }
}

/// Inspect or rewrite an optimized [`ExecutionPlan`] at the mapper boundary
/// (after logical optimization, before physical plan building).
///
/// Return `None` to decline (builtin plan is used as-is).
pub trait MapperExtension: Send + Sync + std::fmt::Debug {
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn map(&self, _plan: &ExecutionPlan) -> Option<DBResult<ExecutionPlan>> {
        None
    }
}

/// Ordered registry for pipeline extensions.
///
/// `Arc`-shared so the pipeline manager and tests can hold the same instance.
#[derive(Debug, Default)]
pub struct ExtensionRegistry {
    parsers: RwLock<Vec<Arc<dyn ParserExtension>>>,
    binders: RwLock<Vec<Arc<dyn BinderExtension>>>,
    planners: RwLock<Vec<Arc<dyn PlannerExtension>>>,
    mappers: RwLock<Vec<Arc<dyn MapperExtension>>>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_parser_extension(&self, extension: Arc<dyn ParserExtension>) {
        self.parsers.write().push(extension);
    }

    pub fn add_binder_extension(&self, extension: Arc<dyn BinderExtension>) {
        self.binders.write().push(extension);
    }

    pub fn add_planner_extension(&self, extension: Arc<dyn PlannerExtension>) {
        self.planners.write().push(extension);
    }

    pub fn add_mapper_extension(&self, extension: Arc<dyn MapperExtension>) {
        self.mappers.write().push(extension);
    }

    pub fn parser_extension_count(&self) -> usize {
        self.parsers.read().len()
    }

    pub fn binder_extension_count(&self) -> usize {
        self.binders.read().len()
    }

    pub fn planner_extension_count(&self) -> usize {
        self.planners.read().len()
    }

    pub fn mapper_extension_count(&self) -> usize {
        self.mappers.read().len()
    }

    fn snapshot_parsers(&self) -> Vec<Arc<dyn ParserExtension>> {
        let guard = self.parsers.read();
        if guard.is_empty() {
            return Vec::new();
        }
        guard.clone()
    }

    fn snapshot_binders(&self) -> Vec<Arc<dyn BinderExtension>> {
        let guard = self.binders.read();
        if guard.is_empty() {
            return Vec::new();
        }
        guard.clone()
    }

    fn snapshot_planners(&self) -> Vec<Arc<dyn PlannerExtension>> {
        let guard = self.planners.read();
        if guard.is_empty() {
            return Vec::new();
        }
        guard.clone()
    }

    fn snapshot_mappers(&self) -> Vec<Arc<dyn MapperExtension>> {
        let guard = self.mappers.read();
        if guard.is_empty() {
            return Vec::new();
        }
        guard.clone()
    }

    /// First `Some` wins; panics are isolated to `None`.
    pub fn try_parse_transform(&self, query_text: &str) -> Option<DBResult<String>> {
        for extension in self.snapshot_parsers() {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                extension.transform(query_text)
            }));
            match outcome {
                Ok(Some(result)) => return Some(result),
                Ok(None) => continue,
                Err(_) => {
                    log::error!(
                        "parser extension {} panicked; falling back to builtin",
                        extension.name()
                    );
                }
            }
        }
        None
    }

    /// First `Some` wins; panics are isolated to `None`.
    pub fn try_bind(
        &self,
        ast: &Arc<Ast>,
        ctx: &BinderExtensionContext,
    ) -> Option<DBResult<BoundStatement>> {
        for extension in self.snapshot_binders() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extension.bind(ast, ctx)));
            match outcome {
                Ok(Some(result)) => return Some(result),
                Ok(None) => continue,
                Err(_) => {
                    log::error!(
                        "binder extension {} panicked; falling back to builtin",
                        extension.name()
                    );
                }
            }
        }
        None
    }

    /// First `Some` wins; panics are isolated to `None`.
    pub fn try_plan(&self, bound: &BoundStatement) -> Option<DBResult<SubPlan>> {
        for extension in self.snapshot_planners() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extension.plan(bound)));
            match outcome {
                Ok(Some(result)) => return Some(result),
                Ok(None) => continue,
                Err(_) => {
                    log::error!(
                        "planner extension {} panicked; falling back to builtin",
                        extension.name()
                    );
                }
            }
        }
        None
    }

    /// First `Some` wins; panics are isolated to `None`.
    pub fn try_map(&self, plan: &ExecutionPlan) -> Option<DBResult<ExecutionPlan>> {
        for extension in self.snapshot_mappers() {
            let outcome =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| extension.map(plan)));
            match outcome {
                Ok(Some(result)) => return Some(result),
                Ok(None) => continue,
                Err(_) => {
                    log::error!(
                        "mapper extension {} panicked; falling back to builtin",
                        extension.name()
                    );
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::Parser;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn parse_ast(query: &str) -> Arc<Ast> {
        let mut parser = Parser::new(query);
        let result = parser.parse().expect("test query parses");
        assert!(!parser.has_errors());
        result.ast
    }

    #[derive(Debug)]
    struct ShowInterceptor {
        hits: Arc<AtomicUsize>,
    }

    impl BinderExtension for ShowInterceptor {
        fn bind(
            &self,
            ast: &Arc<Ast>,
            _ctx: &BinderExtensionContext,
        ) -> Option<DBResult<BoundStatement>> {
            if matches!(ast.stmt(), crate::parser::ast::Stmt::Show(_)) {
                self.hits.fetch_add(1, Ordering::SeqCst);
                // Rewrite: collapse any SHOW into a synthetic SHOW SPACES form
                // by reusing the parsed statement as opaque payload.
                return Some(Ok(BoundStatement::Other(Box::new(ast.stmt().clone()))));
            }
            None
        }
    }

    #[derive(Debug)]
    struct PanickingBinder;

    impl BinderExtension for PanickingBinder {
        fn bind(
            &self,
            _ast: &Arc<Ast>,
            _ctx: &BinderExtensionContext,
        ) -> Option<DBResult<BoundStatement>> {
            panic!("extension boom");
        }
    }

    #[derive(Debug)]
    struct PanickingPlanner;

    impl PlannerExtension for PanickingPlanner {
        fn plan(&self, _bound: &BoundStatement) -> Option<DBResult<SubPlan>> {
            panic!("planner boom");
        }
    }

    #[derive(Debug)]
    struct OtherClaimingPlanner {
        hits: Arc<AtomicUsize>,
    }

    impl PlannerExtension for OtherClaimingPlanner {
        fn plan(&self, bound: &BoundStatement) -> Option<DBResult<SubPlan>> {
            if matches!(bound, BoundStatement::Other(_)) {
                self.hits.fetch_add(1, Ordering::SeqCst);
                return Some(Ok(SubPlan::new(None, None)));
            }
            None
        }
    }

    #[derive(Debug)]
    struct IdentityMapper {
        hits: Arc<AtomicUsize>,
    }

    impl MapperExtension for IdentityMapper {
        fn map(&self, plan: &ExecutionPlan) -> Option<DBResult<ExecutionPlan>> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            Some(Ok(plan.clone()))
        }
    }

    #[derive(Debug)]
    struct PrefixParser;

    impl ParserExtension for PrefixParser {
        fn transform(&self, query_text: &str) -> Option<DBResult<String>> {
            if query_text.starts_with("/* ext */") {
                return Some(Ok(query_text.trim_start_matches("/* ext */").to_string()));
            }
            None
        }
    }

    #[test]
    fn binder_extension_intercepts_and_rewrites() {
        let registry = ExtensionRegistry::new();
        let hits = Arc::new(AtomicUsize::new(0));
        registry.add_binder_extension(Arc::new(ShowInterceptor {
            hits: Arc::clone(&hits),
        }));

        let ast = parse_ast("SHOW SPACES");
        let rewritten = registry
            .try_bind(&ast, &BinderExtensionContext::default())
            .expect("interceptor claims SHOW");
        assert!(rewritten.is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 1);

        // Non-matching statements fall through to builtin (None).
        let other = parse_ast("MATCH (n) RETURN n");
        assert!(registry
            .try_bind(&other, &BinderExtensionContext::default())
            .is_none());
    }

    #[test]
    fn panicking_extensions_fall_back_to_builtin() {
        let registry = ExtensionRegistry::new();
        registry.add_binder_extension(Arc::new(PanickingBinder));
        registry.add_planner_extension(Arc::new(PanickingPlanner));

        let ast = parse_ast("MATCH (n) RETURN n");
        assert!(registry
            .try_bind(&ast, &BinderExtensionContext::default())
            .is_none());

        let bound = BoundStatement::Other(Box::new(ast.stmt().clone()));
        assert!(registry.try_plan(&bound).is_none());
    }

    #[test]
    fn planner_extension_intercepts_matching_bound_statement() {
        let registry = ExtensionRegistry::new();
        let hits = Arc::new(AtomicUsize::new(0));
        registry.add_planner_extension(Arc::new(OtherClaimingPlanner {
            hits: Arc::clone(&hits),
        }));

        let ast = parse_ast("SHOW SPACES");
        let bound = BoundStatement::Other(Box::new(ast.stmt().clone()));
        let planned = registry
            .try_plan(&bound)
            .expect("other-claiming planner handles Other");
        assert!(planned.is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn parser_and_mapper_stubs_register_and_invoke() {
        let registry = ExtensionRegistry::new();
        registry.add_parser_extension(Arc::new(PrefixParser));
        assert_eq!(registry.parser_extension_count(), 1);

        let rewritten = registry
            .try_parse_transform("/* ext */MATCH (n) RETURN n")
            .expect("prefix parser claims marked queries");
        assert_eq!(rewritten.unwrap(), "MATCH (n) RETURN n");
        assert!(registry.try_parse_transform("MATCH (n) RETURN n").is_none());

        // Mapper stub proves the registration/callable contract; an empty
        // optimized plan round-trips through the identity mapper.
        let hits = Arc::new(AtomicUsize::new(0));
        registry.add_mapper_extension(Arc::new(IdentityMapper {
            hits: Arc::clone(&hits),
        }));
        let plan = ExecutionPlan::new(None);
        let mapped = registry
            .try_map(&plan)
            .expect("identity mapper claims everything");
        assert!(mapped.is_ok());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }
}
