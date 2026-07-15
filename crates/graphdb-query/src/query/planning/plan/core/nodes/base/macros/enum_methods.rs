/// Macro for defining planning nodes
///
/// # Example
/// ```
/// define_plan_node! {
///     pub struct GetVerticesNode {
///         space_id: i32,
///         src_vids: String,
///         tag_props: Vec<TagProp>,
///     }
/// ```
///
/// Generate a macro for the `is_xxx` method for `PlanNodeEnum`
///
/// # Examples
/// ```
/// define_enum_is_methods! {
///     PlanNodeEnum,
///     (Start, is_start),
///     (Project, is_project),
///     (Filter, is_filter),
/// }
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
/// define_enum_as_methods! {
///     PlanNodeEnum,
///     (Start, as_start, StartNode),
///     (Project, as_project, ProjectNode),
///     (Filter, as_filter, FilterNode),
/// }
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
/// define_enum_as_mut_methods! {
///     PlanNodeEnum,
///     (Start, as_start_mut, StartNode),
///     (Project, as_project_mut, ProjectNode),
/// }
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

/// Generate a macro for the `type_name` method for `PlanNodeEnum`
///
/// # Examples
/// ```
/// define_enum_type_name! {
///     PlanNodeEnum,
///     (Start, "Start"),
///     (Project, "Project"),
/// }
/// ```
#[macro_export]
macro_rules! define_enum_type_name {
    ($enum_type:ident, $($(#[$meta:meta])* ($variant:ident, $name:expr)),* $(,)?) => {
        impl $enum_type {
            pub fn type_name(&self) -> &'static str {
                match self {
                    $($(#[$meta])* $enum_type::$variant(_) => $name,)*
                }
            }
        }
    };
}

/// Generate a macro for the `category` method of `PlanNodeEnum`
///
/// # Examples
/// ```
/// define_enum_category! {
///     PlanNodeEnum,
///     (Start, PlanNodeCategory::Access),
///     (Project, PlanNodeCategory::Operation),
/// }
/// ```
#[macro_export]
macro_rules! define_enum_category {
    ($enum_type:ident, $($(#[$meta:meta])* ($variant:ident, $category:expr)),* $(,)?) => {
        impl $enum_type {
            pub fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                use $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory;
                match self {
                    $($(#[$meta])* $enum_type::$variant(_) => $category,)*
                }
            }
        }
    };
}

/// Generate a macro for the describe method of PlanNodeEnum
///
/// # Examples
/// ```
/// define_enum_describe! {
///     PlanNodeEnum,
///     (Start, "Start"),
///     (Project, "Project"),
/// }
/// ```
#[macro_export]
macro_rules! define_enum_describe {
    ($enum_type:ident, $($(#[$meta:meta])* ($variant:ident, $name:expr)),* $(,)?) => {
        impl $enum_type {
            pub fn describe(&self) -> $crate::query::planning::plan::explain::PlanNodeDescription {
                use $crate::query::planning::plan::explain::PlanNodeDescription;
                match self {
                    $($(#[$meta])* $enum_type::$variant(node) => {
                        let mut desc = PlanNodeDescription::new($name, node.id());
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
