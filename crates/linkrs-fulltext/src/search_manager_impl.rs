//! SearchIndexManager implementation for FulltextIndexManager.

use linkrs_core::error::search::SearchErrorKind;
use linkrs_core::types::search::IndexDescriptor;
use linkrs_core::types::search_manager::{SearchIndexManager, SearchManagerResult};
use linkrs_core::types::IndexStats;
use std::future::Future;
use std::pin::Pin;

use crate::manager::FulltextIndexManager;

impl SearchIndexManager for FulltextIndexManager {
    fn create_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<String>> + Send + '_>> {
        Box::pin(async move {
            self.create_index(space_id, tag_name, field_name, None)
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
            self.drop_index(space_id, tag_name, field_name)
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
                engine: m.engine_type.to_string(),
                last_updated: Some(m.last_updated),
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
            self.get_stats(space_id, tag_name, field_name)
                .await
                .map_err(|e| e.classify())
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
            self.clear_index(space_id, tag_name, field_name, force)
                .await
                .map_err(|e| e.classify())
        })
    }

    fn rebuild_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Pin<Box<dyn Future<Output = SearchManagerResult<u64>> + Send + '_>> {
        Box::pin(async move {
            // Fulltext rebuild requires a doc source and options that are
            // not part of the unified trait; this is handled by the sync
            // layer. Return a clear error if called directly.
            Err(SearchErrorKind::Internal)
        })
    }

    fn inconsistent_indexes(&self) -> Vec<IndexDescriptor> {
        self.get_inconsistent_indexes()
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
}
