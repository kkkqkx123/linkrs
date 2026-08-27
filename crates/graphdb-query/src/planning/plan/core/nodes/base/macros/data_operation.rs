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
