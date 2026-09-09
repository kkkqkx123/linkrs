//! Macro and type-alias catalog management nodes.
//!
//! Plan nodes for `CREATE/DROP MACRO` and `CREATE/DROP TYPE`. They carry the
//! validated catalog payloads; the spec builder lowers them to
//! `DdlSpec::MacroManage` / `DdlSpec::TypeManage`.

use crate::define_plan_node;
use graphdb_core::metadata::MacroParamDef;
use graphdb_core::types::expr::Expression;

define_plan_node! {
    pub struct CreateMacroNode {
        info: MacroManageInfo,
    }
    manage_enum: MacroManageNode::Create as MacroManage
    input: ZeroInputNode
}

impl CreateMacroNode {
    pub fn new(id: i64, info: MacroManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &MacroManageInfo {
        &self.info
    }

    pub fn macro_name(&self) -> &str {
        &self.info.macro_name
    }
}

define_plan_node! {
    pub struct DropMacroNode {
        macro_name: String,
        if_exists: bool,
    }
    manage_enum: MacroManageNode::Drop as MacroManage
    input: ZeroInputNode
}

impl DropMacroNode {
    pub fn new(id: i64, macro_name: String) -> Self {
        Self {
            id,
            macro_name,
            if_exists: false,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn macro_name(&self) -> &str {
        &self.macro_name
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

define_plan_node! {
    pub struct CreateTypeNode {
        info: TypeManageInfo,
    }
    manage_enum: TypeManageNode::Create as TypeManage
    input: ZeroInputNode
}

impl CreateTypeNode {
    pub fn new(id: i64, info: TypeManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &TypeManageInfo {
        &self.info
    }

    pub fn alias_name(&self) -> &str {
        &self.info.type_name
    }
}

define_plan_node! {
    pub struct DropTypeNode {
        type_name: String,
        if_exists: bool,
    }
    manage_enum: TypeManageNode::Drop as TypeManage
    input: ZeroInputNode
}

impl DropTypeNode {
    pub fn new(id: i64, type_name: String) -> Self {
        Self {
            id,
            type_name,
            if_exists: false,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn with_if_exists(mut self, if_exists: bool) -> Self {
        self.if_exists = if_exists;
        self
    }

    pub fn alias_name(&self) -> &str {
        &self.type_name
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

/// Macro catalog payload for `CREATE MACRO`.
#[derive(Debug, Clone)]
pub struct MacroManageInfo {
    pub macro_name: String,
    pub params: Vec<MacroParamDef>,
    pub body: Expression,
    pub if_not_exists: bool,
}

impl MacroManageInfo {
    pub fn new(
        macro_name: String,
        params: Vec<MacroParamDef>,
        body: Expression,
        if_not_exists: bool,
    ) -> Self {
        Self {
            macro_name,
            params,
            body,
            if_not_exists,
        }
    }
}

/// Type-alias catalog payload for `CREATE TYPE`.
#[derive(Debug, Clone)]
pub struct TypeManageInfo {
    pub type_name: String,
    /// Raw underlying type text (builtin spelling or alias reference).
    pub underlying_type: String,
    pub if_not_exists: bool,
}

impl TypeManageInfo {
    pub fn new(type_name: String, underlying_type: String, if_not_exists: bool) -> Self {
        Self {
            type_name,
            underlying_type,
            if_not_exists,
        }
    }
}
