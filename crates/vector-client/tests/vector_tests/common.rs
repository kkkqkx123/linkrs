//! Common test utilities for vector tests
//!
//! Provides mock implementations and test context for vector search testing.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::RwLock;
use tempfile::TempDir;
use vector_client::embedding::{EmbeddingError, EmbeddingProvider};
use vector_client::engine::VectorEngine;
use vector_client::error::{Result, VectorClientError};
use vector_client::manager::IndexMetadata;
use vector_client::types::*;

const MOCK_ENGINE_VERSION: &str = "1.0.0-mock";

/// Mock Vector Engine for testing
///
/// Implements VectorEngine trait with in-memory storage
pub struct MockVectorEngine {
    collections: RwLock<HashMap<String, MockCollection>>,
    default_dimension: usize,
}

#[derive(Debug, Clone)]
struct MockCollection {
    config: CollectionConfig,
    points: HashMap<String, VectorPoint>,
    payload_indexes: Vec<(String, PayloadSchemaType)>,
}

impl std::fmt::Debug for MockVectorEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockVectorEngine")
            .field(
                "collections",
                &self.collections.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .field("default_dimension", &self.default_dimension)
            .finish()
    }
}

impl MockVectorEngine {
    pub fn new(default_dimension: usize) -> Self {
        Self {
            collections: RwLock::new(HashMap::new()),
            default_dimension,
        }
    }
}

#[async_trait]
impl VectorEngine for MockVectorEngine {
    fn name(&self) -> &str {
        "mock"
    }

    fn version(&self) -> &str {
        MOCK_ENGINE_VERSION
    }

    async fn health_check(&self) -> Result<HealthStatus> {
        Ok(HealthStatus::healthy(self.name(), self.version()))
    }

