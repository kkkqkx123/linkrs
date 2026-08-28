use graphdb_core::types::{Timestamp, MAX_TIMESTAMP};
use graphdb_core::wal::EntityRef;
use graphdb_core::Value;
use std::sync::Arc;

pub(crate) type StaleChecker = Arc<dyn Fn(&EntityRef, Option<Timestamp>) -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy)]
pub struct EdgeIdentity<'a> {
    pub space_id: u64,
    pub src: &'a Value,
    pub dst: &'a Value,
    pub edge_type: &'a str,
    pub ranking: i64,
}

impl<'a> EdgeIdentity<'a> {
    pub fn new(
        space_id: u64,
        src: &'a Value,
        dst: &'a Value,
        edge_type: &'a str,
        ranking: i64,
    ) -> Self {
        Self {
            space_id,
            src,
            dst,
            edge_type,
            ranking,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct IndexIdentity {
    pub(crate) space_id: u64,
    pub(crate) index_id: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexRecord {
    pub created_ts: Timestamp,
    pub deleted_ts: Option<Timestamp>,
    pub entity_version: Option<Timestamp>,
    pub included_columns: Option<Vec<(String, Value)>>,
    pub entity_ref: Option<EntityRef>,
}

impl IndexRecord {
    pub fn new(created_ts: Timestamp) -> Self {
        Self {
            created_ts,
            deleted_ts: None,
            entity_version: None,
            included_columns: None,
            entity_ref: None,
        }
    }

    pub fn new_with_columns(created_ts: Timestamp, included_columns: Vec<(String, Value)>) -> Self {
        Self {
            created_ts,
            deleted_ts: None,
            entity_version: None,
            included_columns: Some(included_columns),
            entity_ref: None,
        }
    }

    pub fn with_entity_ref(mut self, entity_ref: EntityRef) -> Self {
        self.entity_ref = Some(entity_ref);
        self
    }

    pub fn with_entity_version(mut self, version: Timestamp) -> Self {
        self.entity_version = Some(version);
        self
    }

    pub fn is_visible_at(&self, read_ts: Timestamp) -> bool {
        self.created_ts <= read_ts
            && self
                .deleted_ts
                .is_none_or(|deleted_ts| deleted_ts > read_ts)
    }

    pub fn mark_deleted(&mut self, deleted_ts: Timestamp) {
        self.deleted_ts = Some(deleted_ts);
    }
}

impl Default for IndexRecord {
    fn default() -> Self {
        Self::new(MAX_TIMESTAMP)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GcStats {
    pub vertex_entries_removed: usize,
    pub edge_entries_removed: usize,
}

impl GcStats {
    pub fn total_removed(&self) -> usize {
        self.vertex_entries_removed + self.edge_entries_removed
    }

    pub fn is_empty(&self) -> bool {
        self.vertex_entries_removed == 0 && self.edge_entries_removed == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_record_is_visible_at_before_delete() {
        let record = IndexRecord::new(10);
        assert!(record.is_visible_at(15));
        assert!(record.is_visible_at(10));
        assert!(!record.is_visible_at(5));
    }

    #[test]
    fn index_record_is_visible_at_after_delete() {
        let mut record = IndexRecord::new(10);
        record.mark_deleted(20);
        assert!(record.is_visible_at(15));
        assert!(!record.is_visible_at(20));
        assert!(!record.is_visible_at(25));
    }

    #[test]
    fn index_record_mark_deleted_sets_deleted_ts() {
        let mut record = IndexRecord::new(10);
        assert!(record.deleted_ts.is_none());
        record.mark_deleted(20);
        assert_eq!(record.deleted_ts, Some(20));
    }

    #[test]
    fn index_record_with_entity_ref_and_version() {
        let entity = EntityRef::Vertex(Default::default());
        let record = IndexRecord::new(10)
            .with_entity_ref(entity.clone())
            .with_entity_version(42);
        assert_eq!(record.entity_ref, Some(entity));
        assert_eq!(record.entity_version, Some(42));
    }

    #[test]
    fn index_record_default_uses_max_timestamp() {
        let record = IndexRecord::default();
        assert_eq!(record.created_ts, MAX_TIMESTAMP);
        assert!(record.deleted_ts.is_none());
    }

    #[test]
    fn index_record_new_with_columns() {
        let columns = vec![("name".to_string(), Value::string("test"))];
        let record = IndexRecord::new_with_columns(10, columns.clone());
        assert_eq!(record.created_ts, 10);
        assert_eq!(record.included_columns, Some(columns));
    }

    #[test]
    fn gc_stats_total_removed() {
        let stats = GcStats {
            vertex_entries_removed: 3,
            edge_entries_removed: 5,
        };
        assert_eq!(stats.total_removed(), 8);
    }

    #[test]
    fn gc_stats_is_empty() {
        assert!(GcStats::default().is_empty());
        let stats = GcStats {
            vertex_entries_removed: 1,
            ..GcStats::default()
        };
        assert!(!stats.is_empty());
    }

    #[test]
    fn edge_identity_new() {
        let src = Value::Int(1);
        let dst = Value::Int(2);
        let edge = EdgeIdentity::new(1, &src, &dst, "KNOWS", 0);
        assert_eq!(edge.space_id, 1);
        assert_eq!(*edge.src, Value::Int(1));
        assert_eq!(*edge.dst, Value::Int(2));
        assert_eq!(edge.edge_type, "KNOWS");
        assert_eq!(edge.ranking, 0);
    }
}
