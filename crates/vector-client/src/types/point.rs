use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::{Payload, PointId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorPoint {
    pub id: PointId,
    pub vector: Vec<f32>,
    pub payload: Option<Payload>,
}

impl VectorPoint {
    pub fn new(id: impl Into<PointId>, vector: Vec<f32>) -> Self {
        Self {
            id: id.into(),
            vector,
            payload: None,
        }
    }

    pub fn with_payload(mut self, payload: Payload) -> Self {
        self.payload = Some(payload);
        self
    }

    pub fn with_payload_kv(mut self, key: impl Into<String>, value: serde_json::Value) -> Self {
        let payload = self.payload.get_or_insert_with(HashMap::new);
        payload.insert(key.into(), value);
        self
    }

    pub fn dimension(&self) -> usize {
        self.vector.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorPoints {
    pub points: Vec<VectorPoint>,
}

impl VectorPoints {
    pub fn new(points: Vec<VectorPoint>) -> Self {
        Self { points }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }
}

impl From<Vec<VectorPoint>> for VectorPoints {
    fn from(points: Vec<VectorPoint>) -> Self {
        Self::new(points)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpsertResult {
    pub operation_id: Option<u64>,
    pub status: UpsertStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UpsertStatus {
    Completed,
    Acknowledged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteResult {
    pub operation_id: Option<u64>,
    pub deleted_count: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_point_new() {
        let p = VectorPoint::new(42u64, vec![1.0, 2.0, 3.0]);
        assert_eq!(p.id, PointId::Num(42));
        assert_eq!(p.vector, vec![1.0, 2.0, 3.0]);
        assert!(p.payload.is_none());
    }

    #[test]
    fn test_vector_point_with_payload() {
        let mut payload = std::collections::HashMap::new();
        payload.insert("key".into(), serde_json::json!("val"));
        let p = VectorPoint::new("1", vec![1.0]).with_payload(payload);
        assert!(p.payload.is_some());
    }

    #[test]
    fn test_vector_point_with_payload_kv() {
        let p = VectorPoint::new(1u64, vec![1.0])
            .with_payload_kv("color", serde_json::json!("red"))
            .with_payload_kv("size", serde_json::json!(42));
        let payload = p.payload.expect("payload expected");
        assert_eq!(payload.get("color").and_then(|v| v.as_str()), Some("red"));
        assert_eq!(payload.get("size").and_then(|v| v.as_i64()), Some(42));
    }

    #[test]
    fn test_vector_point_dimension() {
        let p = VectorPoint::new(1u64, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(p.dimension(), 4);
    }

    #[test]
    fn test_vector_point_dimension_empty() {
        let p = VectorPoint::new(1u64, vec![]);
        assert_eq!(p.dimension(), 0);
    }

    #[test]
    fn test_vector_points_new() {
        let points = vec![VectorPoint::new(1u64, vec![1.0])];
        let vp = VectorPoints::new(points);
        assert_eq!(vp.len(), 1);
        assert!(!vp.is_empty());
    }

    #[test]
    fn test_vector_points_empty() {
        let vp = VectorPoints::new(vec![]);
        assert!(vp.is_empty());
        assert_eq!(vp.len(), 0);
    }

    #[test]
    fn test_vector_points_from() {
        let points = vec![VectorPoint::new(1u64, vec![1.0])];
        let vp: VectorPoints = points.into();
        assert_eq!(vp.len(), 1);
    }

    #[test]
    fn test_upsert_result() {
        let r = UpsertResult {
            operation_id: Some(123),
            status: UpsertStatus::Completed,
        };
        assert_eq!(r.operation_id, Some(123));
        assert_eq!(r.status, UpsertStatus::Completed);
    }

    #[test]
    fn test_delete_result() {
        let r = DeleteResult {
            operation_id: None,
            deleted_count: 5,
        };
        assert!(r.operation_id.is_none());
        assert_eq!(r.deleted_count, 5);
    }

    #[test]
    fn test_upsert_status_debug() {
        assert_eq!(format!("{:?}", UpsertStatus::Completed), "Completed");
        assert_eq!(format!("{:?}", UpsertStatus::Acknowledged), "Acknowledged");
    }
}
