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
            left: Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            right: Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            hash_keys: Vec<graphdb_core::types::expr::contextual::ContextualExpression>,
            probe_keys: Vec<graphdb_core::types::expr::contextual::ContextualExpression>,
            deps: Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
            column_types: Vec<graphdb_core::DataType>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::planning::plan::core::node_id_generator::next_node_id;
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

            pub fn column_types(&self) -> &[graphdb_core::DataType] {
                &self.column_types
            }

            pub fn set_column_types(&mut self, types: Vec<graphdb_core::DataType>) {
                self.column_types = types;
            }

            pub fn dependencies(&self) -> &[$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn dependencies_mut(&mut self) -> &mut Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum> {
                &mut self.deps
            }

            pub fn hash_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            pub fn set_hash_keys(&mut self, keys: Vec<graphdb_core::types::expr::contextual::ContextualExpression>) {
                self.hash_keys = keys;
            }

            pub fn probe_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }

            pub fn set_probe_keys(&mut self, keys: Vec<graphdb_core::types::expr::contextual::ContextualExpression>) {
                self.probe_keys = keys;
            }

            pub fn left_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input;
                }
            }

            pub fn set_right_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input;
                }
            }

            pub fn add_dependency(&mut self, _dep: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) -> Result<(), $crate::planning::planner::PlannerError> {
                Err($crate::planning::planner::PlannerError::InvalidOperation(
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

            pub fn clone_plan_node(&self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn category(&self) -> $crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Join
            }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode for $name {
            fn left_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input;
                }
            }

            fn set_right_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input;
                }
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::JoinNode for $name {
            fn hash_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            fn probe_keys(&self) -> &[graphdb_core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let column_types_size = std::mem::size_of::<Vec<graphdb_core::DataType>>() * self.column_types.capacity();

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let left_right_size = std::mem::size_of::<Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>() * 2;

                let deps_size = std::mem::size_of::<Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + column_types_size + output_var_size + left_right_size + deps_size
            }
        }
    };
}

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
            left: Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            right: Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            deps: Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>,
            $($field: $type,)*
            output_var: Option<String>,
            col_names: Vec<String>,
            column_types: Vec<graphdb_core::DataType>,
        }

        impl Clone for $name {
            fn clone(&self) -> Self {
                use $crate::planning::plan::core::node_id_generator::next_node_id;
                Self {
                    id: next_node_id(),
                    left: self.left.clone(),
                    right: self.right.clone(),
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

            pub fn column_types(&self) -> &[graphdb_core::DataType] {
                &self.column_types
            }

            pub fn set_column_types(&mut self, types: Vec<graphdb_core::DataType>) {
                self.column_types = types;
            }

            pub fn dependencies(&self) -> &[$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum] {
                &self.deps
            }

            pub fn dependencies_mut(&mut self) -> &mut Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum> {
                &mut self.deps
            }

            pub fn left_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input;
                }
            }

            pub fn set_right_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input;
                }
            }

            pub fn clone_plan_node(&self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self.clone())
            }

            pub fn clone_with_new_id(&self, new_id: i64) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                let mut cloned = self.clone();
                cloned.id = new_id;
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(cloned)
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::PlanNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn category(&self) -> $crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory {
                $crate::planning::plan::core::nodes::base::plan_node_category::PlanNodeCategory::Traversal
            }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                use $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum;
                PlanNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::BinaryInputNode for $name {
            fn left_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input;
                }
            }

            fn set_right_input(&mut self, input: $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input;
                }
            }
        }

        impl $crate::planning::plan::core::nodes::base::plan_node_traits::PlanNodeClonable for $name {
            fn clone_plan_node(&self) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_plan_node()
            }
            fn clone_with_new_id(&self, new_id: i64) -> $crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum {
                self.clone_with_new_id(new_id)
            }
        }

        impl $crate::planning::plan::core::nodes::base::memory_estimation::MemoryEstimatable for $name {
            fn estimate_memory(&self) -> usize {
                let base = std::mem::size_of::<$name>();

                let col_names_size = $crate::planning::plan::core::nodes::base::memory_estimation::estimate_vec_string_memory(&self.col_names());

                let column_types_size = std::mem::size_of::<Vec<graphdb_core::DataType>>() * self.column_types.capacity();

                let output_var_size = std::mem::size_of::<Option<String>>() +
                    self.output_var.as_ref()
                        .map(|s| std::mem::size_of::<String>() + s.capacity())
                        .unwrap_or(0);

                let left_right_size = std::mem::size_of::<Box<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>() * 2;

                let deps_size = std::mem::size_of::<Vec<$crate::planning::plan::core::nodes::base::plan_node_enum::PlanNodeEnum>>();

                base + col_names_size + column_types_size + output_var_size + left_right_size + deps_size
            }
        }
    };
}
