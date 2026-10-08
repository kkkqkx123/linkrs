//! Schema import/export
//!
//! Export and import space schema definitions (tags, edge types, indexes)
//! in JSON or YAML format.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::session::manager::SessionManager;
use linkrs_core::types::import_export::SchemaImportResult;

#[derive(Debug, Clone)]
pub struct SchemaIoConfig {
    pub space_name: String,
    pub output_path: PathBuf,
    pub format: SchemaExportFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SchemaExportFormat {
    #[default]
    Json,
    Yaml,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SchemaDefinition {
    pub space_name: String,
    pub tags: Vec<TagDefinition>,
    pub edge_types: Vec<EdgeTypeDefinition>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TagDefinition {
    pub name: String,
    pub properties: Vec<PropertyDefinition>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EdgeTypeDefinition {
    pub name: String,
    pub properties: Vec<PropertyDefinition>,
    pub source_tag: Option<String>,
    pub target_tag: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PropertyDefinition {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
}

pub struct SchemaExporter;

impl SchemaExporter {
    pub fn new() -> Self {
        Self
    }

    pub async fn export(&self, config: SchemaIoConfig, session: &mut SessionManager) -> Result<()> {
        let space = if config.space_name.is_empty() {
            session.current_space().unwrap_or_default().to_string()
        } else {
            config.space_name.clone()
        };
        let tags = session
            .client()
            .list_tags(&space)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let edge_types = session
            .client()
            .list_edge_types(&space)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;

        let definition = SchemaDefinition {
            space_name: space,
            tags: tags
                .into_iter()
                .map(|t| TagDefinition {
                    name: t.name,
                    properties: t
                        .fields
                        .into_iter()
                        .map(|f| PropertyDefinition {
                            name: f.name,
                            data_type: f.data_type,
                            nullable: f.nullable,
                        })
                        .collect(),
                })
                .collect(),
            edge_types: edge_types
                .into_iter()
                .map(|e| EdgeTypeDefinition {
                    name: e.name,
                    properties: e
                        .fields
                        .into_iter()
                        .map(|f| PropertyDefinition {
                            name: f.name,
                            data_type: f.data_type,
                            nullable: f.nullable,
                        })
                        .collect(),
                    source_tag: None,
                    target_tag: None,
                })
                .collect(),
        };

        let content = match config.format {
            SchemaExportFormat::Json => serde_json::to_string_pretty(&definition)?,
            SchemaExportFormat::Yaml => serde_json::to_string(&definition)?,
        };

        fs::write(&config.output_path, content)?;
        Ok(())
    }
}

impl Default for SchemaExporter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SchemaImporter;

impl SchemaImporter {
    pub fn new() -> Self {
        Self
    }

    pub async fn import(
        &self,
        path: &PathBuf,
        session: &mut SessionManager,
    ) -> Result<SchemaImportResult> {
        let content = fs::read_to_string(path)?;
        let definition: SchemaDefinition = serde_json::from_str(&content)?;
        let space = if definition.space_name.is_empty() {
            session.current_space().unwrap_or_default().to_string()
        } else {
            definition.space_name.clone()
        };

        let mut imported_tags = Vec::new();
        let mut imported_edge_types = Vec::new();
        let mut skipped_items = Vec::new();
        let mut errors = Vec::new();

        for tag in &definition.tags {
            let properties = tag
                .properties
                .iter()
                .map(|p| linkrs_wire::schema::PropertyDef {
                    name: p.name.clone(),
                    data_type: p.data_type.clone(),
                    nullable: p.nullable,
                    default_value: None,
                    comment: None,
                })
                .collect();
            match session
                .client()
                .create_tag(&space, &tag.name, properties)
                .await
            {
                Ok(()) => imported_tags.push(tag.name.clone()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("already exists") || msg.contains("409") {
                        skipped_items.push(tag.name.clone());
                    } else {
                        errors.push(format!("tag {}: {}", tag.name, msg));
                    }
                }
            }
        }

        for edge_type in &definition.edge_types {
            let properties = edge_type
                .properties
                .iter()
                .map(|p| linkrs_wire::schema::PropertyDef {
                    name: p.name.clone(),
                    data_type: p.data_type.clone(),
                    nullable: p.nullable,
                    default_value: None,
                    comment: None,
                })
                .collect();
            match session
                .client()
                .create_edge_type(&space, &edge_type.name, properties)
                .await
            {
                Ok(()) => imported_edge_types.push(edge_type.name.clone()),
                Err(e) => {
                    let msg = e.to_string();
                    if msg.contains("already exists") || msg.contains("409") {
                        skipped_items.push(edge_type.name.clone());
                    } else {
                        errors.push(format!("edge type {}: {}", edge_type.name, msg));
                    }
                }
            }
        }

        let imported_items = (imported_tags.len() + imported_edge_types.len()) as i32;
        Ok(SchemaImportResult {
            success: errors.is_empty(),
            space_name: space,
            imported_items,
            imported_tags,
            imported_edge_types,
            skipped_items,
            errors,
        })
    }
}

impl Default for SchemaImporter {
    fn default() -> Self {
        Self::new()
    }
}
