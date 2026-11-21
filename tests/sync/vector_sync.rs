//! Vector sync integration tests at the root crate boundary.
//!
//! These tests exercise the public coordinator surface and the remote backend
//! shell, so they require `vector-qdrant`. Availability contract: a vector
//! subsystem that is not enabled never becomes a `VectorBackend`; there is no
//! "present but always failing" engine state to detect at runtime.
#![cfg(feature = "vector-qdrant")]

use std::collections::HashMap;
use std::sync::Arc;

use linkrs::core::Value;
use linkrs::sync::{
    VectorBackend, VectorClientConfig, VectorIndexLocation, VectorManager, VectorPointData,
};

/// A vector client built from a disabled configuration must be rejected at
/// backend construction rather than producing a coordinator whose every call
/// fails.
#[tokio::test]
async fn disabled_client_cannot_become_a_backend() {
    let manager = Arc::new(
        VectorManager::new(VectorClientConfig::disabled())
            .await
            .expect("a disabled manager builds without I/O"),
    );
    assert!(!manager.is_engine_available());
    assert!(
        VectorBackend::qdrant(manager).is_err(),
        "a disabled client must not be wrapped into a backend"
    );
}

#[tokio::test]
async fn index_location_naming_and_logical_isolation() {
    let location = VectorIndexLocation::new(5, "Products", "image_embedding");
    assert_eq!(location.space_id, 5);
    assert_eq!(location.tag_name, "Products");
    assert_eq!(location.field_name, "image_embedding");

    // All indexes of a space share one physical collection; the group id is
    // the logical isolation key inside it, length-prefixed so that a tag or
    // field containing `_` cannot be confused with a different pair.
    assert_eq!(location.to_collection_name(), "space_vec_5");
    assert_eq!(location.group_id(), "8_Products_15_image_embedding");
}

#[tokio::test]
async fn point_data_carries_user_payload() {
    let mut payload: HashMap<Arc<str>, Value> = HashMap::new();
    payload.insert(Arc::from("category"), Value::string("electronics"));
    payload.insert(Arc::from("price"), Value::string("99.99"));

    let point = VectorPointData {
        id: "point_1".to_string(),
        vector: vec![1.5, 2.5, 3.5, 4.5],
        payload,
    };

    assert_eq!(point.id, "point_1");
    assert_eq!(point.vector.len(), 4);
    assert_eq!(
        point.payload.get("category").unwrap(),
        &Value::string("electronics")
    );
    assert_eq!(point.payload.get("price").unwrap(), &Value::string("99.99"));
}
