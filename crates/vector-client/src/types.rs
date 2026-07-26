mod config;
mod filter;
mod point;
mod search;

pub use config::*;
pub use filter::*;
pub use point::*;
pub use search::*;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub type Payload = HashMap<String, serde_json::Value>;
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PointId {
    Num(u64),
    Uuid(String),
}

impl std::fmt::Display for PointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PointId::Num(n) => write!(f, "{}", n),
            PointId::Uuid(s) => write!(f, "{}", s),
        }
    }
}

impl From<u64> for PointId {
    fn from(n: u64) -> Self {
        PointId::Num(n)
    }
}

impl From<String> for PointId {
    fn from(s: String) -> Self {
        if let Ok(n) = s.parse::<u64>() {
            PointId::Num(n)
        } else {
            PointId::Uuid(s)
        }
    }
}

impl From<&str> for PointId {
    fn from(s: &str) -> Self {
        PointId::from(s.to_string())
    }
}

pub type CollectionName = String;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_id_from_u64() {
        let id: PointId = 42u64.into();
        assert_eq!(id, PointId::Num(42));
    }

    #[test]
    fn test_point_id_from_string_numeric() {
        let id = PointId::from("123");
        assert_eq!(id, PointId::Num(123));
    }

    #[test]
    fn test_point_id_from_string_uuid() {
        let id = PointId::from("uuid-abc");
        assert_eq!(id, PointId::Uuid("uuid-abc".into()));
    }

    #[test]
    fn test_point_id_from_str() {
        let id = PointId::from("456");
        assert_eq!(id, PointId::Num(456));
    }

    #[test]
    fn test_point_id_from_str_uuid() {
        let id = PointId::from("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(
            id,
            PointId::Uuid("550e8400-e29b-41d4-a716-446655440000".into())
        );
    }

    #[test]
    fn test_point_id_display_num() {
        let id = PointId::Num(42);
        assert_eq!(format!("{}", id), "42");
    }

    #[test]
    fn test_point_id_display_uuid() {
        let id = PointId::Uuid("abc".into());
        assert_eq!(format!("{}", id), "abc");
    }

    #[test]
    fn test_point_id_serialize_deserialize() {
        let id = PointId::Num(42);
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: PointId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }

    #[test]
    fn test_point_id_serialize_deserialize_uuid() {
        let id = PointId::Uuid("test-uuid".into());
        let json = serde_json::to_string(&id).unwrap();
        let deserialized: PointId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, deserialized);
    }
}
