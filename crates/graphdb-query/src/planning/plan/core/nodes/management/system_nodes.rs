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

define_plan_node! {
    pub struct ShowFunctionsNode {
    }
    enum: ShowFunctions
    input: ZeroInputNode
}

impl ShowFunctionsNode {
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
    pub struct ShowGraphsNode {
    }
    enum: ShowGraphs
    input: ZeroInputNode
}

impl ShowGraphsNode {
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
    pub struct ShowMacrosNode {
    }
    enum: ShowMacros
    input: ZeroInputNode
}

impl ShowMacrosNode {
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
    pub struct LoadFromNode {
        source_kind: String,
        source_value: String,
        func_name: Option<String>,
        func_args_json: Option<String>,
        options: Vec<(String, String)>,
    }
    enum: LoadFrom
    input: ZeroInputNode
}

impl LoadFromNode {
    pub fn new(
        id: i64,
        source_kind: String,
        source_value: String,
        func_name: Option<String>,
        func_args_json: Option<String>,
        options: Vec<(String, String)>,
        col_names: Vec<String>,
    ) -> Self {
        Self {
            id,
            source_kind,
            source_value,
            func_name,
            func_args_json,
            options,
            output_var: None,
            col_names,
            column_types: vec![],
        }
    }

    pub fn source_kind(&self) -> &str {
        &self.source_kind
    }

    pub fn source_value(&self) -> &str {
        &self.source_value
    }

    pub fn func_name(&self) -> Option<&str> {
        self.func_name.as_deref()
    }

    pub fn func_args_json(&self) -> Option<&str> {
        self.func_args_json.as_deref()
    }

    pub fn options(&self) -> &[(String, String)] {
        &self.options
    }
}

define_plan_node! {
    pub struct InQueryCallNode {
        func_name: String,
        args_json: String,
        yield_items: Vec<(String, String)>,
    }
    enum: InQueryCall
    input: ZeroInputNode
}

impl InQueryCallNode {
    pub fn new(
        id: i64,
        func_name: String,
        args_json: String,
        yield_items: Vec<(String, String)>,
        col_names: Vec<String>,
    ) -> Self {
        Self {
            id,
            func_name,
            args_json,
            yield_items,
            output_var: None,
            col_names,
            column_types: vec![],
        }
    }

    pub fn func_name(&self) -> &str {
        &self.func_name
    }

    pub fn args_json(&self) -> &str {
        &self.args_json
    }

    pub fn yield_items(&self) -> &[(String, String)] {
        &self.yield_items
    }
}
