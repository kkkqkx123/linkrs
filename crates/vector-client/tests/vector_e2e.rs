#[cfg(feature = "qdrant-http")]
mod e2e_tests;

#[cfg(not(feature = "qdrant-http"))]
#[test]
fn e2e_tests_require_qdrant_http_feature() {}