    async fn create_collection(&self, name: &str, config: CollectionConfig) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        if collections.contains_key(name) {
            return Err(VectorClientError::CollectionAlreadyExists(name.to_string()));
        }
        collections.insert(
            name.to_string(),
            MockCollection {
                config,
                points: HashMap::new(),
                payload_indexes: Vec::new(),
            },
        );
        Ok(())
    }

    async fn delete_collection(&self, name: &str) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        collections.remove(name);
        Ok(())
    }

    async fn collection_exists(&self, name: &str) -> Result<bool> {
        let collections = self.collections.read().unwrap();
        Ok(collections.contains_key(name))
    }

    async fn collection_info(&self, name: &str) -> Result<CollectionInfo> {
        let collections = self.collections.read().unwrap();
        let collection = collections
            .get(name)
            .ok_or_else(|| VectorClientError::CollectionNotFound(name.to_string()))?;
        Ok(CollectionInfo {
            name: name.to_string(),
            vector_count: collection.points.len() as u64,
            indexed_vector_count: collection.points.len() as u64,
            points_count: collection.points.len() as u64,
            segments_count: 1,
            config: collection.config.clone(),
            status: CollectionStatus::Green,
        })
    }

    async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<UpsertResult> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let expected_dim = col.config.vector_size;
        if point.vector.len() != expected_dim {
            return Err(VectorClientError::InvalidVectorDimension {
                expected: expected_dim,
                actual: point.vector.len(),
            });
        }

        col.points.insert(point.id.to_string(), point);
        Ok(UpsertResult {
            operation_id: None,
            status: UpsertStatus::Completed,
        })
    }

    async fn upsert_batch(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
    ) -> Result<UpsertResult> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let expected_dim = col.config.vector_size;
        for point in &points {
            if point.vector.len() != expected_dim {
                return Err(VectorClientError::InvalidVectorDimension {
                    expected: expected_dim,
                    actual: point.vector.len(),
                });
            }
        }

        for point in points {
            col.points.insert(point.id.to_string(), point);
        }
        Ok(UpsertResult {
            operation_id: None,
            status: UpsertStatus::Completed,
        })
    }

    async fn delete(&self, collection: &str, point_id: &str) -> Result<DeleteResult> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let removed = col.points.remove(point_id).is_some();
        Ok(DeleteResult {
            operation_id: None,
            deleted_count: if removed { 1 } else { 0 },
        })
    }

    async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<DeleteResult> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let mut deleted_count = 0u64;
        for id in point_ids {
            if col.points.remove(id).is_some() {
                deleted_count += 1;
            }
        }
        Ok(DeleteResult {
            operation_id: None,
            deleted_count,
        })
    }

    async fn delete_by_filter(
        &self,
        collection: &str,
        filter: VectorFilter,
    ) -> Result<DeleteResult> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let to_delete: Vec<String> = col
            .points
            .values()
            .filter(|point| matches_filter(&Some(filter.clone()), point))
            .map(|point| point.id.to_string())
            .collect();
        let deleted_count = to_delete.len() as u64;
        for id in &to_delete {
            col.points.remove(id);
        }
        Ok(DeleteResult {
            operation_id: None,
            deleted_count,
        })
    }

    async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let mut results: Vec<SearchResult> = col
            .points
            .values()
            .filter_map(|point| {
                if !matches_filter(&query.filter, point) {
                    return None;
                }

                let score = compute_similarity(&query.vector, &point.vector, col.config.distance);
                if let Some(threshold) = query.score_threshold {
                    if score < threshold {
                        return None;
                    }
                }
                Some(SearchResult {
                    id: point.id.clone(),
                    score,
                    payload: if query.with_payload.unwrap_or(true) {
                        point.payload.clone()
                    } else {
                        None
                    },
                    vector: if query.with_vector.unwrap_or(false) {
                        Some(point.vector.clone())
                    } else {
                        None
                    },
                })
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        match &query.search_mode {
            Some(SearchMode::Range {
                radius,
                max_results,
            }) => {
                results.retain(|r| r.score >= *radius);
                if let Some(max) = max_results {
                    results.truncate(*max);
                }
            }
            Some(SearchMode::KNN { k, .. }) => {
                results.truncate(*k);
            }
            _ => {}
        }

        let offset = query.offset.unwrap_or(0);
        let limit = query.limit;

        results = results.into_iter().skip(offset).take(limit).collect();
        Ok(results)
    }

    async fn search_batch(
        &self,
        collection: &str,
        queries: Vec<SearchQuery>,
    ) -> Result<Vec<Vec<SearchResult>>> {
        let mut results = Vec::with_capacity(queries.len());
        for query in queries {
            results.push(self.search(collection, query).await?);
        }
        Ok(results)
    }

    async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        Ok(col.points.get(point_id).cloned())
    }

    async fn get_batch(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
    ) -> Result<Vec<Option<VectorPoint>>> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        Ok(point_ids
            .into_iter()
            .map(|id| col.points.get(id).cloned())
            .collect())
    }

    async fn count(&self, collection: &str) -> Result<u64> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        Ok(col.points.len() as u64)
    }

    async fn set_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        payload: Payload,
    ) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        for id in point_ids {
            if let Some(point) = col.points.get_mut(id) {
                let existing = point.payload.get_or_insert_with(HashMap::new);
                for (k, v) in payload.clone() {
                    existing.insert(k, v);
                }
            }
        }
        Ok(())
    }

    async fn delete_payload(
        &self,
        collection: &str,
        point_ids: Vec<&str>,
        keys: Vec<&str>,
    ) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        for id in point_ids {
            if let Some(point) = col.points.get_mut(id) {
                if let Some(ref mut payload) = point.payload {
                    for key in &keys {
                        payload.remove(*key);
                    }
                }
            }
        }
        Ok(())
    }

    async fn scroll(
        &self,
        collection: &str,
        limit: usize,
        offset: Option<&str>,
        with_payload: Option<bool>,
        with_vector: Option<bool>,
    ) -> Result<(Vec<VectorPoint>, Option<String>)> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;

        let mut points: Vec<VectorPoint> = col
            .points
            .values()
            .map(|p| {
                let mut point = p.clone();
                if !with_payload.unwrap_or(true) {
                    point.payload = None;
                }
                if !with_vector.unwrap_or(false) {
                    point.vector = vec![];
                }
                point
            })
            .collect();

        points.sort_by_key(|a| a.id.to_string());

        let skip = if let Some(offset_id) = offset {
            points
                .iter()
                .position(|p| p.id.to_string() == offset_id)
                .map(|i| i + 1)
                .unwrap_or(0)
        } else {
            0
        };

        let result: Vec<VectorPoint> = points.into_iter().skip(skip).take(limit).collect();
        let next_offset = if result.len() == limit {
            result.last().map(|p| p.id.to_string())
        } else {
            None
        };

        Ok((result, next_offset))
    }

    async fn create_payload_index(
        &self,
        collection: &str,
        field: &str,
        schema: PayloadSchemaType,
    ) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        if !col.payload_indexes.iter().any(|(f, _)| f == field) {
            col.payload_indexes.push((field.to_string(), schema));
        }
        Ok(())
    }

    async fn delete_payload_index(&self, collection: &str, field: &str) -> Result<()> {
        let mut collections = self.collections.write().unwrap();
        let col = collections
            .get_mut(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        col.payload_indexes.retain(|(f, _)| f != field);
        Ok(())
    }

    async fn list_payload_indexes(
        &self,
        collection: &str,
    ) -> Result<Vec<(String, PayloadSchemaType)>> {
        let collections = self.collections.read().unwrap();
        let col = collections
            .get(collection)
            .ok_or_else(|| VectorClientError::CollectionNotFound(collection.to_string()))?;
        Ok(col.payload_indexes.clone())
    }
}

