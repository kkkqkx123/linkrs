//! Schema contract DTOs (HTTP `/schema` endpoints).
//!
//! `PropertyDef` is the unified property definition: the server-side
//! `api-core PropertyDef` (which carries `default_value`/`comment`) and the
//! CLI's serialization-only mirror are both represented by this single wire
//! type. `data_type` is the core `DataType` `Display` output
//! (`BOOL`/`INT`/`FIXEDSTRING(8)`/`VECTOR_DENSE(3)` ...), which the server
//! parses back through the same `FromStr` source of truth.

use serde::{Deserialize, Serialize};

/// Space information
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceInfo {
    pub id: u64,
    pub name: String,
    pub vid_type: String,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Tag information
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagInfo {
    pub name: String,
    #[serde(default, alias = "properties")]
    pub fields: Vec<FieldInfo>,
}

/// Edge type information
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EdgeTypeInfo {
    pub name: String,
    #[serde(default, alias = "properties")]
    pub fields: Vec<FieldInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_tag: Option<String>,
}

/// Field information
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct FieldInfo {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    #[serde(default)]
    pub default_value: Option<String>,
}

/// Property definition (create tag / edge type request payload).
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct PropertyDef {
    pub name: String,
    /// Core `DataType` wire name (see module docs).
    pub data_type: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default)]
    #[schema(value_type = Option<crate::query::CellValue>)]
    pub default_value: Option<serde_json::Value>,
    #[serde(default)]
    pub comment: Option<String>,
}

impl PropertyDef {
    /// Create a property definition from a core data type name.
    pub fn new(name: impl Into<String>, data_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            data_type: data_type.into(),
            nullable: true,
            default_value: None,
            comment: None,
        }
    }

    /// Mark the property as NOT NULL.
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }
}

/// Create space request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateSpaceRequest {
    pub name: String,
    #[serde(default)]
    pub vid_type: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
}

/// Create tag request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateTagRequest {
    pub name: String,
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
}

/// Create edge type request
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CreateEdgeTypeRequest {
    pub name: String,
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
}

/// Space creation receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceCreateResponse {
    pub message: String,
    pub space_name: String,
}

/// Space deletion receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceDeleteResponse {
    pub message: String,
    pub space_name: String,
}

/// Space listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceListResponse {
    #[serde(default)]
    pub spaces: Vec<SpaceInfo>,
}

/// Compact space summary embedded in detail responses.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceSummary {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vid_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Space detail payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SpaceDetailResponse {
    pub space: SpaceSummary,
}

/// Tag creation receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagCreateResponse {
    pub message: String,
    pub tag_name: String,
    pub space_name: String,
}

/// Tag listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagListResponse {
    #[serde(default)]
    pub tags: Vec<TagInfo>,
    pub space_name: String,
}

/// Tag detail payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagDetailResponse {
    pub name: String,
    pub space_name: String,
    #[serde(default)]
    pub fields: Vec<FieldInfo>,
}

/// Tag deletion receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct TagDeleteResponse {
    pub message: String,
    pub tag_name: String,
    pub space_name: String,
}

/// Edge type creation receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EdgeCreateResponse {
    pub message: String,
    pub edge_type_name: String,
    pub space_name: String,
}

/// Edge type listing payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EdgeListResponse {
    #[serde(default)]
    pub edge_types: Vec<EdgeTypeInfo>,
    pub space_name: String,
}

/// Edge type detail payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EdgeDetailResponse {
    pub name: String,
    pub space_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_tag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_tag: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldInfo>,
}

/// Edge type deletion receipt.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct EdgeDeleteResponse {
    pub message: String,
    pub edge_type_name: String,
    pub space_name: String,
}

/// One schema change entry.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchemaChangeEntry {
    pub change_type: String,
    pub description: String,
    #[serde(default)]
    pub details: std::collections::HashMap<String, String>,
}

/// One schema version with its changes.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchemaVersionEntry {
    pub version: u64,
    pub timestamp_ms: u64,
    #[serde(default)]
    pub changes: Vec<SchemaChangeEntry>,
}

/// Version history payload.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct VersionHistoryResponse {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    #[serde(default)]
    pub versions: Vec<SchemaVersionEntry>,
}

/// Schema changes between two versions.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SchemaChangesResponse {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    #[serde(default)]
    pub changes: Vec<SchemaChangeEntry>,
}

/// Breaking change report.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct BreakingChangesResponse {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    pub has_breaking_changes: bool,
    #[serde(default)]
    pub changes: Vec<SchemaChangeEntry>,
    pub recommendation: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn property_def_roundtrip() {
        let def = PropertyDef {
            name: "age".to_string(),
            data_type: "INT".to_string(),
            nullable: false,
            default_value: Some(serde_json::json!(18)),
            comment: Some("years".to_string()),
        };
        let json = serde_json::to_string(&def).unwrap();
        let back: PropertyDef = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "age");
        assert_eq!(back.data_type, "INT");
        assert!(!back.nullable);
        assert_eq!(back.default_value, Some(serde_json::json!(18)));
        assert_eq!(back.comment.as_deref(), Some("years"));
    }

    #[test]
    fn property_def_defaults_absent_fields() {
        let back: PropertyDef =
            serde_json::from_str(r#"{"name": "name", "data_type": "STRING"}"#).unwrap();
        assert!(!back.nullable);
        assert!(back.default_value.is_none());
        assert!(back.comment.is_none());
    }

    #[test]
    fn space_info_roundtrip() {
        let info = SpaceInfo {
            id: 1,
            name: "space1".to_string(),
            vid_type: "INT64".to_string(),
            comment: None,
        };
        let json = serde_json::to_string(&info).unwrap();
        let back: SpaceInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, 1);
        assert_eq!(back.name, "space1");
    }

    #[test]
    fn create_space_request_roundtrip() {
        let request = CreateSpaceRequest {
            name: "s".to_string(),
            vid_type: Some("STRING".to_string()),
            comment: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let back: CreateSpaceRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.vid_type.as_deref(), Some("STRING"));
    }
}
