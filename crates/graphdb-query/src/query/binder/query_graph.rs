use crate::core::types::semantic::ValueType;
use crate::core::types::EdgeDirection;
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct BoundTagRef {
    pub tag_name: String,
    pub properties: HashMap<String, ValueType>,
}

#[derive(Debug, Clone)]
pub struct BoundEdgeTypeRef {
    pub edge_type_name: String,
    pub properties: HashMap<String, ValueType>,
}

#[derive(Debug, Clone)]
pub struct BoundNodePattern {
    pub variable: String,
    pub tags: Vec<BoundTagRef>,
}

#[derive(Debug, Clone)]
pub struct BoundEdgePattern {
    pub variable: String,
    pub edge_types: Vec<BoundEdgeTypeRef>,
    pub direction: EdgeDirection,
    pub src_variable: String,
    pub dst_variable: String,
}

#[derive(Debug, Clone)]
pub struct QueryGraph {
    pub nodes: Vec<BoundNodePattern>,
    pub edges: Vec<BoundEdgePattern>,
}

impl QueryGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn add_node(&mut self, node: BoundNodePattern) {
        self.nodes.push(node);
    }

    pub fn add_edge(&mut self, edge: BoundEdgePattern) {
        self.edges.push(edge);
    }

    pub fn find_node(&self, variable: &str) -> Option<&BoundNodePattern> {
        self.nodes.iter().find(|n| n.variable == variable)
    }

    pub fn find_edge(&self, variable: &str) -> Option<&BoundEdgePattern> {
        self.edges.iter().find(|e| e.variable == variable)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }
}

impl Default for QueryGraph {
    fn default() -> Self {
        Self::new()
    }
}
