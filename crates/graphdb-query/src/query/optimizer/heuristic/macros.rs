//! Rewrite the macro definitions for the rules
//!
//! Provide declarative macros to simplify the definition of rewriting rules and reduce the amount of样板 code.

// ==================== Basic Rules Macros ====================

/// Define the basic rules for rule rewriting.
///
/// 自动生成规则结构体、Default实现、new()方法和RewriteRule trait实现
///
/// # Example
/// ```rust
/// define_rewrite_rule! {
///     name: MyCustomRule,
///     pattern: Pattern::new_with_name("Filter"),
///     apply: |ctx, node| {
// Rule logic
///         Ok(None)
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_rewrite_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        pattern: $pattern:expr,
        apply: $apply_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Create a rule instance
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $pattern
            }

            fn apply(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                let apply_fn: fn(&mut _, &_) -> _ = $apply_closure;
                apply_fn(ctx, node)
            }
        }
    };
}

/// Define rules that match node types.
///
/// Automatic processing of node type matching and unpacking
///
/// # Examples
/// ```rust
/// define_typed_rewrite_rule! {
///     name: EliminateFilterRule,
///     pattern: Pattern::new_with_name("Filter"),
///     node_type: Filter,
///     apply: |ctx, filter_node| {
// The `filter_node` variable already has the type of the `FilterNode` class after unpacking.
///         Ok(None)
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_typed_rewrite_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        pattern: $pattern:expr,
        node_type: $node_type:ident,
        apply: $apply_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Creating rule instances
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $pattern
            }

            fn apply(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

                let typed_node = match node {
                    PlanNodeEnum::$node_type(n) => n,
                    _ => return Ok(None),
                };

                let apply_fn: fn(&mut _, &_) -> _ = $apply_closure;
                apply_fn(ctx, typed_node)
            }
        }
    };
}

// ==================== Pushdown Rule Macros ====================

/// Define the push-down rule.
///
/// Automatically generate implementations for the RewriteRule and PushDownRule traits.
///
/// # Examples
/// ```rust
/// define_rewrite_pushdown_rule! {
///     name: PushFilterDownGetNbrsRule,
///     parent_node: Filter,
///     child_node: GetNeighbors,
///     apply: |ctx, filter_node, get_neighbors_node| {
// Push-down logic
///         Ok(None)
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_rewrite_pushdown_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        parent_node: $parent_type:ident,
        child_node: $child_type:ident,
        apply: $apply_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Creating rule instances
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $crate::query::optimizer::heuristic::pattern::Pattern::new_with_name(stringify!($parent_type))
                    .with_dependency_name(stringify!($child_type))
            }

            fn apply(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

                let parent_node = match node {
                    PlanNodeEnum::$parent_type(n) => n,
                    _ => return Ok(None),
                };

                let input = parent_node.input();
                let child_node = match input {
                    PlanNodeEnum::$child_type(n) => n,
                    _ => return Ok(None),
                };

                let apply_fn: fn(&mut _, &_, &_) -> _ = $apply_closure;
                apply_fn(ctx, parent_node, child_node)
            }
        }

        impl $crate::query::optimizer::heuristic::rule::PushDownRule for $name {
            fn can_push_down(
                &self,
                node: &$crate::query::planning::plan::PlanNodeEnum,
                target: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> bool {
                use $crate::query::planning::plan::PlanNodeEnum;
                matches!(
                    (node, target),
                    (PlanNodeEnum::$parent_type(_), PlanNodeEnum::$child_type(_))
                )
            }

            fn push_down(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
                _target: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::optimizer::heuristic::rule::RewriteRule;
                self.apply(ctx, node)
            }
        }
    };
}

// ==================== Remove Rule Macros ====================

