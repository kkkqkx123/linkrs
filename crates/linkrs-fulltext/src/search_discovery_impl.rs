//! SearchIndexDiscovery implementation for FulltextIndexManager.

use linkrs_core::types::search::IndexDescriptor;
use linkrs_core::types::search_discovery::SearchIndexDiscovery;

use crate::manager::FulltextIndexManager;

impl SearchIndexDiscovery for FulltextIndexManager {
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
                engine: m.engine_type.to_string(),
                last_updated: Some(m.last_updated),
            })
            .collect()
    }

    fn index_exists(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        self.has_index(space_id, tag_name, field_name)
    }

    fn get_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Option<IndexDescriptor> {
        self.get_metadata(space_id, tag_name, field_name)
            .map(|m| IndexDescriptor {
                space_id: m.space_id,
                tag_name: m.tag_name,
                field_name: m.field_name,
                index_name: Some(m.index_name),
                status: m.status.to_string(),
                doc_count: m.doc_count,
                engine: m.engine_type.to_string(),
                last_updated: Some(m.last_updated),
            })
    }
}
