//! Macros for defining logical plan nodes.
//!
//! These mirrors the physical node macros in `core::nodes::base::macros`
//! but generate structs whose children are `LogicalNodeEnum` instead of
//! `PlanNodeEnum`.  All fields are `pub` to allow cross-module construction
//! during physical-to-logical conversion.

#[macro_export]
macro_rules! define_logical_plan_node {
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
            pub id: i64,
            $(pub $field: $type,)*
            pub output_var: Option<String>,
            pub col_names: Vec<String>,
            pub column_types: Vec<$crate::core::DataType>,
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
            pub fn id(&self) -> i64 { self.id }
            pub fn type_name(&self) -> &'static str { stringify!($name) }
            pub fn output_var(&self) -> Option<&str> { self.output_var.as_deref() }
            pub fn col_names(&self) -> &[String] { &self.col_names }
            pub fn set_output_var(&mut self, var: String) { self.output_var = Some(var); }
            pub fn set_col_names(&mut self, names: Vec<String>) { self.col_names = names; }
            pub fn column_types(&self) -> &[$crate::core::DataType] { &self.column_types }
            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) { self.column_types = types; }

            pub fn clone_logical_node(&self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self.clone())
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalZeroInputNode for $name {}
    };

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
            pub id: i64,
            pub deps: Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            $(pub $field: $type,)*
            pub output_var: Option<String>,
            pub col_names: Vec<String>,
            pub column_types: Vec<$crate::core::DataType>,
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
            pub fn id(&self) -> i64 { self.id }
            pub fn type_name(&self) -> &'static str { stringify!($name) }
            pub fn output_var(&self) -> Option<&str> { self.output_var.as_deref() }
            pub fn col_names(&self) -> &[String] { &self.col_names }
            pub fn set_output_var(&mut self, var: String) { self.output_var = Some(var); }
            pub fn set_col_names(&mut self, names: Vec<String>) { self.col_names = names; }
            pub fn column_types(&self) -> &[$crate::core::DataType] { &self.column_types }
            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) { self.column_types = types; }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum] {
                &self.deps
            }

            pub fn add_dependency(&mut self, dep: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.deps.push(dep);
            }

            pub fn remove_dependency(&mut self, id: i64) -> bool {
                let initial_len = self.deps.len();
                self.deps.retain(|dep| dep.id() != id);
                self.deps.len() != initial_len
            }

            pub fn clone_logical_node(&self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self.clone())
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalMultipleInputNode for $name {
            fn inputs(&self) -> &[$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum] {
                &self.deps
            }

            fn inputs_mut(&mut self) -> &mut Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum> {
                &mut self.deps
            }

            fn add_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.deps.push(input);
            }

            fn remove_input(&mut self, index: usize) -> Result<(), String> {
                if index < self.deps.len() {
                    self.deps.remove(index);
                    Ok(())
                } else {
                    Err(format!("Index {} out of range", index))
                }
            }
        }
    };
}

#[macro_export]
macro_rules! define_logical_plan_node_with_deps {
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
            pub id: i64,
            pub input: Option<Box<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>>,
            pub deps: Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            $(pub $field: $type,)*
            pub output_var: Option<String>,
            pub col_names: Vec<String>,
            pub column_types: Vec<$crate::core::DataType>,
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
                    column_types: self.column_types.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 { self.id }
            pub fn type_name(&self) -> &'static str { stringify!($name) }
            pub fn output_var(&self) -> Option<&str> { self.output_var.as_deref() }
            pub fn col_names(&self) -> &[String] { &self.col_names }
            pub fn set_output_var(&mut self, var: String) { self.output_var = Some(var); }
            pub fn set_col_names(&mut self, names: Vec<String>) { self.col_names = names; }
            pub fn column_types(&self) -> &[$crate::core::DataType] { &self.column_types }
            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) { self.column_types = types; }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum] {
                &self.deps
            }

            pub fn dependencies_mut(&mut self) -> &mut Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum> {
                &mut self.deps
            }

            pub fn set_dependencies(&mut self, deps: Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>) {
                self.deps = deps;
            }

            pub fn clone_logical_node(&self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self.clone())
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalSingleInputNode for $name {
            fn input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                self.input.as_ref().expect("Input node does not exist")
            }

            fn input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                self.input.as_mut().expect("Input node does not exist")
            }

            fn set_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.input = Some(Box::new(input.clone()));
                self.deps.clear();
                self.deps.push(input);
            }
        }
    };
}

