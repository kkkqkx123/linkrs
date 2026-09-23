use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::types::VertexId;
use crate::value::Value;

/// Represents a tag in the graph: one label with its property map.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub properties: HashMap<String, Value>,
}

// Implement Hash manually for Tag to handle HashMap hashing
impl std::hash::Hash for Tag {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        // For HashMap, we'll hash key-value pairs in sorted order
        let mut pairs: Vec<_> = self.properties.iter().collect();
        pairs.sort_by_key(|&(k, _)| k);
        for (k, v) in pairs {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl Tag {
    pub fn new(name: String, properties: HashMap<String, Value>) -> Self {
        Self { name, properties }
    }

    /// Estimate the memory usage of the label.
    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();

        // Calculate the actual size of the variable `name` (including heap allocation).
        size += std::mem::size_of::<String>() + self.name.capacity();

        // Hash table bucket array overhead: u64 hash per entry
        size += self.properties.capacity()
            * (8 + std::mem::size_of::<String>() + std::mem::size_of::<Value>());

        for (k, v) in &self.properties {
            size += k.capacity();
            size += v.estimated_size();
        }

        size
    }
}

/// Represents a vertex in the graph: exactly one label with its properties.
///
/// Single-label is enforced by construction: there is no tag list, no
/// vertex-level property map, and no internal row id. A vertex always carries
/// precisely one tag.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Vertex {
    pub vid: VertexId,
    pub tag: Tag,
}

// Implementing a hash function manually to handle the hashing of a HashMap
impl std::hash::Hash for Vertex {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.vid.hash(state);
        self.tag.hash(state);
    }
}

impl Vertex {
    pub fn new(vid: VertexId, tag: Tag) -> Self {
        Self { vid, tag }
    }

    pub fn vid(&self) -> &VertexId {
        &self.vid
    }

    pub fn tag(&self) -> &Tag {
        &self.tag
    }

    pub fn tag_name(&self) -> &str {
        &self.tag.name
    }

    pub fn properties(&self) -> &HashMap<String, Value> {
        &self.tag.properties
    }

    pub fn get_property(&self, prop_name: &str) -> Option<&Value> {
        self.tag.properties.get(prop_name)
    }

    pub fn property_value(&self, property: &str) -> Option<Value> {
        self.tag.properties.get(property).cloned()
    }

    pub fn has_properties(&self) -> bool {
        !self.tag.properties.is_empty()
    }

    fn cmp_properties(
        a: &HashMap<String, Value>,
        b: &HashMap<String, Value>,
    ) -> std::cmp::Ordering {
        match a.len().cmp(&b.len()) {
            std::cmp::Ordering::Equal => {
                let mut a_sorted: Vec<_> = a.iter().collect();
                let mut b_sorted: Vec<_> = b.iter().collect();
                a_sorted.sort_by_key(|(k1, _)| *k1);
                b_sorted.sort_by_key(|(k1, _)| *k1);

                for ((k1, v1), (k2, v2)) in a_sorted.iter().zip(b_sorted.iter()) {
                    match k1.cmp(k2) {
                        std::cmp::Ordering::Equal => match v1.cmp(v2) {
                            std::cmp::Ordering::Equal => continue,
                            ord => return ord,
                        },
                        ord => return ord,
                    }
                }
                std::cmp::Ordering::Equal
            }
            ord => ord,
        }
    }
}

impl Ord for Vertex {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Use chained comparisons to improve readability.
        self.vid.cmp(&other.vid).then_with(|| {
            self.tag
                .name
                .cmp(&other.tag.name)
                .then_with(|| {
                    Vertex::cmp_properties(&self.tag.properties, &other.tag.properties)
                })
        })
    }
}

impl PartialOrd for Vertex {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Vertex {
    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();

        size += self.vid.len();

        size += self.tag.estimated_size();

        size
    }
}

/// Represents an edge in the graph, similar to Nebula's Edge structure
///
/// Edges are uniquely identified by (src, dst, edge_type, ranking).
/// The internal edge ID is managed by the storage layer and is not exposed to users.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    pub src: VertexId,
    pub dst: VertexId,
    pub edge_type: String,
    pub ranking: i64,
    pub props: HashMap<String, Value>,
}

impl Edge {
    pub fn properties(&self) -> &HashMap<String, Value> {
        &self.props
    }

    pub fn src(&self) -> &VertexId {
        &self.src
    }

    pub fn dst(&self) -> &VertexId {
        &self.dst
    }

