/// Generate a macro for the `is_xxx` method for `PlanNodeEnum`
///
/// # Examples
/// ```
/// use graphdb_query::define_enum_is_methods;
///
/// enum MyPlanEnum {
///     Start(i32),
///     Project(i32),
/// }
///
/// define_enum_is_methods! {
///     MyPlanEnum,
///     (Start, is_start),
///     (Project, is_project),
/// }
///
/// let node = MyPlanEnum::Start(42);
/// assert!(node.is_start());
/// assert!(!node.is_project());
/// ```
#[macro_export]
macro_rules! define_enum_is_methods {
    ($enum_type:ident, $(($variant:ident, $method:ident)),* $(,)?) => {
        impl $enum_type {
            $(
                pub fn $method(&self) -> bool {
                    matches!(self, $enum_type::$variant(_))
                }
            )*
        }
    };
}

/// Generate a macro for the `as_xxx` method for `PlanNodeEnum`
///
/// # Examples
/// ```
/// use graphdb_query::define_enum_as_methods;
///
/// enum MyPlanEnum {
///     Start(i32),
///     Project(i32),
/// }
///
/// define_enum_as_methods! {
///     MyPlanEnum,
///     (Start, as_start, i32),
///     (Project, as_project, i32),
/// }
///
/// let node = MyPlanEnum::Start(42);
/// assert_eq!(node.as_start(), Some(&42));
/// assert!(node.as_project().is_none());
/// ```
#[macro_export]
macro_rules! define_enum_as_methods {
    ($enum_type:ident, $(($variant:ident, $method:ident, $node_type:ty)),* $(,)?) => {
        impl $enum_type {
            $(
                pub fn $method(&self) -> Option<&$node_type> {
                    match self {
                        $enum_type::$variant(node) => Some(node),
                        _ => None,
                    }
                }
            )*
        }
    };
}

/// Generate a macro for the `as_xxx_mut` method for `PlanNodeEnum`
///
/// # Examples
/// ```
/// use graphdb_query::define_enum_as_mut_methods;
///
/// enum MyPlanEnum {
///     Start(i32),
///     Project(i32),
/// }
///
/// define_enum_as_mut_methods! {
///     MyPlanEnum,
///     (Start, as_start_mut, i32),
///     (Project, as_project_mut, i32),
/// }
///
/// let mut node = MyPlanEnum::Start(42);
/// if let Some(value) = node.as_start_mut() {
///     *value += 1;
/// }
/// assert!(node.as_start_mut().is_some());
/// assert!(node.as_project_mut().is_none());
/// ```
#[macro_export]
macro_rules! define_enum_as_mut_methods {
    ($enum_type:ident, $(($variant:ident, $method:ident, $node_type:ty)),* $(,)?) => {
        impl $enum_type {
            $(
                pub fn $method(&mut self) -> Option<&mut $node_type> {
                    match self {
                        $enum_type::$variant(node) => Some(node),
                        _ => None,
                    }
                }
            )*
        }
    };
}

/// Generate metadata methods for `PlanNodeEnum` from a single macro table.
///
/// This is the unified, table-driven replacement for the former
/// `define_enum_type_name!` / `define_enum_category!` / `define_enum_describe!`
/// macros. It emits four metadata surfaces from one exhaustive table, so they
/// can never drift from each other or from the enum:
///
/// - `type_name()` — derived from the variant identifier itself via
///   `stringify!`, so it can never drift from the enum.
/// - `ALL_VARIANT_NAMES` — a const slice of every variant name in declaration
///   order (honoring per-item `#[cfg(...)]` gates). This is the single source
///   of truth for "how many plan nodes exist".
/// - `category()` — maps each variant to its `PlanNodeCategory`.
/// - `describe()` — produces the EXPLAIN description. Its label may differ from
///   `type_name()` when the human-readable name is more specific (e.g. the
///   `Window` variant describes as `"Window function"`).
///
/// `is_xxx`/`as_xxx`/`as_xxx_mut` stay separate invocations to keep IDE
/// navigation one hop away from the variant table.
///
/// # Examples
/// ```
/// use graphdb_query::define_all_plan_nodes;
/// use graphdb_query::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
///
/// // The node types only need `id()` and `output_var()` for `describe()`.
/// struct StartNode;
/// struct ProjectNode;
///
/// impl StartNode {
///     fn id(&self) -> i64 {
///         1
///     }
///     fn output_var(&self) -> Option<&str> {
///         None
///     }
/// }
///
/// impl ProjectNode {
///     fn id(&self) -> i64 {
///         2
///     }
///     fn output_var(&self) -> Option<&str> {
///         None
///     }
/// }
///
/// enum MyPlanEnum {
///     Start(StartNode),
///     Project(ProjectNode),
/// }
///
/// define_all_plan_nodes! {
///     MyPlanEnum,
///     (Start, StartNode, PlanNodeCategory::Access, "Start"),
///     (Project, ProjectNode, PlanNodeCategory::Operation, "Project"),
/// }
///
/// let node = MyPlanEnum::Start(StartNode);
/// assert_eq!(node.type_name(), "Start");
/// assert_eq!(node.category(), PlanNodeCategory::Access);
/// assert_eq!(MyPlanEnum::ALL_VARIANT_NAMES.len(), 2);
/// ```
#[macro_export]
macro_rules! define_all_plan_nodes {
    ($enum_type:ident, $($(#[$meta:meta])* ($variant:ident, $node_type:ty, $category:expr, $describe:expr)),* $(,)?) => {
        impl $enum_type {
            /// Static plan-node type name, identical to the enum variant name.
            pub fn type_name(&self) -> &'static str {
                match self {
                    $($(#[$meta])* $enum_type::$variant(_) => stringify!($variant),)*
                }
            }

            /// All variant names, in declaration order, honoring feature gates.
            ///
            /// Generated from the same exhaustive macro table as `type_name()`,
            /// so it always matches the enum exactly (the compiler enforces the
            /// match exhaustiveness; this slice cannot drift).
            pub const ALL_VARIANT_NAMES: &'static [&'static str] = &[
                $($(#[$meta])* stringify!($variant),)*
            ];

            /// The `PlanNodeCategory` this variant belongs to.
            pub fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                use $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
                match self {
                    $($(#[$meta])* $enum_type::$variant(_) => $category,)*
                }
            }

            /// Build the EXPLAIN description for this node.
            pub fn describe(&self) -> $crate::query::planning::plan::explain::PlanNodeDescription {
                use $crate::query::planning::plan::explain::PlanNodeDescription;
                match self {
                    $($(#[$meta])* $enum_type::$variant(node) => {
                        let mut desc = PlanNodeDescription::new($describe, node.id());
                        if let Some(var) = node.output_var() {
                            desc = desc.with_output_var(var.to_string());
                        }
                        desc
                    })*
                }
            }
        }
    };
}
