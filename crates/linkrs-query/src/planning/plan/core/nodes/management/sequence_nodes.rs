//! Sequence catalog management nodes.
//!
//! Plan nodes for `CREATE/ALTER/DROP SEQUENCE`. They carry the validated
//! sequence payloads; the spec builder lowers them to
//! `DdlSpec::SequenceManage`.

use crate::define_plan_node;

define_plan_node! {
    pub struct CreateSequenceNode {
        info: SequenceManageInfo,
    }
    manage_enum: SequenceManageNode::Create as SequenceManage
    input: ZeroInputNode
}

impl CreateSequenceNode {
    pub fn new(id: i64, info: SequenceManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &SequenceManageInfo {
        &self.info
    }

    pub fn seq_name(&self) -> &str {
        &self.info.seq_name
    }
}

define_plan_node! {
    pub struct AlterSequenceNode {
        info: SequenceManageInfo,
    }
    manage_enum: SequenceManageNode::Alter as SequenceManage
    input: ZeroInputNode
}

impl AlterSequenceNode {
    pub fn new(id: i64, info: SequenceManageInfo) -> Self {
        Self {
            id,
            info,
            output_var: None,
            col_names: Vec::new(),
            column_types: vec![],
        }
    }

    pub fn info(&self) -> &SequenceManageInfo {
        &self.info
    }

    pub fn seq_name(&self) -> &str {
        &self.info.seq_name
    }
}

define_plan_node! {
    pub struct DropSequenceNode {
        seq_name: String,
        if_exists: bool,
    }
    manage_enum: SequenceManageNode::Drop as SequenceManage
    input: ZeroInputNode
}

impl DropSequenceNode {
    pub fn new(id: i64, seq_name: String) -> Self {
        Self {
            id,
            seq_name,
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

    pub fn seq_name(&self) -> &str {
        &self.seq_name
    }

    pub fn if_exists(&self) -> bool {
        self.if_exists
    }
}

/// Sequence catalog payload shared by `CREATE` and `ALTER SEQUENCE`.
///
/// `None` option fields mean "clause omitted": the executor substitutes the
/// documented defaults on create and leaves the current value untouched on
/// alter.
#[derive(Debug, Clone)]
pub struct SequenceManageInfo {
    pub seq_name: String,
    pub start: Option<i64>,
    pub increment: Option<i64>,
    pub min_value: Option<i64>,
    pub max_value: Option<i64>,
    pub cycle: Option<bool>,
    pub if_not_exists: bool,
}

impl SequenceManageInfo {
    pub fn new(seq_name: String) -> Self {
        Self {
            seq_name,
            start: None,
            increment: None,
            min_value: None,
            max_value: None,
            cycle: None,
            if_not_exists: false,
        }
    }

    pub fn with_start(mut self, start: Option<i64>) -> Self {
        self.start = start;
        self
    }

    pub fn with_increment(mut self, increment: Option<i64>) -> Self {
        self.increment = increment;
        self
    }

    pub fn with_bounds(mut self, min_value: Option<i64>, max_value: Option<i64>) -> Self {
        self.min_value = min_value;
        self.max_value = max_value;
        self
    }

    pub fn with_cycle(mut self, cycle: Option<bool>) -> Self {
        self.cycle = cycle;
        self
    }

    pub fn with_if_not_exists(mut self, if_not_exists: bool) -> Self {
        self.if_not_exists = if_not_exists;
        self
    }
}
