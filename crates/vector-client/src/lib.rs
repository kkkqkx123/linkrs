pub mod api;

pub mod config;
pub mod engine;
pub mod error;
pub mod manager;
pub mod types;

pub use config::*;
pub use engine::VectorEngine;
pub use error::{Result, VectorClientError};
pub use types::*;

#[cfg(all(feature = "qdrant-http", not(feature = "qdrant-grpc")))]
pub use engine::QdrantEngine;

#[cfg(feature = "qdrant-grpc")]
pub use engine::QdrantGrpcEngine;

#[cfg(feature = "qdrant-grpc")]
pub use engine::grpc::streaming::StreamingEngine;

#[cfg(feature = "qdrant-grpc")]
pub use engine::grpc::interceptor::{attach_api_key, parse_api_key, API_KEY_HEADER};

pub use api::VectorClient;
pub use api::{CollectionApi, PointApi, SearchApi};
pub use manager::VectorManager;

#[cfg(feature = "embedding")]
pub use graphdb_embedding::{
    EmbeddingConfig, EmbeddingError, EmbeddingProvider, EmbeddingService, OpenAICompatibleProvider,
    PreprocessorConfig, PreprocessorImpl,
};
