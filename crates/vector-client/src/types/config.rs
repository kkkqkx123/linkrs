use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum DistanceMetric {
    #[default]
    Cosine,
    Euclid,
    Dot,
    Manhattan,
}

impl DistanceMetric {
    pub fn is_supported_by_qdrant(&self) -> bool {
        matches!(self, Self::Cosine | Self::Euclid | Self::Dot)
    }

    pub fn requires_custom_implementation(&self) -> bool {
        matches!(self, Self::Manhattan)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HnswConfig {
    pub m: usize,
    pub ef_construct: usize,
    pub full_scan_threshold: Option<usize>,
    pub max_indexing_threads: Option<usize>,
    pub on_disk: Option<bool>,
    pub payload_m: Option<usize>,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construct: 100,
            full_scan_threshold: None,
            max_indexing_threads: None,
            on_disk: None,
            payload_m: None,
        }
    }
}

impl HnswConfig {
    pub fn new(m: usize, ef_construct: usize) -> Self {
        Self {
            m,
            ef_construct,
            full_scan_threshold: None,
            max_indexing_threads: None,
            on_disk: None,
            payload_m: None,
        }
    }

    pub fn with_full_scan_threshold(mut self, threshold: usize) -> Self {
        self.full_scan_threshold = Some(threshold);
        self
    }

    pub fn with_max_indexing_threads(mut self, threads: usize) -> Self {
        self.max_indexing_threads = Some(threads);
        self
    }

    pub fn with_on_disk(mut self, on_disk: bool) -> Self {
        self.on_disk = Some(on_disk);
        self
    }

