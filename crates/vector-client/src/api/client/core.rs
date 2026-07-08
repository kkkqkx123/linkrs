use crate::engine::VectorEngine;
use crate::error::Result;
use crate::types::*;

pub type CollectionApiDyn<'a> = CollectionApi<'a, dyn VectorEngine>;
pub type PointApiDyn<'a> = PointApi<'a, dyn VectorEngine>;
pub type SearchApiDyn<'a> = SearchApi<'a, dyn VectorEngine>;

pub struct CollectionApi<'a, E: VectorEngine + ?Sized> {
    engine: &'a E,
}

impl<'a, E: VectorEngine + ?Sized> CollectionApi<'a, E> {
    pub fn new(engine: &'a E) -> Self {
        Self { engine }
    }

    pub async fn create(&self, name: &str, config: CollectionConfig) -> Result<()> {
        self.engine.create_collection(name, config).await
    }

    pub async fn delete(&self, name: &str) -> Result<()> {
        self.engine.delete_collection(name).await
    }

    pub async fn exists(&self, name: &str) -> Result<bool> {
        self.engine.collection_exists(name).await
    }

    pub async fn info(&self, name: &str) -> Result<CollectionInfo> {
        self.engine.collection_info(name).await
    }

    pub async fn count(&self, name: &str) -> Result<u64> {
        self.engine.count(name).await
    }
}

pub struct PointApi<'a, E: VectorEngine + ?Sized> {
    engine: &'a E,
    collection: String,
}

impl<'a, E: VectorEngine + ?Sized> PointApi<'a, E> {
    pub fn new(engine: &'a E, collection: impl Into<String>) -> Self {
        Self {
            engine,
            collection: collection.into(),
        }
    }

    pub async fn upsert(&self, point: VectorPoint) -> Result<UpsertResult> {
        self.engine.upsert(&self.collection, point).await
    }

    pub async fn upsert_batch(&self, points: Vec<VectorPoint>) -> Result<UpsertResult> {
        self.engine.upsert_batch(&self.collection, points).await
    }

    pub async fn get(&self, point_id: &str) -> Result<Option<VectorPoint>> {
        self.engine.get(&self.collection, point_id).await
    }

    pub async fn get_batch(&self, point_ids: Vec<&str>) -> Result<Vec<Option<VectorPoint>>> {
        self.engine.get_batch(&self.collection, point_ids).await
    }

    pub async fn delete(&self, point_id: &str) -> Result<DeleteResult> {
        self.engine.delete(&self.collection, point_id).await
    }

    pub async fn delete_batch(&self, point_ids: Vec<&str>) -> Result<DeleteResult> {
        self.engine.delete_batch(&self.collection, point_ids).await
    }

    pub async fn delete_by_filter(&self, filter: VectorFilter) -> Result<DeleteResult> {
        self.engine.delete_by_filter(&self.collection, filter).await
    }

    pub async fn set_payload(&self, point_ids: Vec<&str>, payload: Payload) -> Result<()> {
        self.engine
            .set_payload(&self.collection, point_ids, payload)
            .await
    }

    pub async fn delete_payload(&self, point_ids: Vec<&str>, keys: Vec<&str>) -> Result<()> {
        self.engine
            .delete_payload(&self.collection, point_ids, keys)
            .await
    }

    pub async fn scroll(
        &self,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> Result<(Vec<VectorPoint>, Option<String>)> {
        self.engine
            .scroll(&self.collection, limit, offset, with_payload, with_vector)
            .await
    }

    pub async fn create_payload_index(&self, field: &str, schema: PayloadSchemaType) -> Result<()> {
        self.engine
            .create_payload_index(&self.collection, field, schema)
            .await
    }

    pub async fn delete_payload_index(&self, field: &str) -> Result<()> {
        self.engine
            .delete_payload_index(&self.collection, field)
            .await
    }

    pub async fn list_payload_indexes(&self) -> Result<Vec<(String, PayloadSchemaType)>> {
        self.engine.list_payload_indexes(&self.collection).await
    }
}

pub struct SearchApi<'a, E: VectorEngine + ?Sized> {
    engine: &'a E,
    collection: String,
}

impl<'a, E: VectorEngine + ?Sized> SearchApi<'a, E> {
    pub fn new(engine: &'a E, collection: impl Into<String>) -> Self {
        Self {
            engine,
            collection: collection.into(),
        }
    }

    pub async fn search(&self, query: SearchQuery) -> Result<Vec<SearchResult>> {
        self.engine.search(&self.collection, query).await
    }

    pub async fn search_batch(&self, queries: Vec<SearchQuery>) -> Result<Vec<Vec<SearchResult>>> {
        self.engine.search_batch(&self.collection, queries).await
    }
}
