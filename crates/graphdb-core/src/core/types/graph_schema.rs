//! Graph Structure Type Definition
//!
//! Contains type definitions related to graph structures in the graph database

use crate::core::DataType;
use serde::{Deserialize, Serialize};

/// Connection type enumeration
///
/// Used to represent the type of join operation in a SQL/graph query
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JoinType {
    /// internal link
    Inner,
    /// left outer join
    Left,
    /// right outer link
    Right,
    /// full external link
    Full,
    /// Cartesian product (cross linking)
    Cross,
}

impl JoinType {
    /// Get the name of the connection type
    pub fn name(&self) -> &'static str {
        match self {
            JoinType::Inner => "INNER",
            JoinType::Left => "LEFT",
            JoinType::Right => "RIGHT",
            JoinType::Full => "FULL",
            JoinType::Cross => "CROSS",
        }
    }

    /// Determine if it is an outer connection (Left/Right/Full)
    pub fn is_outer(&self) -> bool {
        matches!(self, JoinType::Left | JoinType::Right | JoinType::Full)
    }

    /// Determining whether a connection is an inner connection
    pub fn is_inner(&self) -> bool {
        matches!(self, JoinType::Inner)
    }
}

impl From<&str> for JoinType {
    fn from(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "INNER" => JoinType::Inner,
            "LEFT" => JoinType::Left,
            "RIGHT" => JoinType::Right,
            "FULL" => JoinType::Full,
            "CROSS" => JoinType::Cross,
            _ => JoinType::Inner,
        }
    }
}

/// Sort Direction Enumeration
///
/// Used to indicate the direction of sorting in an ORDER BY clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    /// ascending order
    Asc,
    /// descending order
    Desc,
}

impl OrderDirection {
    /// Get the name of the sort direction
    pub fn name(&self) -> &'static str {
        match self {
            OrderDirection::Asc => "ASC",
            OrderDirection::Desc => "DESC",
        }
    }

    /// Get reverse sort direction
    pub fn reverse(&self) -> Self {
        match self {
            OrderDirection::Asc => OrderDirection::Desc,
            OrderDirection::Desc => OrderDirection::Asc,
        }
    }
}

impl From<&str> for OrderDirection {
    fn from(s: &str) -> Self {
        match s.to_uppercase().as_str() {
            "ASC" | "ASCENDING" => OrderDirection::Asc,
            "DESC" | "DESCENDING" => OrderDirection::Desc,
            _ => OrderDirection::Asc,
        }
    }
}

impl From<bool> for OrderDirection {
    fn from(desc: bool) -> Self {
        if desc {
            OrderDirection::Desc
        } else {
            OrderDirection::Asc
        }
    }
}

/// Type of orientation of the edge
///
/// Used to represent the traversal direction of an edge, supports outgoing, incoming and bi-directional traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeDirection {
    /// Outgoing edge: from source node to target node
    Out,
    /// Incoming edge: pointing from target node to source node
    In,
    /// Bidirectional: contains both outgoing and incoming edges
    Both,
}

impl EdgeDirection {
    /// Determine whether to include outgoing edges
    pub fn is_outgoing(&self) -> bool {
        matches!(self, EdgeDirection::Out | EdgeDirection::Both)
    }

    /// Determine if the inbound edge is included
    pub fn is_incoming(&self) -> bool {
        matches!(self, EdgeDirection::In | EdgeDirection::Both)
    }

    /// Get Reverse Direction
    pub fn reverse(&self) -> Self {
        match self {
            EdgeDirection::Out => EdgeDirection::In,
            EdgeDirection::In => EdgeDirection::Out,
            EdgeDirection::Both => EdgeDirection::Both,
        }
    }

    /// Determine if it is positive (outgoing edge)
    /// For Forward/Backward naming compatibility
    pub fn is_forward(&self) -> bool {
        matches!(self, EdgeDirection::Out | EdgeDirection::Both)
    }

    /// Determine if it is inverted (inbound edge)
    /// For Forward/Backward naming compatibility
    pub fn is_backward(&self) -> bool {
        matches!(self, EdgeDirection::In | EdgeDirection::Both)
    }
}

impl From<&str> for EdgeDirection {
    fn from(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "out" | "outgoing" | "forward" => EdgeDirection::Out,
            "in" | "incoming" | "backward" => EdgeDirection::In,
            "both" | "bidirectional" => EdgeDirection::Both,
            _ => EdgeDirection::Both,
        }
    }
}

impl From<String> for EdgeDirection {
    fn from(s: String) -> Self {
        EdgeDirection::from(s.as_str())
    }
}

/// Vertex Type Definition
#[derive(Debug, Clone, PartialEq)]
pub struct VertexType {
    pub tag_id: Option<i32>,
    pub tag_name: String,
    pub properties: Vec<PropertyType>,
}

/// Attribute Type Definition
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PropertyType {
    pub name: String,
    pub type_def: DataType,
    pub is_nullable: bool,
}

