//! Plan node macro definition
//!
//! Provide macros to simplify the definition of plan nodes and reduce the amount of样板 code.
//!
//! # Refactoring changes
//! Remove the dependency on `ast::Variable` and use `String` instead.

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
/// 假设所有节点都有 id() 和 output_var() 方法
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

/// Macro for defining plan nodes (ZeroInputNode and MultipleInputNode variants)
///
/// # Example
/// ```
/// define_plan_node! {
///     pub struct GetVerticesNode {
///         space_id: i32,
///         src_vids: String,
///         tag_props: Vec<TagProp>,
///     }
///     enum: GetVertices
///     input: ZeroInputNode
/// }
/// ```
#[macro_export]
macro_rules! define_plan_node {
    // The ZeroInputNode branch
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        enum: $enum_variant:ident
        input: ZeroInputNode
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 {
                self.id()
            }

            fn name(&self) -> &'static str {
                self.type_name()
            }

            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Access
            }

            fn output_var(&self) -> Option<&str> {
                self.output_var()
            }

            fn col_names(&self) -> &[String] {
                self.col_names()
            }

            fn set_output_var(&mut self, var: String) {
                self.set_output_var(var);
            }

            fn set_col_names(&mut self, names: Vec<String>) {
                self.set_col_names(names);
            }

            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }

            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::ZeroInputNode for $name {}

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                base + col_names_size + output_var_size
            }
        }
    };

    // The Management Node branch (ZeroInputNode with parameterized enum)
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        manage_enum: $category:ident :: $variant:ident as $enum_variant:ident
        input: ZeroInputNode
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::management::manage_node_enums::$category;
                PlanNodeEnum::$enum_variant($category::$variant(self.clone()))
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::management::manage_node_enums::$category;
                PlanNodeEnum::$enum_variant($category::$variant(cloned))
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 {
                self.id()
            }

            fn name(&self) -> &'static str {
                self.type_name()
            }

            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Access
            }

            fn output_var(&self) -> Option<&str> {
                self.output_var()
            }

            fn col_names(&self) -> &[String] {
                self.col_names()
            }

            fn set_output_var(&mut self, var: String) {
                self.set_output_var(var);
            }

            fn set_col_names(&mut self, names: Vec<String>) {
                self.set_col_names(names);
            }

            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                use $crate::query::planning::plan::core::nodes::management::manage_node_enums::$category;
                PlanNodeEnum::$enum_variant($category::$variant(self))
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }

            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::ZeroInputNode for $name {}

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();
                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                base + col_names_size + output_var_size
            }
        }
    };

    // The MultipleInputNode branch
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        enum: $enum_variant:ident
        input: MultipleInputNode
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            deps: Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    deps: self.deps.clone(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn add_dependency(&mut self, dep: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.deps.push(dep);
            }

            pub fn remove_dependency(&mut self, id: i64) -> bool {
                let initial_len = self.deps.len();
                self.deps.retain(|dep| dep.id() != id);
                self.deps.len() != initial_len
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 {
                self.id()
            }

            fn name(&self) -> &'static str {
                self.type_name()
            }

            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Access
            }

            fn output_var(&self) -> Option<&str> {
                self.output_var()
            }

            fn col_names(&self) -> &[String] {
                self.col_names()
            }

            fn set_output_var(&mut self, var: String) {
                self.set_output_var(var);
            }

            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::MultipleInputNode for $name {
            fn inputs(&self) -> &[$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            fn inputs_mut(&mut self) -> &mut Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum> {
                &mut self.deps
            }

            fn add_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.deps.push(input);
            }

            fn remove_input(&mut self, index: usize) -> Result<(), String> {
                if index < self.deps.len() {
                    self.deps.remove(index);
                    Ok(())
                } else {
                    Err(format!("Index {} Out of range", index))
                }
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let input_size = std::mem::size_of::<Option<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>>();

                let deps_size = std::mem::size_of::<Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + output_var_size + input_size + deps_size
            }
        }
    };
}

// ============================================================================
// Join Plan Node Macros
// ============================================================================

/// Define the macro for connecting nodes
#[macro_export]
macro_rules! define_join_node {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        enum: $enum_variant:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            left: Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            right: Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            hash_keys: Vec<$crate::core::types::expr::contextual::ContextualExpression>,
            probe_keys: Vec<$crate::core::types::expr::contextual::ContextualExpression>,
            deps: Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    left: self.left.clone(),
                    right: self.right.clone(),
                    hash_keys: self.hash_keys.clone(),
                    probe_keys: self.probe_keys.clone(),
                    deps: self.deps.clone(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn hash_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            pub fn probe_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }

            pub fn left_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            pub fn set_right_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }

            pub fn add_dependency(&mut self, _dep: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) -> Result<(), $crate::query::planning::planner::PlannerError> {
                Err($crate::query::planning::planner::PlannerError::InvalidOperation(
                    format!("The {} node does not support adding dependencies, it requires exactly two inputs", stringify!($name))
                ))
            }

            pub fn remove_dependency(&mut self, id: i64) -> bool {
                let initial_len = self.deps.len();
                self.deps.retain(|dep| dep.id() != id);
                let final_len = self.deps.len();

                if initial_len != final_len {
                    if self.left.id() == id {
                        if let Some(new_left) = self.deps.get(0) {
                            self.left = Box::new(new_left.clone());
                        }
                    }
                    if self.right.id() == id {
                        if let Some(new_right) = self.deps.get(1) {
                            self.right = Box::new(new_right.clone());
                        }
                    }
                    true
                } else {
                    false
                }
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Join
            }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode for $name {
            fn left_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            fn set_right_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::JoinNode for $name {
            fn hash_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            fn probe_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let left_right_size = std::mem::size_of::<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>() * 2;

                let deps_size = std::mem::size_of::<Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + output_var_size + left_right_size + deps_size
            }
        }
    };
}