fn compute_similarity(query: &[f32], vector: &[f32], metric: DistanceMetric) -> f32 {
    match metric {
        DistanceMetric::Cosine => cosine_similarity(query, vector),
        DistanceMetric::Euclid => {
            let dist = euclidean_distance(query, vector);
            1.0 / (1.0 + dist)
        }
        DistanceMetric::Dot => dot_product(query, vector),
        DistanceMetric::Manhattan => {
            let dist = manhattan_distance(query, vector);
            1.0 / (1.0 + dist)
        }
    }
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot = dot_product(a, b);
    let norm_a = (a.iter().map(|x| x * x).sum::<f32>()).sqrt();
    let norm_b = (b.iter().map(|x| x * x).sum::<f32>()).sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

fn dot_product(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

fn euclidean_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn manhattan_distance(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum()
}

fn matches_filter(filter: &Option<VectorFilter>, point: &VectorPoint) -> bool {
    let Some(filter) = filter else {
        return true;
    };

    if let Some(ref must) = filter.must {
        for condition in must {
            if !matches_condition(condition, point) {
                return false;
            }
        }
    }

    if let Some(ref must_not) = filter.must_not {
        for condition in must_not {
            if matches_condition(condition, point) {
                return false;
            }
        }
    }

    if let Some(ref should) = filter.should {
        let min_count = filter.min_should.as_ref().map(|m| m.min_count).unwrap_or(1);
        let match_count = should
            .iter()
            .filter(|c| matches_condition(c, point))
            .count();
        if match_count < min_count {
            return false;
        }
    }

    true
}

fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let d_lat = (lat2 - lat1).to_radians();
    let d_lon = (lon2 - lon1).to_radians();
    let lat1 = lat1.to_radians();
    let lat2 = lat2.to_radians();
    let a = (d_lat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (d_lon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();
    const R: f64 = 6371.0;
    R * c
}

fn matches_condition(condition: &FilterCondition, point: &VectorPoint) -> bool {
    if let ConditionType::HasId { ids } = &condition.condition {
        return ids.contains(&point.id.to_string());
    }
    if let ConditionType::IsEmpty = &condition.condition {
        return match &point.payload {
            None => true,
            Some(payload) => match payload.get(&condition.field) {
                None => true,
                Some(v) => v.is_null(),
            },
        };
    }
    if let ConditionType::IsNull = &condition.condition {
        return match &point.payload {
            None => true,
            Some(payload) => payload.get(&condition.field).is_none(),
        };
    }

    let Some(ref payload) = point.payload else {
        return false;
    };

    match &condition.condition {
        ConditionType::Match { value } => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let Some(str_val) = field_value.as_str() {
                    return str_val == value;
                }
                return &field_value.to_string() == value;
            }
            false
        }
        ConditionType::MatchAny { values } => {
            if let Some(field_value) = payload.get(&condition.field) {
                return values.iter().any(|v| v == field_value);
            }
            false
        }
        ConditionType::Range(range) => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let Some(num) = field_value.as_f64() {
                    if let Some(gt) = range.gt {
                        if num <= gt {
                            return false;
                        }
                    }
                    if let Some(gte) = range.gte {
                        if num < gte {
                            return false;
                        }
                    }
                    if let Some(lt) = range.lt {
                        if num >= lt {
                            return false;
                        }
                    }
                    if let Some(lte) = range.lte {
                        if num > lte {
                            return false;
                        }
                    }
                    return true;
                }
            }
            false
        }
        ConditionType::IsEmpty | ConditionType::IsNull => unreachable!(),
        ConditionType::HasId { .. } => unreachable!(),
        ConditionType::Contains { value } => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let Some(arr) = field_value.as_array() {
                    return arr
                        .iter()
                        .any(|v| v.as_str().map(|s| s == value).unwrap_or(false));
                }
            }
            false
        }
        ConditionType::Nested { filter } => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let Some(obj) = field_value.as_object() {
                    let nested_payload: Payload =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    let nested_point = VectorPoint {
                        id: point.id.clone(),
                        vector: point.vector.clone(),
                        payload: Some(nested_payload),
                    };
                    return matches_filter(&Some(*filter.clone()), &nested_point);
                }
            }
            false
        }
        ConditionType::GeoRadius(geo) => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let (Some(lat), Some(lon)) = (
                    field_value.get("lat").and_then(|v| v.as_f64()),
                    field_value.get("lon").and_then(|v| v.as_f64()),
                ) {
                    let distance = haversine_distance(geo.center.lat, geo.center.lon, lat, lon);
                    return distance <= geo.radius;
                }
            }
            false
        }
        ConditionType::GeoBoundingBox(geo) => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let (Some(lat), Some(lon)) = (
                    field_value.get("lat").and_then(|v| v.as_f64()),
                    field_value.get("lon").and_then(|v| v.as_f64()),
                ) {
                    return lat >= geo.bottom_right.lat
                        && lat <= geo.top_left.lat
                        && lon >= geo.top_left.lon
                        && lon <= geo.bottom_right.lon;
                }
            }
            false
        }
        ConditionType::ValuesCount(vc) => {
            if let Some(field_value) = payload.get(&condition.field) {
                if let Some(arr) = field_value.as_array() {
                    let count = arr.len() as u64;
                    if let Some(gt) = vc.gt {
                        if count <= gt {
                            return false;
                        }
                    }
                    if let Some(gte) = vc.gte {
                        if count < gte {
                            return false;
                        }
                    }
                    if let Some(lt) = vc.lt {
                        if count >= lt {
                            return false;
                        }
                    }
                    if let Some(lte) = vc.lte {
                        if count > lte {
                            return false;
                        }
                    }
                    return true;
                }
            }
            false
        }
    }
}

