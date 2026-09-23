//! Primary-storage scan feeding fulltext rebuild backfill.
//!
//! Enumerates text documents for one `(tag, field)` index from both the
//! vertex tag scan and the edge type scan (a tag name and an edge type may
//! share a name, and the live path indexes both into the same fulltext
//! index). Only `Value::String` properties are yielded, mirroring the live
//! delivery path, which skips every other value type.

use async_trait::async_trait;
#[cfg(feature = "fulltext")]
use graphdb_core::Edge;
use graphdb_core::{Value, Vertex};
#[cfg(feature = "fulltext")]
use graphdb_sync::{RebuildDoc, RebuildDocSource, RebuildEntity};
#[cfg(feature = "vector")]
use graphdb_sync::{VectorDocSource, VectorRebuildDoc};

use crate::storage::{StorageError, StorageReader};

/// Batches text documents out of storage for rebuild backfill.
#[cfg(feature = "fulltext")]
pub struct StorageRebuildSource<'a, S: StorageReader + ?Sized> {
    storage: &'a S,
    space_name: String,
    tag_name: String,
    field_name: String,
    batch_size: usize,
    vertex_offset: usize,
    vertex_paginated: bool,
    vertex_cache: Vec<Vertex>,
    vertex_cursor: usize,
    vertices_done: bool,
    edge_offset: usize,
    edge_paginated: bool,
    edge_cache: Vec<Edge>,
    edge_cursor: usize,
    edges_done: bool,
}

#[cfg(feature = "fulltext")]
impl<'a, S: StorageReader + ?Sized> StorageRebuildSource<'a, S> {
    pub fn new(
        storage: &'a S,
        space_name: String,
        tag_name: String,
        field_name: String,
        batch_size: usize,
    ) -> Self {
        Self {
            storage,
            space_name,
            tag_name,
            field_name,
            batch_size: batch_size.max(1),
            vertex_offset: 0,
            vertex_paginated: true,
            vertex_cache: Vec::new(),
            vertex_cursor: 0,
            vertices_done: false,
            edge_offset: 0,
            edge_paginated: true,
            edge_cache: Vec::new(),
            edge_cursor: 0,
            edges_done: false,
        }
    }

    fn vertex_texts(&self, vertex: &Vertex) -> Vec<String> {
        // Single-label vertices carry exactly one tag; only its properties
        // are indexed.
        let mut texts = Vec::new();
        if vertex.tag.name == self.tag_name {
            Self::push_field_text(&mut texts, vertex.tag.properties.get(&self.field_name));
        }
        texts
    }

    fn push_field_text(texts: &mut Vec<String>, value: Option<&Value>) {
        if let Some(Value::String(text)) = value {
            texts.push(text.to_string());
        }
    }

    fn next_vertex_page(&mut self) -> Result<Vec<Vertex>, StorageError> {
        if self.vertex_paginated {
            match self.storage.scan_vertices_by_tag_paginated(
                &self.space_name,
                &self.tag_name,
                self.vertex_offset,
                self.batch_size,
            ) {
                Ok(page) => {
                    self.vertex_offset += page.len();
                    return Ok(page);
                }
                Err(_) => {
                    // Paginated scan unsupported: fall back to one full scan.
                    self.vertex_paginated = false;
                }
            }
        }
        if self.vertex_cache.is_empty() && self.vertex_cursor == 0 {
            self.vertex_cache = self
                .storage
                .scan_vertices_by_tag(&self.space_name, &self.tag_name)?;
        }
        let start = self.vertex_cursor;
        let end = (start + self.batch_size).min(self.vertex_cache.len());
        self.vertex_cursor = end;
        Ok(self.vertex_cache[start..end].to_vec())
    }

    fn next_edge_page(&mut self) -> Result<Vec<Edge>, StorageError> {
        if self.edge_paginated {
            match self.storage.scan_edges_by_type_paginated(
                &self.space_name,
                &self.tag_name,
                self.edge_offset,
                self.batch_size,
            ) {
                Ok(page) => {
                    self.edge_offset += page.len();
                    return Ok(page);
                }
                Err(_) => {
                    self.edge_paginated = false;
                }
            }
        }
        if self.edge_cache.is_empty() && self.edge_cursor == 0 {
            self.edge_cache = self
                .storage
                .scan_edges_by_type(&self.space_name, &self.tag_name)?;
        }
        let start = self.edge_cursor;
        let end = (start + self.batch_size).min(self.edge_cache.len());
        self.edge_cursor = end;
        Ok(self.edge_cache[start..end].to_vec())
    }
}