/// Edge type reference definition
///
/// A simplified representation of edge types for the derivation of graph structure types, the
/// Contains the information needed for type derivation of source label, target label, and rank enabled state
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeTypeRef {
    pub edge_type: i32,
    pub edge_name: String,
    pub src_tag: String,
    pub dst_tag: String,
    pub properties: Vec<PropertyType>,
    pub rank_enabled: bool,
}

/// Path Type Definition
#[derive(Debug, Clone, PartialEq)]
pub enum PathType {
    SimplePath,
    AllPaths,
    ShortestPath,
    NonWeightedShortestPath,
    WeightedShortestPath,
}

/// path information
#[derive(Debug, Clone, PartialEq)]
pub struct PathInfo {
    pub path_type: PathType,
    pub steps: Option<(i32, i32)>,
    pub node_types: Vec<VertexType>,
    pub edge_types: Vec<EdgeTypeRef>,
}

/// Graph Structure Type Derivator
pub struct GraphTypeInference;

impl Default for GraphTypeInference {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphTypeInference {
    pub fn new() -> Self {
        Self
    }

    /// Deriving Vertex Types
    pub fn deduce_vertex_type(&self, tag_name: &str, tag_id: Option<i32>) -> VertexType {
        VertexType {
            tag_id,
            tag_name: tag_name.to_string(),
            properties: Vec::new(),
        }
    }

    /// Derived edge types
    pub fn deduce_edge_type(&self, edge_name: &str, edge_type: i32) -> EdgeTypeRef {
        EdgeTypeRef {
            edge_type,
            edge_name: edge_name.to_string(),
            src_tag: String::new(),
            dst_tag: String::new(),
            properties: Vec::new(),
            rank_enabled: true,
        }
    }

    /// Inferred path type
    pub fn deduce_path_type(&self, path_type: PathType, steps: Option<(i32, i32)>) -> PathInfo {
        PathInfo {
            path_type,
            steps,
            node_types: Vec::new(),
            edge_types: Vec::new(),
        }
    }

    /// Deriving Attribute Types
    pub fn deduce_property_type(&self, prop_name: &str, _object_type: &str) -> Option<DataType> {
        match prop_name.to_lowercase().as_str() {
            "id" => Some(DataType::Int),
            "name" | "title" | "desc" | "description" => Some(DataType::String),
            "age" | "count" | "size" | "year" | "month" | "day" | "hour" | "minute" | "second" => {
                Some(DataType::Int)
            }
            "price" | "score" | "rate" | "ratio" | "percent" | "weight" | "height" | "width"
            | "length" => Some(DataType::Float),
            "created_at" | "updated_at" | "birthday" | "date" | "time" | "datetime" => {
                Some(DataType::DateTime)
            }
            "active" | "enabled" | "visible" | "valid" | "exists" => Some(DataType::Bool),
            "tags" | "labels" | "categories" => Some(DataType::List),
            "properties" | "attrs" | "attributes" => Some(DataType::Map),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graph_type_inference_creation() {
        let _inference = GraphTypeInference::new();
        // The test passes and is successful when it reaches this point
    }

    #[test]
    fn test_deduce_vertex_type() {
        let inference = GraphTypeInference::new();

        let vertex_type = inference.deduce_vertex_type("person", Some(1));
        assert_eq!(vertex_type.tag_name, "person");
        assert_eq!(vertex_type.tag_id, Some(1));
        assert!(vertex_type.properties.is_empty());
    }

    #[test]
    fn test_deduce_edge_type() {
        let inference = GraphTypeInference::new();

        let edge_type = inference.deduce_edge_type("knows", 2);
        assert_eq!(edge_type.edge_name, "knows");
        assert_eq!(edge_type.edge_type, 2);
        assert!(edge_type.rank_enabled);
        assert!(edge_type.properties.is_empty());
    }

    #[test]
    fn test_deduce_path_type() {
        let inference = GraphTypeInference::new();

        let path_info = inference.deduce_path_type(PathType::ShortestPath, Some((1, 3)));
        assert_eq!(path_info.path_type, PathType::ShortestPath);
        assert_eq!(path_info.steps, Some((1, 3)));
        assert!(path_info.node_types.is_empty());
        assert!(path_info.edge_types.is_empty());
    }

    #[test]
    fn test_deduce_property_type() {
        let inference = GraphTypeInference::new();

        assert_eq!(
            inference.deduce_property_type("id", "person"),
            Some(DataType::Int)
        );
        assert_eq!(
            inference.deduce_property_type("name", "person"),
            Some(DataType::String)
        );
        assert_eq!(
            inference.deduce_property_type("age", "person"),
            Some(DataType::Int)
        );
        assert_eq!(
            inference.deduce_property_type("price", "product"),
            Some(DataType::Float)
        );
        assert_eq!(
            inference.deduce_property_type("created_at", "person"),
            Some(DataType::DateTime)
        );
        assert_eq!(
            inference.deduce_property_type("active", "person"),
            Some(DataType::Bool)
        );
        assert_eq!(
            inference.deduce_property_type("tags", "person"),
            Some(DataType::List)
        );
        assert_eq!(
            inference.deduce_property_type("properties", "person"),
            Some(DataType::Map)
        );
        assert_eq!(inference.deduce_property_type("unknown", "person"), None);
    }
}