/// Mock Embedding Provider for testing
pub struct MockEmbeddingProvider {
    dimension: usize,
    model_name: String,
}

impl std::fmt::Debug for MockEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockEmbeddingProvider")
            .field("dimension", &self.dimension)
            .field("model_name", &self.model_name)
            .finish()
    }
}

impl MockEmbeddingProvider {
    pub fn new(dimension: usize) -> Self {
        Self {
            dimension,
            model_name: "mock-embedding-model".to_string(),
        }
    }

    fn generate_embedding(&self, text: &str) -> Vec<f32> {
        let mut embedding = vec![0.0f32; self.dimension];
        let bytes = text.as_bytes();
        for (i, byte) in bytes.iter().enumerate() {
            let idx = i % self.dimension;
            embedding[idx] += (*byte as f32) / 255.0;
        }
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in embedding.iter_mut() {
                *val /= norm;
            }
        }
        embedding
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbeddingProvider {
    async fn embed(&self, texts: &[&str]) -> std::result::Result<Vec<Vec<f32>>, EmbeddingError> {
        Ok(texts.iter().map(|t| self.generate_embedding(t)).collect())
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    fn model_name(&self) -> &str {
        &self.model_name
    }
}

/// Mock Vector Manager for testing
pub struct MockVectorManager {
    engine: Arc<dyn VectorEngine>,
    indexes: DashMap<String, IndexMetadata>,
}

impl std::fmt::Debug for MockVectorManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockVectorManager")
            .field("engine", &self.engine.name())
            .field("index_count", &self.indexes.len())
            .finish()
    }
}

impl MockVectorManager {
    pub fn new(engine: Arc<dyn VectorEngine>) -> Self {
        Self {
            engine,
            indexes: DashMap::new(),
        }
    }

    pub fn engine(&self) -> &Arc<dyn VectorEngine> {
        &self.engine
    }

    pub async fn create_index(&self, name: &str, config: CollectionConfig) -> Result<()> {
        if self.indexes.contains_key(name) {
            return Err(VectorClientError::IndexAlreadyExists(name.to_string()));
        }
        self.engine.create_collection(name, config.clone()).await?;
        let metadata = IndexMetadata::new(name.to_string(), config);
        self.indexes.insert(name.to_string(), metadata);
        Ok(())
    }

    pub async fn drop_index(&self, name: &str) -> Result<()> {
        if let Some((_, _)) = self.indexes.remove(name) {
            self.engine.delete_collection(name).await?;
        }
        Ok(())
    }

    pub fn index_exists(&self, name: &str) -> bool {
        self.indexes.contains_key(name)
    }

