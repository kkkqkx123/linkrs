use crate::{PurgeTarget, TtlPurgeEntry};
use linkrs_core::types::VertexId;
use std::collections::HashMap;

use linkrs_core::{StorageResult, Value};

use super::context::GraphStorageContext;
use super::GraphStorage;

/// Rows deleted between flushes inside a purge. Bounds the working set of the
/// delete loop instead of collecting every expired id up front, so a large
/// table cannot spike memory. Per-schema deletions stay sequential.
pub(crate) const PURGE_DELETE_BATCH: usize = 1024;

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

/// Delete one bounded batch of expired vertices in order, draining the batch.
fn flush_vertex_batch(
    ctx: &GraphStorageContext,
    space: &str,
    tag: &str,
    batch: &mut Vec<VertexId>,
) -> StorageResult<usize> {
    let mut removed = 0;
    for vid in batch.drain(..) {
        super::writer::delete_vertex(ctx, space, tag, &vid)?;
        removed += 1;
    }
    Ok(removed)
}

/// Delete one bounded batch of expired edges in order, draining the batch.
fn flush_edge_batch(
    ctx: &GraphStorageContext,
    space: &str,
    edge_type: &str,
    batch: &mut Vec<(VertexId, VertexId, i64)>,
) -> StorageResult<usize> {
    let mut removed = 0;
    for (src, dst, rank) in batch.drain(..) {
        super::writer::delete_edge(ctx, space, &src, &dst, edge_type, rank)?;
        removed += 1;
    }
    Ok(removed)
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
        let ttl_duration = tag_info.ttl_duration;
        let ttl_col = tag_info.ttl_col.as_deref();
        let mut removed = 0;
        let mut batch: Vec<VertexId> = Vec::with_capacity(PURGE_DELETE_BATCH.min(records.len()));
        for record in &records {
            if record_expired_from_slice(ttl_duration, ttl_col, &record.properties) {
                batch.push(record.vid);
                if batch.len() >= PURGE_DELETE_BATCH {
                    removed += flush_vertex_batch(ctx, space, tag, &mut batch)?;
                }
            }
        }
        removed += flush_vertex_batch(ctx, space, tag, &mut batch)?;
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
        let mut removed = 0;
        let mut batch: Vec<(VertexId, VertexId, i64)> = Vec::with_capacity(PURGE_DELETE_BATCH);
        loop {
            let chunk = cursor.next_batch(PURGE_DELETE_BATCH)?;
            if chunk.is_empty() {
                break;
            }
            for edge in &chunk {
                if linkrs_core::types::ttl::is_record_expired(
                    edge_info.ttl_duration,
                    edge_info.ttl_col.as_deref(),
                    edge.properties(),
                ) {
                    batch.push((edge.src, edge.dst, edge.ranking));
                    if batch.len() >= PURGE_DELETE_BATCH {
                        removed += flush_edge_batch(ctx, space, edge_type, &mut batch)?;
                    }
                }
            }
        }
        removed += flush_edge_batch(ctx, space, edge_type, &mut batch)?;
        Ok(removed)
    }

    /// Physical-deletion entry point shared by the `PURGE EXPIRED` statement
    /// and maintenance callers.
    ///
    /// `target = None` scans every TTL-configured tag and edge type in the
    /// space, returning one [`TtlPurgeEntry`] per such schema. `target =
    /// Some(..)` scans only that schema, returning its single entry (a schema
    /// that exists but has no TTL yields `removed = 0`; a schema that does not
    /// exist is an error).
    pub fn purge_expired(
        &self,
        space: &str,
        target: Option<PurgeTarget<'_>>,
    ) -> StorageResult<Vec<TtlPurgeEntry>> {
        match target {
            Some(PurgeTarget::Tag(name)) => {
                let removed = self.purge_expired_vertices(space, name)?;
                Ok(vec![TtlPurgeEntry {
                    schema_name: name.to_string(),
                    is_edge: false,
                    removed,
                }])
            }
            Some(PurgeTarget::Edge(name)) => {
                let removed = self.purge_expired_edges(space, name)?;
                Ok(vec![TtlPurgeEntry {
                    schema_name: name.to_string(),
                    is_edge: true,
                    removed,
                }])
            }
            None => {
                let ctx: &GraphStorageContext = &self.ctx;
                let mut entries = Vec::new();
                for tag in ctx.schema_manager().list_tags(space)? {
                    if linkrs_core::types::ttl::has_valid_ttl(
                        tag.ttl_duration,
                        tag.ttl_col.as_deref(),
                    ) {
                        let removed = self.purge_expired_vertices(space, &tag.tag_name)?;
                        entries.push(TtlPurgeEntry {
                            schema_name: tag.tag_name,
                            is_edge: false,
                            removed,
                        });
                    }
                }
                for edge in ctx.schema_manager().list_edge_types(space)? {
                    if linkrs_core::types::ttl::has_valid_ttl(
                        edge.ttl_duration,
                        edge.ttl_col.as_deref(),
                    ) {
                        let removed = self.purge_expired_edges(space, &edge.edge_type_name)?;
                        entries.push(TtlPurgeEntry {
                            schema_name: edge.edge_type_name,
                            is_edge: true,
                            removed,
                        });
                    }
                }
                Ok(entries)
            }
        }
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
        assert!(!record_expired_from_slice(
            Some(10),
            Some("missing"),
            &props
        ));
    }

    #[test]
    fn edge_types_ttl_active_covers_three_states() {
        use crate::{StorageReader, StorageSchemaOps};
        use linkrs_core::types::{EdgeTypeInfo, PropertyDef, SpaceInfo};

        let mut storage = GraphStorage::new().expect("create storage");

        // Space with no TTL edge type at all.
        let mut nottl = SpaceInfo::new("nottl".to_string());
        StorageSchemaOps::create_space(&mut storage, &mut nottl).expect("create space");
        let plain_only =
            EdgeTypeInfo::new("PLAIN".to_string()).with_properties(vec![PropertyDef::new(
                "w".into(),
                linkrs_core::DataType::Int,
            )]);
        StorageSchemaOps::create_edge_type(&mut storage, "nottl", &plain_only)
            .expect("create plain edge");
        assert!(!storage
            .edge_types_ttl_active("nottl", &[])
            .expect("nottl all types"));
        assert!(!storage
            .edge_types_ttl_active("nottl", &["PLAIN".to_string()])
            .expect("nottl specific type"));

        // Space mixing a TTL edge type with a plain one.
        let mut mixed = SpaceInfo::new("mixed".to_string());
        StorageSchemaOps::create_space(&mut storage, &mut mixed).expect("create space");
        StorageSchemaOps::create_edge_type(&mut storage, "mixed", &plain_only)
            .expect("create plain edge");
        let ttl_edge = EdgeTypeInfo::new("TTLED".to_string())
            .with_properties(vec![PropertyDef::new(
                "ts".into(),
                linkrs_core::DataType::BigInt,
            )])
            .with_ttl(Some(100), Some("ts".to_string()));
        StorageSchemaOps::create_edge_type(&mut storage, "mixed", &ttl_edge)
            .expect("create ttl edge");

        // Querying the specific non-TTL type stays inactive.
        assert!(!storage
            .edge_types_ttl_active("mixed", &["PLAIN".to_string()])
            .expect("mixed plain type"));
        // Partial activation: querying the TTL type is active.
        assert!(storage
            .edge_types_ttl_active("mixed", &["TTLED".to_string()])
            .expect("mixed ttled type"));
        // Empty means all edge types, which include a TTL one.
        assert!(storage
            .edge_types_ttl_active("mixed", &[])
            .expect("mixed all types"));
    }
}
