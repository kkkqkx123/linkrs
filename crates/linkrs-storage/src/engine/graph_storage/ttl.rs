use std::collections::HashMap;

use linkrs_core::{StorageResult, Value};

use super::context::GraphStorageContext;
use super::GraphStorage;

/// TTL check over a raw property slice — the shared primitive for read
/// paths that hold `VertexRecord`/`EdgeRecord` slices instead of a
/// materialized `Vertex`/`Edge`.
pub(crate) fn record_expired_from_slice(
    ttl_duration: Option<i64>,
    ttl_col: Option<&str>,
    props: &[(std::sync::Arc<str>, Value)],
) -> bool {
    let (Some(duration), Some(col)) = (ttl_duration, ttl_col) else {
        return false;
    };
    if duration <= 0 || col.is_empty() {
        return false;
    }
    let map: HashMap<std::sync::Arc<str>, Value> = props.iter().cloned().collect();
    linkrs_core::types::ttl::is_record_expired(Some(duration), Some(col), &map)
}

impl GraphStorage {
    pub fn purge_expired_vertices(&self, space: &str, tag: &str) -> StorageResult<usize> {
        let ctx: &GraphStorageContext = &self.ctx;
        let tag_info = ctx.schema_manager().get_tag(space, tag)?.ok_or_else(|| {
            linkrs_core::StorageError::not_found(format!("Tag {tag} not found in space {space}"))
        })?;
        if !linkrs_core::types::ttl::has_valid_ttl(
            tag_info.ttl_duration,
            tag_info.ttl_col.as_deref(),
        ) {
            return Ok(0);
        }
        let ts = ctx.get_read_timestamp();
        let Some(records) = ctx.scan_vertices(tag_info.tag_id, ts) else {
            return Ok(0);
        };
        let mut expired = Vec::new();
        for record in records {
            if record_expired_from_slice(
                tag_info.ttl_duration,
                tag_info.ttl_col.as_deref(),
                &record.properties,
            ) {
                expired.push(record.vid);
            }
        }
        let mut removed = 0;
        for vid in expired {
            super::writer::delete_vertex(ctx, space, tag, &vid)?;
            removed += 1;
        }
        Ok(removed)
    }

    pub fn purge_expired_edges(&self, space: &str, edge_type: &str) -> StorageResult<usize> {
        let ctx: &GraphStorageContext = &self.ctx;
        let edge_info = ctx
            .schema_manager()
            .get_edge_type(space, edge_type)?
            .ok_or_else(|| {
                linkrs_core::StorageError::not_found(format!(
                    "Edge type {edge_type} not found in space {space}"
                ))
            })?;
        if !linkrs_core::types::ttl::has_valid_ttl(
            edge_info.ttl_duration,
            edge_info.ttl_col.as_deref(),
        ) {
            return Ok(0);
        }
        let mut cursor = super::cursor_impl::create_edge_cursor(
            std::sync::Arc::new(ctx.clone()),
            space,
            &crate::cursor::ScanOptions {
                edge_type: Some(edge_type.to_string()),
                ..Default::default()
            },
        )?;
        let mut expired = Vec::new();
        loop {
            let batch = cursor.next_batch(1024)?;
            if batch.is_empty() {
                break;
            }
            for edge in &batch {
                if linkrs_core::types::ttl::is_record_expired(
                    edge_info.ttl_duration,
                    edge_info.ttl_col.as_deref(),
                    edge.properties(),
                ) {
                    expired.push((edge.src, edge.dst, edge.ranking));
                }
            }
        }
        let mut removed = 0;
        for (src, dst, rank) in expired {
            super::writer::delete_edge(ctx, space, &src, &dst, edge_type, rank)?;
            removed += 1;
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_helper_matches_record_helper() {
        let props: Vec<(std::sync::Arc<str>, Value)> =
            vec![(std::sync::Arc::<str>::from("ts"), Value::BigInt(100))];
        assert!(record_expired_from_slice(Some(10), Some("ts"), &props));
        assert!(!record_expired_from_slice(None, None, &props));
        assert!(!record_expired_from_slice(Some(10), Some("missing"), &props));
    }
}