    pub fn get_index_metadata(&self, name: &str) -> Option<IndexMetadata> {
        self.indexes.get(name).map(|m| m.clone())
    }

    pub fn list_indexes(&self) -> Vec<IndexMetadata> {
        self.indexes.iter().map(|m| m.value().clone()).collect()
    }

    pub async fn upsert(&self, collection: &str, point: VectorPoint) -> Result<()> {
        self.engine.upsert(collection, point).await?;
        Ok(())
    }

    pub async fn upsert_batch(&self, collection: &str, points: Vec<VectorPoint>) -> Result<()> {
        self.engine.upsert_batch(collection, points).await?;
        Ok(())
    }

    pub async fn delete(&self, collection: &str, point_id: &str) -> Result<()> {
        self.engine.delete(collection, point_id).await?;
        Ok(())
    }

    pub async fn delete_batch(&self, collection: &str, point_ids: Vec<&str>) -> Result<()> {
        self.engine.delete_batch(collection, point_ids).await?;
        Ok(())
    }

    pub async fn search(&self, collection: &str, query: SearchQuery) -> Result<Vec<SearchResult>> {
        self.engine.search(collection, query).await
    }

    pub async fn get(&self, collection: &str, point_id: &str) -> Result<Option<VectorPoint>> {
        self.engine.get(collection, point_id).await
    }

    pub async fn count(&self, collection: &str) -> Result<u64> {
        self.engine.count(collection).await
    }
}

/// Vector Test Context with mock engine support
pub struct VectorTestContext {
    pub manager: Arc<MockVectorManager>,
    pub embedding_provider: Arc<MockEmbeddingProvider>,
    #[allow(dead_code)]
    pub temp_dir: TempDir,
    pub default_dimension: usize,
}

impl VectorTestContext {
    pub fn new() -> Self {
        Self::with_dimension(128)
    }

    pub fn with_dimension(dimension: usize) -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let engine = Arc::new(MockVectorEngine::new(dimension));
        let manager = Arc::new(MockVectorManager::new(engine));
        let embedding_provider = Arc::new(MockEmbeddingProvider::new(dimension));
        Self {
            manager,
            embedding_provider,
            temp_dir,
            default_dimension: dimension,
        }
    }

    pub async fn create_test_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        vector_size: Option<usize>,
        distance: Option<DistanceMetric>,
    ) -> Result<String> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        let config = CollectionConfig::new(
            vector_size.unwrap_or(self.default_dimension),
            distance.unwrap_or(DistanceMetric::Cosine),
        );
        self.manager.create_index(&collection_name, config).await?;
        Ok(collection_name)
    }

    pub async fn insert_test_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
        vector: Vec<f32>,
        payload: Option<Payload>,
    ) -> Result<()> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        let mut point = VectorPoint::new(point_id.to_string(), vector);
        if let Some(p) = payload {
            point = point.with_payload(p);
        }
        self.manager.upsert(&collection_name, point).await
    }

    pub async fn insert_test_vectors(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        points: Vec<VectorPoint>,
    ) -> Result<()> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.upsert_batch(&collection_name, points).await
    }

    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        let query = SearchQuery::new(query_vector, limit);
        self.manager.search(&collection_name, query).await
    }

    pub async fn search_with_threshold(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query_vector: Vec<f32>,
        limit: usize,
        threshold: f32,
    ) -> Result<Vec<SearchResult>> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        let query = SearchQuery::new(query_vector, limit).with_score_threshold(threshold);
        self.manager.search(&collection_name, query).await
    }

    pub async fn drop_index(&self, space_id: u64, tag_name: &str, field_name: &str) -> Result<()> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.drop_index(&collection_name).await
    }

    pub fn has_index(&self, space_id: u64, tag_name: &str, field_name: &str) -> bool {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.index_exists(&collection_name)
    }

    pub async fn count(&self, space_id: u64, tag_name: &str, field_name: &str) -> Result<u64> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.count(&collection_name).await
    }

    pub async fn get_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> Result<Option<VectorPoint>> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.get(&collection_name, point_id).await
    }

    pub async fn delete_vector(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        point_id: &str,
    ) -> Result<()> {
        let collection_name = format!("space_{}_{}_{}", space_id, tag_name, field_name);
        self.manager.delete(&collection_name, point_id).await
    }

    pub async fn generate_embedding(&self, text: &str) -> Vec<f32> {
        self.embedding_provider
            .embed(&[text])
            .await
            .expect("Failed to generate embedding")
            .into_iter()
            .next()
            .expect("Expected one embedding")
    }

    pub async fn generate_embeddings(&self, texts: &[&str]) -> Vec<Vec<f32>> {
        self.embedding_provider
            .embed(texts)
            .await
            .expect("Failed to generate embeddings")
    }
}

