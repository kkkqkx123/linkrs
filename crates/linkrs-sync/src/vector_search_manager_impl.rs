//! SearchIndexManager implementation for VectorIndexManager.

use linkrs_core::error::search::SearchErrorKind;
use linkrs_core::types::search::IndexDescriptor;
use linkrs_core::types::search_manager::{SearchIndexManager, SearchManagerResult};
use linkrs_core::types::IndexStats;
use std::future::Future;
use std::pin::Pin;

use crate::vector_index_manager::VectorIndexManager;
use crate::vector_sync::VectorIndexLocation;

impl SearchIndexManager for VectorIndexManager {
    fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<String>> + Send + '_>> {
        Box::pin(async move {
            self.create_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| e.classify())
        })
    }

    fn drop_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<()>> + Send + '_>> {
        Box::pin(async move {
            self.drop_vector_index(space_id, tag_name, field_name)
                .await
                .map_err(|e| e.classify())
        })
    }

    fn list_indexes(&self) -> Vec<IndexDescriptor> {
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

    fn get_stats(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<IndexStats>> + Send + '_>> {
        Box::pin(async move {
            let count = self
                .count(space_id, tag_name, field_name)
                .await
                .map_err(|e| e.classify())?;
            Ok(IndexStats::new(count as usize, 0))
        })
    }

    fn clear_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        force: bool,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<()>> + Send + '_>> {
        Box::pin(async move {
            if !force {
                return Err(SearchErrorKind::Internal);
            }
            self.purge_index_data(space_id, tag_name, field_name)
                .await
                .map_err(|e| e.classify())
        })
    }

    fn rebuild_index(
        &self,
        _space_id: u64,
        _tag_name: &str,
        _field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<u64>> + Send + '_>> {
        Box::pin(async move {
            // Vector rebuild requires a doc source and options that are
            // not part of the unified trait; this is handled by the sync
            // layer. Return a clear error if called directly.
            Err(SearchErrorKind::Internal)
        })
    }

    fn inconsistent_indexes(&self) -> Vec<IndexDescriptor> {
        self.inconsistent_vector_indexes()
            .into_iter()
            .map(|p| {
                let loc = VectorIndexLocation::new(p.space_id, &p.tag_name, &p.field_name);
                IndexDescriptor {
                    space_id: loc.space_id,
                    tag_name: loc.tag_name.to_string(),
                    field_name: loc.field_name.to_string(),
                    index_name: None,
                    status: p.phase.as_str().to_string(),
                    doc_count: 0,
                    engine: "vector".to_string(),
                    last_updated: None,
                }
            })
            .collect()
    }
}