/// Define the elimination rules.
///
/// Automatically generate implementations of the RewriteRule and EliminationRule traits.
///
/// # Examples
/// ```rust
/// define_rewrite_elimination_rule! {
///     name: EliminateFilterRule,
///     node_type: Filter,
///     can_eliminate: |filter_node| {
///         is_expression_tautology(filter_node.condition())
///     },
///     eliminate: |ctx, filter_node| {
///         let mut result = TransformResult::new();
///         result.erase_curr = true;
///         result.add_new_node(filter_node.input().clone());
///         Ok(Some(result))
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_rewrite_elimination_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        node_type: $node_type:ident,
        can_eliminate: $can_eliminate_closure:expr,
        eliminate: $eliminate_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Creating rule instances
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $crate::query::optimizer::heuristic::pattern::Pattern::new_with_name(stringify!($node_type))
            }

            fn apply(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;

                let typed_node = match node {
                    PlanNodeEnum::$node_type(n) => n,
                    _ => return Ok(None),
                };

                let can_eliminate_fn: fn(&_) -> _ = $can_eliminate_closure;
                if !can_eliminate_fn(typed_node) {
                    return Ok(None);
                }

                let eliminate_fn: fn(&mut _, &_) -> _ = $eliminate_closure;
                eliminate_fn(ctx, typed_node)
            }
        }

        impl $crate::query::optimizer::heuristic::rule::EliminationRule for $name {
            fn can_eliminate(&self, node: &$crate::query::planning::plan::PlanNodeEnum) -> bool {
                use $crate::query::planning::plan::PlanNodeEnum;

                let typed_node = match node {
                    PlanNodeEnum::$node_type(n) => n,
                    _ => return false,
                };

                let can_eliminate_fn: fn(&_) -> _ = $can_eliminate_closure;
                can_eliminate_fn(typed_node)
            }

            fn eliminate(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                self.apply(ctx, node)
            }
        }
    };
}

/// Define a simple elimination rule (which only deletes the current node).
///
/// # Examples
/// ```rust
/// define_simple_rewrite_elimination_rule! {
///     name: EliminateTrueFilterRule,
///     node_type: Filter,
///     condition: |filter_node: &FilterNode| is_expression_tautology(filter_node.condition())
/// }
/// ```
#[macro_export]
macro_rules! define_simple_rewrite_elimination_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        node_type: $node_type:ident,
        condition: $condition_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Creating rule instances
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $crate::query::optimizer::heuristic::pattern::Pattern::new_with_name(stringify!($node_type))
            }

            fn apply(
                &self,
                _ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

                let typed_node = match node {
                    PlanNodeEnum::$node_type(n) => n,
                    _ => return Ok(None),
                };

                let condition_fn: fn(&_) -> _ = $condition_closure;
                if !condition_fn(typed_node) {
                    return Ok(None);
                }

                let mut result = $crate::query::optimizer::heuristic::result::TransformResult::new();
                result.erase_curr = true;
                result.add_new_node(typed_node.input().clone());
                Ok(Some(result))
            }
        }

        impl $crate::query::optimizer::heuristic::rule::EliminationRule for $name {
            fn can_eliminate(&self, node: &$crate::query::planning::plan::PlanNodeEnum) -> bool {
                use $crate::query::planning::plan::PlanNodeEnum;

                let typed_node = match node {
                    PlanNodeEnum::$node_type(n) => n,
                    _ => return false,
                };

                let condition_fn: fn(&_) -> _ = $condition_closure;
                condition_fn(typed_node)
            }

            fn eliminate(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::optimizer::heuristic::rule::RewriteRule;
                self.apply(ctx, node)
            }
        }
    };
}

// ==================== Merging Rules Macro ====================

