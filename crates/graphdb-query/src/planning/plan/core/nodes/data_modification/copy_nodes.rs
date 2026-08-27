//! COPY Operation Plan Nodes
//!
//! Provides plan nodes for COPY FROM/TO CSV bulk import and export.

use crate::define_plan_node;

/// Target for COPY operation
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyTarget {
    Vertex(String),
    Edge(String),
}

impl CopyTarget {
    pub fn is_vertex(&self) -> bool {
        matches!(self, CopyTarget::Vertex(_))
    }

    pub fn is_edge(&self) -> bool {
        matches!(self, CopyTarget::Edge(_))
    }

    pub fn name(&self) -> &str {
        match self {
            CopyTarget::Vertex(s) | CopyTarget::Edge(s) => s,
        }
    }
}

define_plan_node! {
    pub struct CopyFromNode {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
        batch_size: usize,
    }
    enum: CopyFrom
    input: ZeroInputNode
}

impl CopyFromNode {
    pub fn new(
        id: i64,
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
        batch_size: usize,
    ) -> Self {
        Self {
            id,
            space_name,
            target,
            file_path,
            header,
            delimiter,
            batch_size,
            output_var: None,
            col_names: vec!["copy_result".to_string()],
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn target(&self) -> &CopyTarget {
        &self.target
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn header(&self) -> bool {
        self.header
    }

    pub fn delimiter(&self) -> char {
        self.delimiter
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }
}

define_plan_node! {
    pub struct CopyToNode {
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
    }
    enum: CopyTo
    input: ZeroInputNode
}

impl CopyToNode {
    pub fn new(
        id: i64,
        space_name: String,
        target: CopyTarget,
        file_path: String,
        header: bool,
        delimiter: char,
    ) -> Self {
        Self {
            id,
            space_name,
            target,
            file_path,
            header,
            delimiter,
            output_var: None,
            col_names: vec!["copy_result".to_string()],
            column_types: vec![],
        }
    }

    pub fn space_name(&self) -> &str {
        &self.space_name
    }

    pub fn target(&self) -> &CopyTarget {
        &self.target
    }

    pub fn file_path(&self) -> &str {
        &self.file_path
    }

    pub fn header(&self) -> bool {
        self.header
    }

    pub fn delimiter(&self) -> char {
        self.delimiter
    }
}
