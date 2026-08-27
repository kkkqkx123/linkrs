//! System information query nodes.
//!
//! Plan nodes for SHOW CONFIGS / SHOW QUERIES / SHOW SESSIONS statements.

use crate::define_plan_node;

define_plan_node! {
    pub struct ShowConfigsNode {
        module: Option<String>,
    }
    enum: ShowConfigs
    input: ZeroInputNode
}

impl ShowConfigsNode {
    pub fn new(id: i64, module: Option<String>) -> Self {
        Self {
            id,
            module,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn module(&self) -> Option<&str> {
        self.module.as_deref()
    }
}

define_plan_node! {
    pub struct ShowQueriesNode {
    }
    enum: ShowQueries
    input: ZeroInputNode
}

impl ShowQueriesNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }
}

define_plan_node! {
    pub struct ShowSessionsNode {
    }
    enum: ShowSessions
    input: ZeroInputNode
}

impl ShowSessionsNode {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }
}
