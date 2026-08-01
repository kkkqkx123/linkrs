#![cfg(feature = "qdrant-http")]

#[path = "e2e_tests/common.rs"]
mod common;
#[path = "e2e_tests/e2e_cleanup.rs"]
mod e2e_cleanup;
#[path = "e2e_tests/geo_e2e.rs"]
mod geo_e2e;
#[path = "e2e_tests/hnsw_quantization_e2e.rs"]
mod hnsw_quantization_e2e;
#[path = "e2e_tests/http_engine_e2e.rs"]
mod http_engine_e2e;