    pub fn edge_type(&self) -> &str {
        &self.edge_type
    }

    pub fn ranking(&self) -> i64 {
        self.ranking
    }

    pub fn get_all_properties(&self) -> &HashMap<String, Value> {
        &self.props
    }

    /// Get a specific property by name
    pub fn get_property(&self, name: &str) -> Option<&Value> {
        self.props.get(name)
    }

    /// Set a property value
    pub fn set_property(&mut self, name: String, value: Value) {
        self.props.insert(name, value);
    }

    /// Remove a property
    pub fn remove_property(&mut self, name: &str) -> Option<Value> {
        self.props.remove(name)
    }

    /// Check if edge has a specific property
    pub fn has_property(&self, name: &str) -> bool {
        self.props.contains_key(name)
    }

    /// Get the number of properties
    pub fn property_count(&self) -> usize {
        self.props.len()
    }

    /// Check if edge has any properties
    pub fn has_properties(&self) -> bool {
        !self.props.is_empty()
    }

    pub fn new_empty(src: VertexId, dst: VertexId, edge_type: String, ranking: i64) -> Self {
        Self {
            src,
            dst,
            edge_type,
            ranking,
            props: HashMap::new(),
        }
    }

    pub fn debug_string(&self) -> String {
        format!(
            "Edge({:?} -> {:?}, type: {}, ranking: {})",
            self.src, self.dst, self.edge_type, self.ranking
        )
    }

    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();

        size += self.src.len();
        size += self.dst.len();

        size += std::mem::size_of::<String>() + self.edge_type.capacity();

        size += self.props.capacity()
            * (8 + std::mem::size_of::<String>() + std::mem::size_of::<Value>());

        for (k, v) in &self.props {
            size += k.capacity();
            size += v.estimated_size();
        }

        size
    }
}

// Implement Hash manually for Edge to handle HashMap hashing
impl std::hash::Hash for Edge {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.src.hash(state);
        self.dst.hash(state);
        self.edge_type.hash(state);
        self.ranking.hash(state);
        // For HashMap, we'll hash key-value pairs in sorted order
        let mut pairs: Vec<_> = self.props.iter().collect();
        pairs.sort_by_key(|&(k, _)| k);
        for (k, v) in pairs {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl Edge {
    pub fn new(
        src: VertexId,
        dst: VertexId,
        edge_type: String,
        ranking: i64,
        props: HashMap<String, Value>,
    ) -> Self {
        Self {
            src,
            dst,
            edge_type,
            ranking,
            props,
        }
    }
}

impl Ord for Edge {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Use chained comparisons to improve readability.
        self.src
            .cmp(&other.src)
            .then_with(|| self.dst.cmp(&other.dst))
            .then_with(|| self.edge_type.cmp(&other.edge_type))
            .then_with(|| self.ranking.cmp(&other.ranking))
            .then_with(|| Vertex::cmp_properties(&self.props, &other.props))
    }
}

impl PartialOrd for Edge {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// Key of one edge for batch deletion, mirroring the identity half of `Edge.
///
/// Carries no properties: deletes address `(src, dst, edge_type, ranking)`
/// only, so callers deleting many edges avoid building full payload edges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EdgeDeleteKey {
    pub src: VertexId,
    pub dst: VertexId,
    pub edge_type: String,
    pub ranking: i64,
}

impl EdgeDeleteKey {
    pub fn new(src: VertexId, dst: VertexId, edge_type: String, ranking: i64) -> Self {
        Self {
            src,
            dst,
            edge_type,
            ranking,
        }
    }
}

/// Represents a step in a path
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Step {
    pub dst: Box<Vertex>,
    pub edge: Box<Edge>,
}

impl std::hash::Hash for Step {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.dst.hash(state);
        self.edge.hash(state);
    }
}

impl Ord for Step {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Comparison order: dst -> edge
        match self.dst.cmp(&other.dst) {
            std::cmp::Ordering::Equal => self.edge.cmp(&other.edge),
            ord => ord,
        }
    }
}

impl PartialOrd for Step {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Step {
    pub fn new(dst: Vertex, edge_type: String, _edge_name: String, ranking: i64) -> Self {
        let edge = Edge::new_empty(VertexId::new(), VertexId::new(), edge_type, ranking);
        Self {
            dst: Box::new(dst),
            edge: Box::new(edge),
        }
    }