/// Define merge rules
///
/// Automatically generate implementations for the RewriteRule and MergeRule traits.
///
/// # Examples
/// ```rust
/// define_rewrite_merge_rule! {
///     name: CombineFilterRule,
///     parent_node: Filter,
///     child_node: Filter,
///     can_merge: |parent, child| true,
///     merge: |ctx, parent, child| {
// Merge the logic
///         Ok(None)
///     }
/// }
/// ```
#[macro_export]
macro_rules! define_rewrite_merge_rule {
    (
        $(#[$meta:meta])*
        name: $name:ident,
        parent_node: $parent_type:ident,
        child_node: $child_type:ident,
        can_merge: $can_merge_closure:expr,
        merge: $merge_closure:expr
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name;

        impl $name {
            /// Creating rule instances
            pub fn new() -> Self {
                Self
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl $crate::query::optimizer::heuristic::rule::RewriteRule for $name {
            fn name(&self) -> &'static str {
                stringify!($name)
            }

            fn pattern(&self) -> $crate::query::optimizer::heuristic::pattern::Pattern {
                $crate::query::optimizer::heuristic::pattern::Pattern::new_with_name(stringify!($parent_type))
                    .with_dependency_name(stringify!($child_type))
            }

            fn apply(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                node: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode;

                let parent_node = match node {
                    PlanNodeEnum::$parent_type(n) => n,
                    _ => return Ok(None),
                };

                let input = parent_node.input();
                let child_node = match input {
                    PlanNodeEnum::$child_type(n) => n,
                    _ => return Ok(None),
                };

                let can_merge_fn: fn(&_, &_) -> _ = $can_merge_closure;
                if !can_merge_fn(parent_node, child_node) {
                    return Ok(None);
                }

                let merge_fn: fn(&mut _, &_, &_) -> _ = $merge_closure;
                merge_fn(ctx, parent_node, child_node)
            }
        }

        impl $crate::query::optimizer::heuristic::rule::MergeRule for $name {
            fn can_merge(
                &self,
                parent: &$crate::query::planning::plan::PlanNodeEnum,
                child: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> bool {
                use $crate::query::planning::plan::PlanNodeEnum;

                let parent_node = match parent {
                    PlanNodeEnum::$parent_type(n) => n,
                    _ => return false,
                };

                let child_node = match child {
                    PlanNodeEnum::$child_type(n) => n,
                    _ => return false,
                };

                let can_merge_fn: fn(&_, &_) -> _ = $can_merge_closure;
                can_merge_fn(parent_node, child_node)
            }

            fn create_merged_node(
                &self,
                ctx: &mut $crate::query::optimizer::heuristic::context::RewriteContext,
                parent: &$crate::query::planning::plan::PlanNodeEnum,
                child: &$crate::query::planning::plan::PlanNodeEnum,
            ) -> $crate::query::optimizer::heuristic::result::RewriteResult<Option<$crate::query::optimizer::heuristic::result::TransformResult>> {
                use $crate::query::planning::plan::PlanNodeEnum;

                let parent_node = match parent {
                    PlanNodeEnum::$parent_type(n) => n,
                    _ => return Ok(None),
                };

                let child_node = match child {
                    PlanNodeEnum::$child_type(n) => n,
                    _ => return Ok(None),
                };

                let merge_fn: fn(&mut _, &_, &_) -> _ = $merge_closure;
                merge_fn(ctx, parent_node, child_node)
            }
        }
    };
}

// ==================== Rule Registration Macro ====================

/// Definition of rules: Registry
///
/// Automatically generate the default implementation of RuleRegistry, which includes the registration of all rules.
///
/// # Examples
/// ```rust
/// define_rewrite_rule_registry! {
///     elimination: [
///         EliminateFilter,
///         RemoveNoopProject,
///     ],
///     merge: [
///         CombineFilter,
///         CollapseProject,
///     ],
/// }
/// ```
#[macro_export]
macro_rules! define_rewrite_rule_registry {
    (
        $(
            $category:ident: [
                $($rule_name:ident),* $(,)?
            ]
        ),* $(,)?
    ) => {
        impl Default for RuleRegistry {
            fn default() -> Self {
                let mut registry = Self::new();
                $(
                    $(
                        registry.add(RewriteRule::$rule_name(
                            $crate::query::optimizer::heuristic::$category::paste! {[<$rule_name Rule>]}::new()
                        ));
                    )*
                )*
                registry
            }
        }
    };
}

// Re-export all macros
pub use crate::define_rewrite_elimination_rule;
pub use crate::define_rewrite_merge_rule;
pub use crate::define_rewrite_pushdown_rule;
pub use crate::define_rewrite_rule;
pub use crate::define_rewrite_rule_registry;
pub use crate::define_simple_rewrite_elimination_rule;
pub use crate::define_typed_rewrite_rule;