    pub fn with_payload_m(mut self, payload_m: usize) -> Self {
        self.payload_m = Some(payload_m);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum IndexType {
    #[default]
    HNSW,
    FLAT,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionRatio {
    X4,
    X8,
    X16,
    X32,
    X64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QuantizationType {
    Scalar {
        quantile: Option<f32>,
        always_ram: Option<bool>,
    },
    Product {
        compression: CompressionRatio,
        always_ram: Option<bool>,
    },
    Binary {
        always_ram: Option<bool>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QuantizationConfig {
    pub enabled: bool,
    pub quant_type: Option<QuantizationType>,
}

impl QuantizationConfig {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            quant_type: None,
        }
    }

    pub fn scalar(quantile: f32) -> Self {
        Self {
            enabled: true,
            quant_type: Some(QuantizationType::Scalar {
                quantile: Some(quantile),
                always_ram: Some(true),
            }),
        }
    }

    pub fn product(compression: CompressionRatio) -> Self {
        Self {
            enabled: true,
            quant_type: Some(QuantizationType::Product {
                compression,
                always_ram: Some(true),
            }),
        }
    }

    pub fn binary() -> Self {
        Self {
            enabled: true,
            quant_type: Some(QuantizationType::Binary {
                always_ram: Some(true),
            }),
        }
    }

    pub fn with_always_ram(mut self, always_ram: bool) -> Self {
        if let Some(ref mut qt) = self.quant_type {
            match qt {
                QuantizationType::Scalar { always_ram: ar, .. } => *ar = Some(always_ram),
                QuantizationType::Product { always_ram: ar, .. } => *ar = Some(always_ram),
                QuantizationType::Binary { always_ram: ar } => *ar = Some(always_ram),
            }
        }
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    pub vector_size: usize,
    pub distance: DistanceMetric,
    pub index_type: Option<IndexType>,
    pub hnsw_config: Option<HnswConfig>,
    pub quantization_config: Option<QuantizationConfig>,
    pub replication_factor: Option<usize>,
    pub write_consistency_factor: Option<usize>,
    pub on_disk_payload: Option<bool>,
    pub shard_number: Option<usize>,
}

impl CollectionConfig {
    pub fn new(vector_size: usize, distance: DistanceMetric) -> Self {
        Self {
            vector_size,
            distance,
            index_type: None,
            hnsw_config: None,
            quantization_config: None,
            replication_factor: None,
            write_consistency_factor: None,
            on_disk_payload: None,
            shard_number: None,
        }
    }

    pub fn with_index_type(mut self, index_type: IndexType) -> Self {
        self.index_type = Some(index_type);
        self
    }

    pub fn with_hnsw(mut self, hnsw_config: HnswConfig) -> Self {
        self.index_type = Some(IndexType::HNSW);
        self.hnsw_config = Some(hnsw_config);
        self
    }

    pub fn with_quantization(mut self, quantization_config: QuantizationConfig) -> Self {
        self.quantization_config = Some(quantization_config);
        self
    }

    pub fn with_shard_number(mut self, shard_number: usize) -> Self {
        self.shard_number = Some(shard_number);
        self
    }

    pub fn with_on_disk_payload(mut self, on_disk_payload: bool) -> Self {
        self.on_disk_payload = Some(on_disk_payload);
        self
    }
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self::new(1536, DistanceMetric::Cosine)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub name: String,
    pub vector_count: u64,
    pub indexed_vector_count: u64,
    pub points_count: u64,
    pub segments_count: u64,
    pub config: CollectionConfig,
    pub status: CollectionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CollectionStatus {
    Green,
    Yellow,
    Red,
    Grey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PayloadSchemaType {
    Keyword,
    Integer,
    Float,
    Text,
    Bool,
    Geo,
    Datetime,
}

impl PayloadSchemaType {
    pub fn as_str(&self) -> &'static str {
        match self {
            PayloadSchemaType::Keyword => "keyword",
            PayloadSchemaType::Integer => "integer",
            PayloadSchemaType::Float => "float",
            PayloadSchemaType::Text => "text",
            PayloadSchemaType::Bool => "bool",
            PayloadSchemaType::Geo => "geo",
            PayloadSchemaType::Datetime => "datetime",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthStatus {
    pub is_healthy: bool,
    pub engine_name: String,
    pub engine_version: String,
    pub message: Option<String>,
}

impl HealthStatus {
    pub fn healthy(engine_name: impl Into<String>, engine_version: impl Into<String>) -> Self {
        Self {
            is_healthy: true,
            engine_name: engine_name.into(),
            engine_version: engine_version.into(),
            message: None,
        }
    }

    pub fn unhealthy(
        engine_name: impl Into<String>,
        engine_version: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            is_healthy: false,
            engine_name: engine_name.into(),
            engine_version: engine_version.into(),
            message: Some(message.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_distance_metric_supported() {
        assert!(DistanceMetric::Cosine.is_supported_by_qdrant());
        assert!(DistanceMetric::Euclid.is_supported_by_qdrant());
        assert!(DistanceMetric::Dot.is_supported_by_qdrant());
        assert!(!DistanceMetric::Manhattan.is_supported_by_qdrant());
    }

    #[test]
    fn test_distance_metric_custom_implementation() {
        assert!(!DistanceMetric::Cosine.requires_custom_implementation());
        assert!(!DistanceMetric::Euclid.requires_custom_implementation());
        assert!(!DistanceMetric::Dot.requires_custom_implementation());
        assert!(DistanceMetric::Manhattan.requires_custom_implementation());
    }

    #[test]
    fn test_hnsw_config_default() {
        let cfg = HnswConfig::default();
        assert_eq!(cfg.m, 16);
        assert_eq!(cfg.ef_construct, 100);
        assert!(cfg.full_scan_threshold.is_none());
    }

    #[test]
    fn test_hnsw_config_new() {
        let cfg = HnswConfig::new(32, 200);
        assert_eq!(cfg.m, 32);
        assert_eq!(cfg.ef_construct, 200);
    }

    #[test]
    fn test_hnsw_config_builder() {
        let cfg = HnswConfig::new(16, 100)
            .with_full_scan_threshold(10000)
            .with_max_indexing_threads(4)
            .with_on_disk(true)
            .with_payload_m(8);
        assert_eq!(cfg.full_scan_threshold, Some(10000));
        assert_eq!(cfg.max_indexing_threads, Some(4));
        assert_eq!(cfg.on_disk, Some(true));
        assert_eq!(cfg.payload_m, Some(8));
    }

    #[test]
    fn test_quantization_config_disabled() {
        let cfg = QuantizationConfig::disabled();
        assert!(!cfg.enabled);
        assert!(cfg.quant_type.is_none());
    }

    #[test]
    fn test_quantization_config_scalar() {
        let cfg = QuantizationConfig::scalar(0.99);
        assert!(cfg.enabled);
        match cfg.quant_type {
            Some(QuantizationType::Scalar { .. }) => {}
            _ => panic!("expected Scalar"),
        }
    }

    #[test]
    fn test_quantization_config_product() {
        let cfg = QuantizationConfig::product(CompressionRatio::X8);
        assert!(cfg.enabled);
        match cfg.quant_type {
            Some(QuantizationType::Product { compression, .. }) => {
                assert!(matches!(compression, CompressionRatio::X8));
            }
            _ => panic!("expected Product"),
        }
    }

    #[test]
    fn test_quantization_config_binary() {
        let cfg = QuantizationConfig::binary();
        assert!(cfg.enabled);
        match cfg.quant_type {
            Some(QuantizationType::Binary { .. }) => {}
            _ => panic!("expected Binary"),
        }
    }

    #[test]
    fn test_quantization_config_with_always_ram() {
        let cfg = QuantizationConfig::scalar(0.5).with_always_ram(false);
        match cfg.quant_type {
            Some(QuantizationType::Scalar { always_ram, .. }) => {
                assert_eq!(always_ram, Some(false));
            }
            _ => panic!("expected Scalar"),
        }
    }

    #[test]
    fn test_collection_config_new() {
        let cfg = CollectionConfig::new(768, DistanceMetric::Cosine);
        assert_eq!(cfg.vector_size, 768);
        assert_eq!(cfg.distance, DistanceMetric::Cosine);
    }

    #[test]
    fn test_collection_config_with_index_type() {
        let cfg =
            CollectionConfig::new(384, DistanceMetric::Euclid).with_index_type(IndexType::FLAT);
        assert_eq!(cfg.index_type, Some(IndexType::FLAT));
    }

    #[test]
    fn test_collection_config_with_hnsw() {
        let hnsw = HnswConfig::new(32, 200);
        let cfg = CollectionConfig::new(128, DistanceMetric::Dot).with_hnsw(hnsw);
        assert_eq!(cfg.index_type, Some(IndexType::HNSW));
        assert!(cfg.hnsw_config.is_some());
    }

    #[test]
    fn test_collection_config_with_quantization() {
        let q = QuantizationConfig::scalar(0.99);
        let cfg = CollectionConfig::new(1536, DistanceMetric::Cosine).with_quantization(q);
        assert!(cfg.quantization_config.is_some());
    }

    #[test]
    fn test_collection_config_with_shard_number() {
        let cfg = CollectionConfig::new(768, DistanceMetric::Cosine).with_shard_number(2);
        assert_eq!(cfg.shard_number, Some(2));
    }

    #[test]
    fn test_collection_config_with_on_disk_payload() {
        let cfg = CollectionConfig::new(768, DistanceMetric::Cosine).with_on_disk_payload(true);
        assert_eq!(cfg.on_disk_payload, Some(true));
    }

    #[test]
    fn test_collection_config_default() {
        let cfg = CollectionConfig::default();
        assert_eq!(cfg.vector_size, 1536);
        assert_eq!(cfg.distance, DistanceMetric::Cosine);
    }

    #[test]
    fn test_payload_schema_type_as_str() {
        assert_eq!(PayloadSchemaType::Keyword.as_str(), "keyword");
        assert_eq!(PayloadSchemaType::Integer.as_str(), "integer");
        assert_eq!(PayloadSchemaType::Float.as_str(), "float");
        assert_eq!(PayloadSchemaType::Text.as_str(), "text");
        assert_eq!(PayloadSchemaType::Bool.as_str(), "bool");
        assert_eq!(PayloadSchemaType::Geo.as_str(), "geo");
        assert_eq!(PayloadSchemaType::Datetime.as_str(), "datetime");
    }

    #[test]
    fn test_health_status_healthy() {
        let h = HealthStatus::healthy("test-engine", "1.0");
        assert!(h.is_healthy);
        assert_eq!(h.engine_name, "test-engine");
        assert_eq!(h.engine_version, "1.0");
        assert!(h.message.is_none());
    }

    #[test]
    fn test_health_status_unhealthy() {
        let h = HealthStatus::unhealthy("test-engine", "1.0", "not ready");
        assert!(!h.is_healthy);
        assert_eq!(h.message, Some("not ready".to_string()));
    }

    #[test]
    fn test_index_type_default() {
        assert_eq!(IndexType::default(), IndexType::HNSW);
    }

    #[test]
    fn test_distance_metric_default() {
        assert_eq!(DistanceMetric::default(), DistanceMetric::Cosine);
    }

    #[test]
    fn test_collection_status_debug() {
        assert_eq!(format!("{:?}", CollectionStatus::Green), "Green");
        assert_eq!(format!("{:?}", CollectionStatus::Yellow), "Yellow");
        assert_eq!(format!("{:?}", CollectionStatus::Red), "Red");
        assert_eq!(format!("{:?}", CollectionStatus::Grey), "Grey");
    }
}