    pub fn new_with_edge(dst: Vertex, edge: Edge) -> Self {
        Self {
            dst: Box::new(dst),
            edge: Box::new(edge),
        }
    }

    pub fn src_vid(&self) -> &VertexId {
        &self.edge.src
    }

    pub fn dst_vid(&self) -> &VertexId {
        &self.dst.vid
    }

    /// Obtaining the ranking of the edges
    pub fn ranking(&self) -> i64 {
        self.edge.ranking
    }

    /// Estimating the memory usage of the steps in a process
    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();

        // Calculate the actual sizes of dst and edge (including the heap allocation for the Box and its content).
        size += std::mem::size_of::<Box<Vertex>>() + self.dst.estimated_size();
        size += std::mem::size_of::<Box<Edge>>() + self.edge.estimated_size();

        size
    }
}

/// Represents a path in the graph
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Path {
    pub src: Box<Vertex>,
    pub steps: Vec<Step>,
}

impl Ord for Path {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Use chained comparisons to improve readability.
        self.src
            .cmp(&other.src)
            .then_with(|| self.steps.len().cmp(&other.steps.len()))
            .then_with(|| self.cmp_steps(other))
    }
}

impl Path {
    /// Compare the steps in the paths
    fn cmp_steps(&self, other: &Self) -> std::cmp::Ordering {
        // Compare each step.
        for (step1, step2) in self.steps.iter().zip(other.steps.iter()) {
            let step_cmp = step1.cmp(step2);
            if step_cmp != std::cmp::Ordering::Equal {
                return step_cmp;
            }
        }
        std::cmp::Ordering::Equal
    }
}

impl PartialOrd for Path {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

// Implementing a hash function manually to handle complex data types
impl std::hash::Hash for Path {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.src.hash(state);
        for step in &self.steps {
            step.hash(state);
        }
    }
}

impl Path {
    /// Create a new path.
    pub fn new(src: Vertex) -> Self {
        Self {
            src: Box::new(src),
            steps: Vec::new(),
        }
    }

    /// Estimate the memory usage of the path
    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();

        // Calculate the actual size of `src` (including the heap allocation for the `Box` and the content of the `Vertex`).
        size += std::mem::size_of::<Box<Vertex>>() + self.src.as_ref().estimated_size();

        // Calculate the memory overhead for the capacity of Vec<Step>
        size += self.steps.capacity() * std::mem::size_of::<Step>();

        for step in &self.steps {
            size += step.estimated_size();
        }

        size
    }

    /// Add steps to the path
    pub fn add_step(&mut self, step: Step) {
        self.steps.push(step);
    }

    /// Obtain the edges in the path
    pub fn edges(&self) -> Vec<&Edge> {
        self.steps.iter().map(|step| step.edge.as_ref()).collect()
    }

    /// Get the path length (number of steps)
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Get path length (number of steps)
    pub fn length(&self) -> usize {
        self.steps.len()
    }

    /// Obtain the steps in the path
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// Check whether the path is empty (contains only the source vertex).
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Check for duplicate edges in the path
    pub fn has_duplicate_edges(&self) -> bool {
        let mut seen_edges: std::collections::HashSet<(VertexId, VertexId, String)> =
            std::collections::HashSet::new();

        for step in &self.steps {
            let edge_key = (step.edge.src, step.edge.dst, step.edge.edge_type.clone());

            if !seen_edges.insert(edge_key) {
                return true;
            }
        }

        false
    }

    pub fn reverse(&mut self) {
        if self.steps.is_empty() {
            return;
        }

        self.steps.reverse();

        for step in &mut self.steps {
            std::mem::swap(&mut step.edge.src, &mut step.edge.dst);
        }

        if let Some(last_step) = self.steps.first() {
            *self.src = (*last_step.dst).clone();
        }
    }

    /// Append reverse path (for bidirectional BFS path splicing)
    /// Append the reverse of another path to the current path
    pub fn append_reverse(&mut self, other: Path) {
        if other.steps.is_empty() {
            return;
        }

        // Reverse steps to get other paths
        let mut other_steps: Vec<Step> = other.steps.into_iter().rev().collect();

        // Reverse the direction of each edge
        for step in &mut other_steps {
            std::mem::swap(&mut step.edge.src, &mut step.edge.dst);
        }

        // Append to current path
        self.steps.extend(other_steps);
    }
}

impl Default for Path {
    fn default() -> Self {
        Self {
            src: Box::new(Vertex::default()),
            steps: Vec::new(),
        }
    }
}
