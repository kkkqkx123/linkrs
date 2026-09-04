use graphdb_core::types::semantic::ValueType;
use graphdb_core::types::EdgeDirection;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct BoundTagRef {
    pub tag_name: Arc<str>,
    pub properties: HashMap<String, ValueType>,
}

#[derive(Debug, Clone)]
pub struct BoundEdgeTypeRef {
    pub edge_type_name: Arc<str>,
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
    node_index: HashMap<String, usize>,
    edge_index: HashMap<String, usize>,
}

impl QueryGraph {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            edges: Vec::new(),
            node_index: HashMap::new(),
            edge_index: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, node: BoundNodePattern) {
        let idx = self.nodes.len();
        self.node_index.insert(node.variable.clone(), idx);
        self.nodes.push(node);
    }

    pub fn add_edge(&mut self, edge: BoundEdgePattern) {
        let idx = self.edges.len();
        self.edge_index.insert(edge.variable.clone(), idx);
        self.edges.push(edge);
    }

    pub fn find_node(&self, variable: &str) -> Option<&BoundNodePattern> {
        self.node_index
            .get(variable)
            .and_then(|&idx| self.nodes.get(idx))
    }

    pub fn find_edge(&self, variable: &str) -> Option<&BoundEdgePattern> {
        self.edge_index
            .get(variable)
            .and_then(|&idx| self.edges.get(idx))
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