#[cfg(feature = "fulltext")]
#[async_trait]
impl<S: StorageReader + ?Sized + Send + Sync> RebuildDocSource for StorageRebuildSource<'_, S> {
    async fn next_batch(&mut self) -> Result<Option<Vec<RebuildDoc>>, String> {
        loop {
            if !self.vertices_done {
                let page = self.next_vertex_page().map_err(|e| e.to_string())?;
                if page.len() < self.batch_size {
                    self.vertices_done = true;
                }
                if page.is_empty() {
                    continue;
                }
                let mut docs = Vec::new();
                for vertex in &page {
                    for text in self.vertex_texts(vertex) {
                        docs.push(RebuildDoc {
                            entity: RebuildEntity::Vertex { id: vertex.vid },
                            text,
                        });
                    }
                }
                if docs.is_empty() {
                    continue;
                }
                return Ok(Some(docs));
            }
            if !self.edges_done {
                let page = self.next_edge_page().map_err(|e| e.to_string())?;
                if page.len() < self.batch_size {
                    self.edges_done = true;
                }
                if page.is_empty() {
                    continue;
                }
                let mut docs = Vec::new();
                for edge in &page {
                    if let Some(Value::String(text)) = edge.props.get(&self.field_name) {
                        docs.push(RebuildDoc {
                            entity: RebuildEntity::Edge {
                                src: edge.src,
                                dst: edge.dst,
                                edge_type: self.tag_name.clone(),
                                ranking: edge.ranking,
                            },
                            text: text.to_string(),
                        });
                    }
                }
                if docs.is_empty() {
                    continue;
                }
                return Ok(Some(docs));
            }
            return Ok(None);
        }
    }
}

/// Batches vertex snapshots out of storage for vector rebuild backfill.
///
/// Only vertices carrying a vector value for the rebuilt field are yielded,
/// mirroring the live delivery path, which only indexes `Value::Vector`
/// properties. Vectors are vertex-scoped, so no edge scan is needed.
#[cfg(feature = "vector")]
pub struct StorageVectorSource<'a, S: StorageReader + ?Sized> {
    storage: &'a S,
    space_name: String,
    tag_name: String,
    field_name: String,
    batch_size: usize,
    vertex_offset: usize,
    vertex_paginated: bool,
    vertex_cache: Vec<Vertex>,
    vertex_cursor: usize,
    vertices_done: bool,
}

#[cfg(feature = "vector")]
impl<'a, S: StorageReader + ?Sized> StorageVectorSource<'a, S> {
    pub fn new(
        storage: &'a S,
        space_name: String,
        tag_name: String,
        field_name: String,
        batch_size: usize,
    ) -> Self {
        Self {
            storage,
            space_name,
            tag_name,
            field_name,
            batch_size: batch_size.max(1),
            vertex_offset: 0,
            vertex_paginated: true,
            vertex_cache: Vec::new(),
            vertex_cursor: 0,
            vertices_done: false,
        }
    }

    fn vertex_properties(&self, vertex: &Vertex) -> Vec<(String, Value)> {
        // Single-label vertices carry exactly one tag; only its properties
        // are indexed.
        if vertex.tag.name != self.tag_name {
            return Vec::new();
        }
        vertex
            .tag
            .properties
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
    }

    fn next_vertex_page(&mut self) -> Result<Vec<Vertex>, StorageError> {
        if self.vertex_paginated {
            match self.storage.scan_vertices_by_tag_paginated(
                &self.space_name,
                &self.tag_name,
                self.vertex_offset,
                self.batch_size,
            ) {
                Ok(page) => {
                    self.vertex_offset += page.len();
                    return Ok(page);
                }
                Err(_) => {
                    // Paginated scan unsupported: fall back to one full scan.
                    self.vertex_paginated = false;
                }
            }
        }
        if self.vertex_cache.is_empty() && self.vertex_cursor == 0 {
            self.vertex_cache = self
                .storage
                .scan_vertices_by_tag(&self.space_name, &self.tag_name)?;
        }
        let start = self.vertex_cursor;
        let end = (start + self.batch_size).min(self.vertex_cache.len());
        self.vertex_cursor = end;
        Ok(self.vertex_cache[start..end].to_vec())
    }
}

#[cfg(feature = "vector")]
#[async_trait]
impl<S: StorageReader + ?Sized + Send + Sync> VectorDocSource for StorageVectorSource<'_, S> {
    async fn next_batch(&mut self) -> Result<Option<Vec<VectorRebuildDoc>>, String> {
        loop {
            if self.vertices_done {
                return Ok(None);
            }
            let page = self.next_vertex_page().map_err(|e| e.to_string())?;
            if page.len() < self.batch_size {
                self.vertices_done = true;
            }
            if page.is_empty() {
                continue;
            }
            let mut docs = Vec::new();
            for vertex in &page {
                let properties = self.vertex_properties(vertex);
                let has_vector = properties
                    .iter()
                    .any(|(name, value)| name == &self.field_name && value.as_vector().is_some());
                if !has_vector {
                    continue;
                }
                docs.push(VectorRebuildDoc {
                    vertex_id: vertex.vid,
                    properties,
                });
            }
            if docs.is_empty() {
                continue;
            }
            return Ok(Some(docs));
        }
    }
}
