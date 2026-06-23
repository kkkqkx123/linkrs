//! Data Modification Type Definition

use crate::core::Value;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertVertexInfo {
    pub space_id: u64,
    pub vertex_id: Value,
    pub tag_name: String,
    pub props: Vec<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InsertEdgeInfo {
    pub space_id: u64,
    pub src_vertex_id: Value,
    pub dst_vertex_id: Value,
    pub edge_name: String,
    pub rank: i64,
    pub props: Vec<(String, Value)>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateTarget {
    pub space_name: String,
    pub label: String,
    pub id: Value,
    pub prop: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum UpdateOp {
    Set,
    Add,
    Subtract,
    Append,
    Remove,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateInfo {
    pub update_target: UpdateTarget,
    pub update_op: UpdateOp,
    pub value: Value,
}
