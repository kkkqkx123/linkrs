/// Macro for defining plan nodes (ZeroInputNode and MultipleInputNode variants)
///
/// # Example
/// ```ignore
/// use graphdb_query::define_plan_node;
///
/// struct TagProp;
///
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
            column_types: Vec<$crate::core::DataType>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                    column_types: self.column_types.clone(),
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

            pub fn column_types(&self) -> &[$crate::core::DataType] {
                &self.column_types
            }

            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) {
                self.column_types = types;
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

                let column_types_size = std::mem::size_of::<Vec<$crate::core::DataType>>() * self.column_types.capacity();

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                base + col_names_size + column_types_size + output_var_size
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
            column_types: Vec<$crate::core::DataType>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::query::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    $($field: self.$field.clone(),)*
                    output_var: self.output_var.clone(),
                    col_names: self.col_names.clone(),
                    column_types: self.column_types.clone(),
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

            pub fn column_types(&self) -> &[$crate::core::DataType] {
                &self.column_types
            }

            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) {
                self.column_types = types;
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

                let column_types_size = std::mem::size_of::<Vec<$crate::core::DataType>>() * self.column_types.capacity();

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                base + col_names_size + column_types_size + output_var_size
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
            column_types: Vec<$crate::core::DataType>,
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
                    column_types: self.column_types.clone(),
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

            pub fn column_types(&self) -> &[$crate::core::DataType] {
                &self.column_types
            }

            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) {
                self.column_types = types;
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

                let column_types_size = std::mem::size_of::<Vec<$crate::core::DataType>>() * self.column_types.capacity();

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let input_size = std::mem::size_of::<Option<Box<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>>();

                let deps_size = std::mem::size_of::<Vec<$crate::query::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + column_types_size + output_var_size + input_size + deps_size
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