// ============================================================================
// Operation Plan Node Macros
// ============================================================================

/// Define a macro for planning nodes with dependencies
#[macro_export]
macro_rules! define_plan_node_with_deps {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        enum: $enum_variant:ident
        input: SingleInputNode
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            input: Option<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>,
            deps: Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    input: self.input.clone(),
                    deps: self.deps.clone(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn dependencies_mut(&mut self) -> &mut Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum> {
                &mut self.deps
            }

            pub fn set_dependencies(&mut self, deps: Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>) {
                self.deps = deps;
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Operation
            }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::SingleInputNode for $name {
            fn input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.input.as_ref().expect("Input node does not exist")
            }

            fn input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.input.as_mut().expect("Input node does not exist")
            }

            fn set_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.input = Some(Box::new(input.clone()));
                self.deps.clear();
                self.deps.push(input);
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let input_size = std::mem::size_of::<Option<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>>();

                let deps_size = std::mem::size_of::<Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + output_var_size + input_size + deps_size
            }
        }
    };
}

// ============================================================================
// Traversal Plan Node Macros
// ============================================================================

/// Define the macro for the dual-input plan node
#[macro_export]
macro_rules! define_binary_input_node {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            $($field:ident: $type:ty),* $(,)?
        }
        enum: $enum_variant:ident
        input: BinaryInputNode
    ) => {
        $(#[$meta])*
        #[derive(Debug)]
        pub struct $name {
            id: i64,
            left: Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            right: Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            deps: Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    left: self.left.clone(),
                    right: self.right.clone(),
                    deps: self.deps.clone(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 {
                self.id
            }

            pub fn type_name(&self) -> &'static str {
                stringify!($name)
            }

            pub fn output_var(&self) -> Option<&str> {
                self.output_var.as_deref()
            }

            pub fn col_names(&self) -> &[String] {
                &self.col_names
            }

            pub fn set_output_var(&mut self, var: String) {
                self.output_var = Some(var);
            }

            pub fn set_col_names(&mut self, names: Vec<String>) {
                self.col_names = names;
            }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn left_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            pub fn set_right_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }

            pub fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn category(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::query::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Traversal
            }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode for $name {
            fn left_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            fn set_right_input(&mut self, input: $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::query::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::query::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let left_right_size = std::mem::size_of::<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>() * 2;

                let deps_size = std::mem::size_of::<Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + output_var_size + left_right_size + deps_size
            }
        }
    };
}

// ============================================================================
// Data Modification Plan Node Macros
// ============================================================================

/// Macro to define a data operation info struct with common fields
#[macro_export]
macro_rules! define_data_op_info {
    (
        $(#[$meta:meta])*
        pub struct $name:ident {
            space_name: String,
            $($field:ident: $ftype:ty),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone)]
        pub struct $name {
            pub space_name: String,
            $(pub $field: $ftype,)*
        }
    };
}

/// Macro to define a vertices operation node with common methods
#[macro_export]
macro_rules! define_vertices_node {
    (
        $(#[$meta:meta])*
        pub struct $node_name:ident {
            info: $info_type:ty,
        }
        enum: $enum_variant:ident
    ) => {
        $crate::define_plan_node! {
            $(#[$meta])*
            pub struct $node_name {
                info: $info_type,
            }
            enum: $enum_variant
            input: ZeroInputNode
        }

        impl $node_name {
            pub fn new(id: i64, info: $info_type) -> Self {
                Self {
                    id,
                    info,
                    output_var: None,
                    col_names: vec![stringify!($enum_variant).to_lowercase()],
                }
            }

            pub fn info(&self) -> &$info_type {
                &self.info
            }

            pub fn space_name(&self) -> &str {
                &self.info.space_name
            }
        }
    };
}

/// Macro to define an edges operation node with common methods
#[macro_export]
macro_rules! define_edges_node {
    (
        $(#[$meta:meta])*
        pub struct $node_name:ident {
            info: $info_type:ty,
        }
        enum: $enum_variant:ident
    ) => {
        $crate::define_plan_node! {
            $(#[$meta])*
            pub struct $node_name {
                info: $info_type,
            }
            enum: $enum_variant
            input: ZeroInputNode
        }

        impl $node_name {
            pub fn new(id: i64, info: $info_type) -> Self {
                Self {
                    id,
                    info,
                    output_var: None,
                    col_names: vec![stringify!($enum_variant).to_lowercase()],
                }
            }

            pub fn info(&self) -> &$info_type {
                &self.info
            }

            pub fn space_name(&self) -> &str {
                &self.info.space_name
            }
        }
    };
}

/// Macro to define a single-target operation node (like UpdateNode)
#[macro_export]
macro_rules! define_single_op_node {
    (
        $(#[$meta:meta])*
        pub struct $node_name:ident {
            info: $info_type:ty,
        }
        enum: $enum_variant:ident
    ) => {
        $crate::define_plan_node! {
            $(#[$meta])*
            pub struct $node_name {
                info: $info_type,
            }
            enum: $enum_variant
            input: ZeroInputNode
        }

        impl $node_name {
            pub fn new(id: i64, info: $info_type) -> Self {
                Self {
                    id,
                    info,
                    output_var: None,
                    col_names: vec![stringify!($enum_variant).to_lowercase()],
                }
            }

            pub fn info(&self) -> &$info_type {
                &self.info
            }
        }
    };
}
