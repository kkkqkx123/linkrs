use super::QueryPipelineManager;
use crate::core::error::{DBError, DBResult, QueryError};
use crate::core::metadata::SchemaManager;
use crate::query::metadata::MetadataContext;
use crate::query::validator::ValidatedStatement;
use crate::query::QueryContext;
#[cfg(feature = "fulltext-search")]
use crate::search::manager::FulltextIndexManager;
use crate::storage::QueryStorage;
#[cfg(feature = "qdrant")]
use crate::sync::vector_sync::VectorSyncCoordinator;
use std::sync::Arc;

impl<S: QueryStorage + 'static> QueryPipelineManager<S> {
    pub(crate) fn build_metadata_context(
        &self,
        validated: &ValidatedStatement,
        qctx: Arc<QueryContext>,
    ) -> DBResult<Option<MetadataContext>> {
        use crate::query::parser::ast::Stmt;

        let space_id = qctx.space_id().unwrap_or(0);
        let mut context = MetadataContext::new();
        let stmt = validated.stmt();
        let mut has_metadata = false;

        match stmt {
            Stmt::SearchVector(search) => {
                #[cfg(feature = "qdrant")]
                if let Some(ref vector_coordinator) = self.vector_coordinator {
                    match self.resolve_vector_index(
                        space_id,
                        &search.index_name,
                        vector_coordinator,
                    ) {
                        Ok(index_metadata) => {
                            context.set_index_metadata(search.index_name.clone(), index_metadata);
                            has_metadata = true;
                        }
                        Err(msg) => {
                            return Err(DBError::from(QueryError::invalid_query(format!(
                                "Vector index not found: {}",
                                msg
                            ))));
                        }
                    }
                } else {
                    return Err(DBError::from(QueryError::invalid_query(
                        "Vector search not enabled".to_string(),
                    )));
                }
                #[cfg(not(feature = "qdrant"))]
                let _ = search;
            }
            Stmt::LookupVector(lookup) => {
                #[cfg(feature = "qdrant")]
                if let Some(ref vector_coordinator) = self.vector_coordinator {
                    match self.resolve_vector_index(
                        space_id,
                        &lookup.index_name,
                        vector_coordinator,
                    ) {
                        Ok(index_metadata) => {
                            context.set_index_metadata(lookup.index_name.clone(), index_metadata);
                            has_metadata = true;
                        }
                        Err(msg) => {
                            return Err(DBError::from(QueryError::invalid_query(format!(
                                "Vector index not found: {}",
                                msg
                            ))));
                        }
                    }
                }
                #[cfg(not(feature = "qdrant"))]
                let _ = lookup;
            }
            Stmt::MatchVector(_) => {
                log::debug!("MatchVector metadata resolution deferred to executor");
            }
            Stmt::Search(search) => {
                #[cfg(feature = "fulltext-search")]
                if let Some(ref fulltext_manager) = self.fulltext_manager {
                    match self.resolve_fulltext_index(
                        space_id,
                        &search.index_name,
                        fulltext_manager,
                    ) {
                        Ok(index_metadata) => {
                            context.set_index_metadata(search.index_name.clone(), index_metadata);
                            has_metadata = true;
                        }
                        Err(msg) => {
                            return Err(DBError::from(QueryError::invalid_query(format!(
                                "Fulltext index not found: {}",
                                msg
                            ))));
                        }
                    }
                }
                #[cfg(not(feature = "fulltext-search"))]
                let _ = search;
            }
            Stmt::LookupFulltext(lookup) => {
                #[cfg(feature = "fulltext-search")]
                if let Some(ref fulltext_manager) = self.fulltext_manager {
                    match self.resolve_fulltext_index(
                        space_id,
                        &lookup.index_name,
                        fulltext_manager,
                    ) {
                        Ok(index_metadata) => {
                            context.set_index_metadata(lookup.index_name.clone(), index_metadata);
                            has_metadata = true;
                        }
                        Err(msg) => {
                            return Err(DBError::from(QueryError::invalid_query(format!(
                                "Fulltext index not found: {}",
                                msg
                            ))));
                        }
                    }
                }
                #[cfg(not(feature = "fulltext-search"))]
                let _ = lookup;
            }
            Stmt::MatchFulltext(match_stmt) => {
                #[cfg(feature = "fulltext-search")]
                if let Some(ref index_name) = match_stmt.fulltext_condition.index_name {
                    if let Some(ref fulltext_manager) = self.fulltext_manager {
                        match self.resolve_fulltext_index(space_id, index_name, fulltext_manager) {
                            Ok(index_metadata) => {
                                context.set_index_metadata(index_name.clone(), index_metadata);
                                has_metadata = true;
                            }
                            Err(msg) => {
                                return Err(DBError::from(QueryError::invalid_query(format!(
                                    "Fulltext index not found: {}",
                                    msg
                                ))));
                            }
                        }
                    }
                }
                #[cfg(not(feature = "fulltext-search"))]
                let _ = match_stmt;
            }
            Stmt::Match(_match_stmt) => {
                let referenced_tags = &validated.validation_info.semantic_info.referenced_tags;
                let referenced_edges = &validated.validation_info.semantic_info.referenced_edges;

                let referenced_set: std::collections::HashSet<&str> = referenced_tags
                    .iter()
                    .chain(referenced_edges.iter())
                    .map(String::as_str)
                    .collect();

                if let Some(ref schema_manager) = self.schema_manager {
                    for tag_name in referenced_tags {
                        match self.resolve_tag_metadata(space_id, tag_name, schema_manager) {
                            Ok(tag_metadata) => {
                                context.set_tag_metadata(tag_name.clone(), tag_metadata);
                                has_metadata = true;
                            }
                            Err(e) => {
                                return Err(DBError::from(QueryError::invalid_query(format!(
                                    "Tag '{}' not found: {}",
                                    tag_name, e
                                ))));
                            }
                        }
                    }

                    for edge_type in referenced_edges {
                        match self.resolve_edge_type_metadata(space_id, edge_type, schema_manager) {
                            Ok(edge_metadata) => {
                                context.set_edge_type_metadata(edge_type.clone(), edge_metadata);
                                has_metadata = true;
                            }
                            Err(e) => {
                                return Err(DBError::from(QueryError::invalid_query(format!(
                                    "Edge type '{}' not found: {}",
                                    edge_type, e
                                ))));
                            }
                        }
                    }
                }

                match self.resolve_all_indexes(space_id) {
                    Ok(indexes) => {
                        for index in indexes {
                            if referenced_set.contains(index.tag_name.as_str())
                                || (index.tag_name.is_empty() && !referenced_edges.is_empty())
                            {
                                context.set_index_metadata(index.index_name.clone(), index);
                                has_metadata = true;
                            }
                        }
                    }
                    Err(e) => {
                        log::warn!("Failed to resolve indexes for space {}: {}", space_id, e);
                    }
                }
            }
            Stmt::CreateFulltextIndex(create) => {
                #[cfg(feature = "fulltext-search")]
                if !create.schema_name.is_empty() {
                    if let Some(ref schema_manager) = self.schema_manager {
                        match self.resolve_tag_metadata(
                            space_id,
                            &create.schema_name,
                            schema_manager,
                        ) {
                            Ok(tag_metadata) => {
                                context.set_tag_metadata(create.schema_name.clone(), tag_metadata);
                                has_metadata = true;
                            }
                            Err(e) => {
                                return Err(DBError::from(QueryError::invalid_query(format!(
                                    "Tag '{}' not found: {}",
                                    create.schema_name, e
                                ))));
                            }
                        }
                    }
                }
                #[cfg(not(feature = "fulltext-search"))]
                let _ = create;
            }
            _ => {
                log::debug!("No metadata resolution for statement type: {:?}", stmt);
            }
        }

        if has_metadata {
            Ok(Some(context))
        } else {
            Ok(None)
        }
    }

    fn resolve_tag_metadata(
        &self,
        space_id: u64,
        tag_name: &str,
        schema_manager: &SchemaManager,
    ) -> Result<crate::query::metadata::TagMetadata, String> {
        use crate::query::metadata::{PropertyDefinition, PropertyType};

        let space = schema_manager
            .get_space_by_id(space_id)
            .map_err(|e| format!("Failed to get space {}: {}", space_id, e))?
            .ok_or_else(|| format!("Space {} not found", space_id))?;

        let tag_info = schema_manager
            .get_tag(&space.space_name, tag_name)
            .map_err(|e| format!("Failed to get tag '{}': {}", tag_name, e))?
            .ok_or_else(|| format!("Tag '{}' not found in space {}", tag_name, space_id))?;

        let mut metadata =
            crate::query::metadata::TagMetadata::new(tag_info.tag_name.clone(), space_id);
        metadata.properties = tag_info
            .properties
            .iter()
            .map(|prop| PropertyDefinition {
                name: prop.name.clone(),
                data_type: PropertyType::from(prop.data_type.clone()),
                nullable: prop.nullable,
                default_value: None,
            })
            .collect();

        Ok(metadata)
    }

    fn resolve_edge_type_metadata(
        &self,
        space_id: u64,
        edge_type: &str,
        schema_manager: &SchemaManager,
    ) -> Result<crate::query::metadata::EdgeTypeMetadata, String> {
        use crate::query::metadata::{PropertyDefinition, PropertyType};

        let space = schema_manager
            .get_space_by_id(space_id)
            .map_err(|e| format!("Failed to get space {}: {}", space_id, e))?
            .ok_or_else(|| format!("Space {} not found", space_id))?;

        let edge_info = schema_manager
            .get_edge_type(&space.space_name, edge_type)
            .map_err(|e| format!("Failed to get edge type '{}': {}", edge_type, e))?
            .ok_or_else(|| format!("Edge type '{}' not found in space {}", edge_type, space_id))?;

        let mut metadata = crate::query::metadata::EdgeTypeMetadata::new(
            edge_info.edge_type_name.clone(),
            space_id,
        );
        metadata.properties = edge_info
            .properties
            .iter()
            .map(|prop| PropertyDefinition {
                name: prop.name.clone(),
                data_type: PropertyType::from(prop.data_type.clone()),
                nullable: prop.nullable,
                default_value: None,
            })
            .collect();

        Ok(metadata)
    }

    #[cfg(feature = "fulltext-search")]
    fn resolve_fulltext_index(
        &self,
        space_id: u64,
        index_name: &str,
        fulltext_manager: &FulltextIndexManager,
    ) -> Result<crate::query::metadata::IndexMetadata, String> {
        let indexes = fulltext_manager.list_indexes();
        for index in &indexes {
            if index.space_id == space_id && index.index_name == index_name {
                return Ok(crate::query::metadata::IndexMetadata::new(
                    index.index_name.clone(),
                    space_id,
                    index.tag_name.clone(),
                    index.field_name.clone(),
                    crate::query::metadata::IndexType::Fulltext,
                ));
            }
        }
        Err(format!(
            "Fulltext index '{}' not found in space {}",
            index_name, space_id
        ))
    }

    #[cfg(feature = "qdrant")]
    fn resolve_vector_index(
        &self,
        space_id: u64,
        index_name: &str,
        vector_coordinator: &VectorSyncCoordinator,
    ) -> Result<crate::query::metadata::IndexMetadata, String> {
        let indexes = vector_coordinator.list_indexes();
        for idx in &indexes {
            let expected_collection =
                format!("space_{}_{}_{}", space_id, idx.tag_name, idx.field_name);
            if idx.collection_name == index_name
                || expected_collection == *index_name
                || idx.index_name.as_deref() == Some(index_name)
            {
                return Ok(crate::query::metadata::IndexMetadata::new(
                    idx.collection_name.clone(),
                    space_id,
                    idx.tag_name.clone(),
                    idx.field_name.clone(),
                    crate::query::metadata::IndexType::Vector,
                ));
            }
        }
        Err(format!(
            "Vector index '{}' not found in space {}",
            index_name, space_id
        ))
    }

    fn resolve_all_indexes(
        &self,
        space_id: u64,
    ) -> Result<Vec<crate::query::metadata::IndexMetadata>, String> {
        use crate::query::metadata::IndexType;

        let mut indexes = Vec::new();
        let mut seen = std::collections::HashSet::new();

        if let Some(ref index_manager) = self.index_manager {
            if let Ok(tag_indexes) = index_manager.list_tag_indexes(space_id) {
                for index in tag_indexes {
                    if seen.insert(index.name.clone()) {
                        indexes.push(crate::query::metadata::IndexMetadata::new(
                            index.name,
                            space_id,
                            index.schema_name,
                            index
                                .fields
                                .first()
                                .map(|f| f.name.clone())
                                .unwrap_or_default(),
                            IndexType::Native,
                        ));
                    }
                }
            }
            if let Ok(edge_indexes) = index_manager.list_edge_indexes(space_id) {
                for index in edge_indexes {
                    if seen.insert(index.name.clone()) {
                        indexes.push(crate::query::metadata::IndexMetadata::new(
                            index.name,
                            space_id,
                            String::new(),
                            index
                                .fields
                                .first()
                                .map(|f| f.name.clone())
                                .unwrap_or_default(),
                            IndexType::Native,
                        ));
                    }
                }
            }
        }

        #[cfg(feature = "fulltext-search")]
        if let Some(ref ft) = self.fulltext_manager {
            for idx in ft.list_indexes() {
                if idx.space_id == space_id && seen.insert(idx.index_name.clone()) {
                    indexes.push(crate::query::metadata::IndexMetadata::new(
                        idx.index_name,
                        space_id,
                        idx.tag_name,
                        idx.field_name,
                        IndexType::Fulltext,
                    ));
                }
            }
        }

        #[cfg(feature = "qdrant")]
        if let Some(ref vec) = self.vector_coordinator {
            for idx in vec.list_indexes() {
                if idx.space_id == space_id && seen.insert(idx.collection_name.clone()) {
                    indexes.push(crate::query::metadata::IndexMetadata::new(
                        idx.collection_name.clone(),
                        space_id,
                        idx.tag_name.clone(),
                        idx.field_name.clone(),
                        IndexType::Vector,
                    ));
                }
            }
        }

        Ok(indexes)
    }
}