impl Default for VectorTestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a random vector with specified dimension
pub fn generate_random_vector(dimension: usize) -> Vec<f32> {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hash, Hasher};

    let state = RandomState::new();
    let mut hasher = state.build_hasher();
    std::time::SystemTime::now().hash(&mut hasher);

    let mut vector = Vec::with_capacity(dimension);
    for i in 0..dimension {
        let mut h = hasher.clone();
        h.write_u64(i as u64);
        let val = h.finish() as f32 / u64::MAX as f32;
        vector.push(val * 2.0 - 1.0);
    }

    let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for val in vector.iter_mut() {
            *val /= norm;
        }
    }
    vector
}

/// Generate test vectors with specific patterns
pub fn generate_test_vectors(count: usize, dimension: usize, seed: u64) -> Vec<Vec<f32>> {
    (0..count)
        .map(|i| {
            let mut vector = vec![0.0f32; dimension];
            let base = ((seed + i as u64) as f32 * 0.1).fract();
            for (j, val) in vector.iter_mut().enumerate() {
                *val = (base + j as f32 * 0.1).sin();
            }
            let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                for val in vector.iter_mut() {
                    *val /= norm;
                }
            }
            vector
        })
        .collect()
}

/// Create test points with IDs
pub fn create_test_points(
    ids: Vec<&str>,
    vectors: Vec<Vec<f32>>,
    payloads: Option<Vec<Payload>>,
) -> Vec<VectorPoint> {
    ids.into_iter()
        .zip(vectors)
        .enumerate()
        .map(|(i, (id, vector))| {
            let mut point = VectorPoint::new(id.to_string(), vector);
            if let Some(ref payloads) = payloads {
                if i < payloads.len() {
                    point = point.with_payload(payloads[i].clone());
                }
            }
            point
        })
        .collect()
}

/// Assert search result contains point
pub fn assert_search_result_contains(
    results: &[SearchResult],
    expected_id: &str,
) -> std::result::Result<(), String> {
    if results.iter().any(|r| r.id.to_string() == expected_id) {
        Ok(())
    } else {
        Err(format!(
            "Search results should contain point '{}', but got: {:?}",
            expected_id,
            results.iter().map(|r| &r.id).collect::<Vec<_>>()
        ))
    }
}

/// Assert search result does not contain point
#[allow(dead_code)]
pub fn assert_search_result_not_contains(
    results: &[SearchResult],
    unexpected_id: &str,
) -> std::result::Result<(), String> {
    if !results.iter().any(|r| r.id.to_string() == unexpected_id) {
        Ok(())
    } else {
        Err(format!(
            "Search results should not contain point '{}'",
            unexpected_id
        ))
    }
}

/// Assert search result count
pub fn assert_search_result_count(
    results: &[SearchResult],
    expected_count: usize,
) -> std::result::Result<(), String> {
    if results.len() == expected_count {
        Ok(())
    } else {
        Err(format!(
            "Expected {} results, but got {}",
            expected_count,
            results.len()
        ))
    }
}

/// Assert search results are sorted by score (descending)
pub fn assert_results_sorted_by_score(results: &[SearchResult]) -> std::result::Result<(), String> {
    for i in 1..results.len() {
        if results[i].score > results[i - 1].score {
            return Err(format!(
                "Results should be sorted by score descending, but found {} > {} at positions {} and {}",
                results[i].score,
                results[i - 1].score,
                i,
                i - 1
            ));
        }
    }
    Ok(())
}

/// Assert all scores are above threshold
pub fn assert_scores_above_threshold(
    results: &[SearchResult],
    threshold: f32,
) -> std::result::Result<(), String> {
    for result in results {
        if result.score < threshold {
            return Err(format!(
                "Score {} is below threshold {} for point '{}'",
                result.score, threshold, result.id
            ));
        }
    }
    Ok(())
}

/// Create a simple payload
pub fn create_simple_payload(key: &str, value: &str) -> Payload {
    let mut payload = Payload::new();
    payload.insert(key.to_string(), serde_json::json!(value));
    payload
}

/// Create payload with multiple fields
pub fn create_payload(fields: Vec<(&str, serde_json::Value)>) -> Payload {
    fields
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}
