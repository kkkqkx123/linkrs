//! SearchIndexDiscovery implementation for VectorIndexManager.

use linkrs_core::types::search::IndexDescriptor;
use linkrs_core::types::search_discovery::SearchIndexDiscovery;

use crate::vector_index_manager::VectorIndexManager;
use crate::vector_sync::VectorIndexLocation;

impl SearchIndexDiscovery for VectorIndexManager {
    fn discover_indexes(&self) -> Vec<IndexDescriptor> {
        self.list_indexes()
            .into_iter()
            .map(|m| IndexDescriptor {
                space_id: m.space_id,
                tag_name: m.tag_name,
                field_name: m.field_name,
                index_name: Some(m.index_name),
                status: m.status.to_string(),
                doc_count: m.doc_count,
                engine: "vector".to_string(),
                last_updated: m.last_updated,
            })
            .collect()
    }

    fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.contains_key(&loc)
    }

    fn get_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexDescriptor> {
        let loc = VectorIndexLocation::new(space_id, tag_name, field_name);
        self.logical_indexes.get(&loc).map(|m| IndexDescriptor {
            space_id: m.space_id,
            tag_name: m.tag_name.to_string(),
            field_name: m.field_name.to_string(),
            index_name: Some(m.index_name.clone()),
            status: m.status.to_string(),
            doc_count: m.doc_count,
            engine: "vector".to_string(),
            last_updated: m.last_updated,
        })
    }
}
