use linkrs_core::types::{EdgeTypeInfo, TagInfo};
use linkrs_core::{Edge, Vertex};

pub(crate) fn vertex_is_expired(tag_info: &TagInfo, vertex: &Vertex) -> bool {
    linkrs_core::types::ttl::is_record_expired(
        tag_info.ttl_duration,
        tag_info.ttl_col.as_deref(),
        vertex.properties(),
    )
}

pub(crate) fn edge_is_expired(edge_info: &EdgeTypeInfo, edge: &Edge) -> bool {
    linkrs_core::types::ttl::is_record_expired(
        edge_info.ttl_duration,
        edge_info.ttl_col.as_deref(),
        edge.properties(),
    )
}