#[macro_export]
macro_rules! define_logical_join_node {
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
            pub id: i64,
            pub left: Box<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            pub right: Box<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            pub hash_keys: Vec<$crate::core::types::expr::contextual::ContextualExpression>,
            pub probe_keys: Vec<$crate::core::types::expr::contextual::ContextualExpression>,
            pub deps: Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            $(pub $field: $type,)*
            pub output_var: Option<String>,
            pub col_names: Vec<String>,
            pub column_types: Vec<$crate::core::DataType>,
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
                    column_types: self.column_types.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 { self.id }
            pub fn type_name(&self) -> &'static str { stringify!($name) }
            pub fn output_var(&self) -> Option<&str> { self.output_var.as_deref() }
            pub fn col_names(&self) -> &[String] { &self.col_names }
            pub fn set_output_var(&mut self, var: String) { self.output_var = Some(var); }
            pub fn set_col_names(&mut self, names: Vec<String>) { self.col_names = names; }
            pub fn column_types(&self) -> &[$crate::core::DataType] { &self.column_types }
            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) { self.column_types = types; }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum] {
                &self.deps
            }

            pub fn hash_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            pub fn probe_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }

            pub fn left_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            pub fn set_right_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }

            pub fn add_dependency(&mut self, _dep: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) -> Result<(), $crate::query::planning::planner::PlannerError> {
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

            pub fn clone_logical_node(&self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self.clone())
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalBinaryInputNode for $name {
            fn left_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            fn set_right_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalJoinNode for $name {
            fn hash_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.hash_keys
            }

            fn probe_keys(&self) -> &[$crate::core::types::expr::contextual::ContextualExpression] {
                &self.probe_keys
            }
        }
    };
}

#[macro_export]
macro_rules! define_logical_binary_input_node {
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
            pub id: i64,
            pub left: Box<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            pub right: Box<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            pub deps: Vec<$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum>,
            $(pub $field: $type,)*
            pub output_var: Option<String>,
            pub col_names: Vec<String>,
            pub column_types: Vec<$crate::core::DataType>,
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
                    column_types: self.column_types.clone(),
                }
            }
        }

        impl $name {
            pub fn id(&self) -> i64 { self.id }
            pub fn type_name(&self) -> &'static str { stringify!($name) }
            pub fn output_var(&self) -> Option<&str> { self.output_var.as_deref() }
            pub fn col_names(&self) -> &[String] { &self.col_names }
            pub fn set_output_var(&mut self, var: String) { self.output_var = Some(var); }
            pub fn set_col_names(&mut self, names: Vec<String>) { self.col_names = names; }
            pub fn column_types(&self) -> &[$crate::core::DataType] { &self.column_types }
            pub fn set_column_types(&mut self, types: Vec<$crate::core::DataType>) { self.column_types = types; }

            pub fn dependencies(&self) -> &[$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum] {
                &self.deps
            }

            pub fn left_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.left
            }

            pub fn right_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.right
            }

            pub fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.left
            }

            pub fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.right
            }

            pub fn set_left_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            pub fn set_right_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }

            pub fn clone_logical_node(&self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self.clone())
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalNode for $name {
            fn id(&self) -> i64 { self.id() }
            fn name(&self) -> &'static str { self.type_name() }
            fn output_var(&self) -> Option<&str> { self.output_var() }
            fn col_names(&self) -> &[String] { self.col_names() }
            fn set_output_var(&mut self, var: String) { self.set_output_var(var); }
            fn set_col_names(&mut self, names: Vec<String>) { self.set_col_names(names); }
            fn into_enum(self) -> $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                use $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum;
                LogicalNodeEnum::$enum_variant(self)
            }
        }

        impl $crate::query::planning::plan::logical::logical_node_traits::LogicalBinaryInputNode for $name {
            fn left_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.left
            }

            fn right_input(&self) -> &$crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &self.right
            }

            fn left_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.left
            }

            fn right_input_mut(&mut self) -> &mut $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum {
                &mut self.right
            }

            fn set_left_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.left = Box::new(input.clone());
                if self.deps.len() > 0 {
                    self.deps[0] = input.clone();
                }
            }

            fn set_right_input(&mut self, input: $crate::query::planning::plan::logical::logical_node_enum::LogicalNodeEnum) {
                self.right = Box::new(input.clone());
                if self.deps.len() > 1 {
                    self.deps[1] = input.clone();
                }
            }
        }
    };
}
