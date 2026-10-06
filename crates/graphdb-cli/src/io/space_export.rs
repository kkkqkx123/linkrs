//! Full space export with tag-based iteration
//!
//! Exports all vertices organized by tags and all edges organized by edge types
//! within a space, leveraging the storage layer's per-tag/per-edge-type scan APIs.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::io::ExportFormat;
use crate::session::manager::SessionManager;

#[derive(Debug, Clone)]
pub struct SpaceExportConfig {
    pub space_name: String,
    pub output_path: PathBuf,
    pub format: ExportFormat,
    pub include_schema: bool,
    pub include_data: bool,
    pub streaming: bool,
    pub chunk_size: usize,
    pub tag_filter: Option<Vec<String>>,
    pub edge_type_filter: Option<Vec<String>>,
}

impl Default for SpaceExportConfig {
    fn default() -> Self {
        Self {
            space_name: String::new(),
            output_path: PathBuf::from("export"),
            format: ExportFormat::csv(),
            include_schema: true,
            include_data: true,
            streaming: true,
            chunk_size: 1000,
            tag_filter: None,
            edge_type_filter: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SpaceExportStats {
    pub tags_exported: usize,
    pub edge_types_exported: usize,
    pub total_vertices: usize,
    pub total_edges: usize,
    pub bytes_written: u64,
    pub duration_ms: u64,
    pub errors: Vec<String>,
}

impl SpaceExportStats {
    pub fn format_summary(&self) -> String {
        let mut output = String::new();
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Space Export Statistics\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&format!("Tags exported:     {}\n", self.tags_exported));
        output.push_str(&format!(
            "Edge types:        {}\n",
            self.edge_types_exported
        ));
        output.push_str(&format!("Total vertices:    {}\n", self.total_vertices));
        output.push_str(&format!("Total edges:       {}\n", self.total_edges));
        output.push_str(&format!(
            "Bytes written:     {}\n",
            format_bytes(self.bytes_written)
        ));
        output.push_str(&format!(
            "Duration:          {:.3} s\n",
            self.duration_ms as f64 / 1000.0
        ));
        if !self.errors.is_empty() {
            output.push_str(&format!("Errors:            {}\n", self.errors.len()));
            for err in self.errors.iter().take(5) {
                output.push_str(&format!("  - {}\n", err));
            }
        }
        output
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TagExportData {
    pub tag_name: String,
    pub vertex_count: u64,
    pub property_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct EdgeTypeExportData {
    pub edge_type_name: String,
    pub edge_count: u64,
    pub property_names: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpaceExportMetadata {
    pub version: String,
    pub timestamp: i64,
    pub space_name: String,
    pub format: String,
    pub tags: Vec<TagExportData>,
    pub edge_types: Vec<EdgeTypeExportData>,
    pub total_vertices: u64,
    pub total_edges: u64,
}

pub struct SpaceExporter {
    config: SpaceExportConfig,
    start_time: Instant,
}

impl SpaceExporter {
    pub fn new(config: SpaceExportConfig) -> Self {
        Self {
            config,
            start_time: Instant::now(),
        }
    }

    pub async fn export(&self, session: &mut SessionManager) -> Result<SpaceExportStats> {
        let mut stats = SpaceExportStats {
            tags_exported: 0,
            edge_types_exported: 0,
            total_vertices: 0,
            total_edges: 0,
            bytes_written: 0,
            duration_ms: 0,
            errors: Vec::new(),
        };

        self.ensure_space(session).await.unwrap_or_else(|e| {
            stats.errors.push(format!("Use space failed: {}", e));
        });

        let file = File::create(&self.config.output_path)?;
        let mut writer = BufWriter::new(file);

        let mut metadata = SpaceExportMetadata {
            version: env!("CARGO_PKG_VERSION").to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            space_name: self.config.space_name.clone(),
            format: format!("{:?}", self.config.format),
            tags: Vec::new(),
            edge_types: Vec::new(),
            total_vertices: 0,
            total_edges: 0,
        };

        if self.config.include_schema {
            writeln!(writer, "# GraphDB Space Export")?;
            writeln!(writer, "# Space: {}", metadata.space_name)?;
            writeln!(writer, "# Version: {}", metadata.version)?;
            writeln!(writer, "# Timestamp: {}", metadata.timestamp)?;
            writeln!(writer, "#")?;
        }

        if self.config.include_data {
            match &self.config.format {
                ExportFormat::Csv { .. } => {
                    self.export_csv(&mut writer, &mut stats, &mut metadata, session)
                        .await?;
                }
                ExportFormat::Json { .. } => {
                    self.export_json(&mut writer, &mut stats, &mut metadata, session)
                        .await?;
                }
                ExportFormat::JsonLines => {
                    self.export_jsonl(&mut writer, &mut stats, &mut metadata, session)
                        .await?;
                }
            }
        }

        if self.config.include_schema {
            let meta_path = self.config.output_path.with_extension("metadata.json");
            serde_json::to_writer_pretty(std::fs::File::create(&meta_path)?, &metadata)?;
        }

        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;
        stats.bytes_written = std::fs::metadata(&self.config.output_path)?.len();

        Ok(stats)
    }

    async fn ensure_space(&self, session: &mut SessionManager) -> Result<()> {
        if self.config.space_name.is_empty() {
            return Ok(());
        }
        let current = session.current_space().unwrap_or_default().to_string();
        if current != self.config.space_name {
            session
                .switch_space(&self.config.space_name)
                .await
                .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        }
        Ok(())
    }

    async fn list_targets(&self, session: &SessionManager) -> (Vec<String>, Vec<String>) {
        let space = if self.config.space_name.is_empty() {
            session.current_space().unwrap_or_default().to_string()
        } else {
            self.config.space_name.clone()
        };
        let tags = session
            .client()
            .list_tags(&space)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.name)
            .filter(|n| {
                self.config
                    .tag_filter
                    .as_ref()
                    .map(|f| f.contains(n))
                    .unwrap_or(true)
            })
            .collect();
        let edge_types = session
            .client()
            .list_edge_types(&space)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|e| e.name)
            .filter(|n| {
                self.config
                    .edge_type_filter
                    .as_ref()
                    .map(|f| f.contains(n))
                    .unwrap_or(true)
            })
            .collect();
        (tags, edge_types)
    }

    async fn fetch_vertices(
        &self,
        session: &SessionManager,
        tag: &str,
    ) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        loop {
            let query = format!(
                "MATCH (n:{}) RETURN n SKIP {} LIMIT {}",
                tag, offset, self.config.chunk_size
            );
            let result = match session.execute_query(&query).await {
                Ok(r) => r,
                Err(_) => break,
            };
            if result.rows.is_empty() {
                break;
            }
            let len = result.rows.len();
            for row in &result.rows {
                out.push(serde_json::to_value(row).unwrap_or(serde_json::Value::Null));
            }
            offset += len;
            if len < self.config.chunk_size {
                break;
            }
        }
        out
    }

    async fn fetch_edges(
        &self,
        session: &SessionManager,
        edge_type: &str,
    ) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let mut offset = 0usize;
        loop {
            let query = format!(
                "MATCH ()-[e:{}]->() RETURN e SKIP {} LIMIT {}",
                edge_type, offset, self.config.chunk_size
            );
            let result = match session.execute_query(&query).await {
                Ok(r) => r,
                Err(_) => break,
            };
            if result.rows.is_empty() {
                break;
            }
            let len = result.rows.len();
            for row in &result.rows {
                out.push(serde_json::to_value(row).unwrap_or(serde_json::Value::Null));
            }
            offset += len;
            if len < self.config.chunk_size {
                break;
            }
        }
        out
    }

    async fn export_csv(
        &self,
        writer: &mut impl Write,
        stats: &mut SpaceExportStats,
        metadata: &mut SpaceExportMetadata,
        session: &mut SessionManager,
    ) -> Result<()> {
        writeln!(writer, "type,tag,properties")?;

        let (tags, edge_types) = self.list_targets(session).await;

        for tag in tags {
            let rows = self.fetch_vertices(session, &tag).await;
            let count = rows.len() as u64;
            for row in &rows {
                writeln!(writer, "vertex,{},{}", tag, row)?;
                stats.total_vertices += 1;
            }
            if count > 0 {
                metadata.tags.push(TagExportData {
                    tag_name: tag.clone(),
                    vertex_count: count,
                    property_names: Vec::new(),
                });
                stats.tags_exported += 1;
            }
        }

        for edge_type in edge_types {
            let rows = self.fetch_edges(session, &edge_type).await;
            let count = rows.len() as u64;
            for row in &rows {
                writeln!(writer, "edge,{},{}", edge_type, row)?;
                stats.total_edges += 1;
            }
            if count > 0 {
                metadata.edge_types.push(EdgeTypeExportData {
                    edge_type_name: edge_type.clone(),
                    edge_count: count,
                    property_names: Vec::new(),
                });
                stats.edge_types_exported += 1;
            }
        }

        metadata.total_vertices = stats.total_vertices as u64;
        metadata.total_edges = stats.total_edges as u64;

        Ok(())
    }

    async fn export_json(
        &self,
        writer: &mut impl Write,
        stats: &mut SpaceExportStats,
        metadata: &mut SpaceExportMetadata,
        session: &mut SessionManager,
    ) -> Result<()> {
        let (tags, edge_types) = self.list_targets(session).await;
        let mut vertices: Vec<serde_json::Value> = Vec::new();
        let mut edges: Vec<serde_json::Value> = Vec::new();

        for tag in tags {
            let rows = self.fetch_vertices(session, &tag).await;
            let count = rows.len() as u64;
            stats.total_vertices += rows.len();
            if count > 0 {
                metadata.tags.push(TagExportData {
                    tag_name: tag.clone(),
                    vertex_count: count,
                    property_names: Vec::new(),
                });
                stats.tags_exported += 1;
            }
            for row in rows {
                vertices.push(serde_json::json!({"tag": tag, "properties": row}));
            }
        }

        for edge_type in edge_types {
            let rows = self.fetch_edges(session, &edge_type).await;
            let count = rows.len() as u64;
            stats.total_edges += rows.len();
            if count > 0 {
                metadata.edge_types.push(EdgeTypeExportData {
                    edge_type_name: edge_type.clone(),
                    edge_count: count,
                    property_names: Vec::new(),
                });
                stats.edge_types_exported += 1;
            }
            for row in rows {
                edges.push(serde_json::json!({"edge_type": edge_type, "properties": row}));
            }
        }

        metadata.total_vertices = stats.total_vertices as u64;
        metadata.total_edges = stats.total_edges as u64;

        let doc = serde_json::json!({
            "space": self.config.space_name,
            "schema": {
                "tags": metadata.tags,
                "edge_types": metadata.edge_types,
            },
            "data": {
                "vertices": vertices,
                "edges": edges,
            },
        });
        writer.write_all(serde_json::to_string_pretty(&doc)?.as_bytes())?;
        writer.write_all(b"\n")?;
        Ok(())
    }

    async fn export_jsonl(
        &self,
        writer: &mut impl Write,
        stats: &mut SpaceExportStats,
        metadata: &mut SpaceExportMetadata,
        session: &mut SessionManager,
    ) -> Result<()> {
        let (tags, edge_types) = self.list_targets(session).await;

        for tag in tags {
            let rows = self.fetch_vertices(session, &tag).await;
            let count = rows.len() as u64;
            stats.total_vertices += rows.len();
            if count > 0 {
                metadata.tags.push(TagExportData {
                    tag_name: tag.clone(),
                    vertex_count: count,
                    property_names: Vec::new(),
                });
                stats.tags_exported += 1;
            }
            for row in rows {
                let line = serde_json::json!({"type": "vertex", "tag": tag, "properties": row});
                writer.write_all(serde_json::to_string(&line)?.as_bytes())?;
                writer.write_all(b"\n")?;
            }
        }

        for edge_type in edge_types {
            let rows = self.fetch_edges(session, &edge_type).await;
            let count = rows.len() as u64;
            stats.total_edges += rows.len();
            if count > 0 {
                metadata.edge_types.push(EdgeTypeExportData {
                    edge_type_name: edge_type.clone(),
                    edge_count: count,
                    property_names: Vec::new(),
                });
                stats.edge_types_exported += 1;
            }
            for row in rows {
                let line = serde_json::json!({
                    "type": "edge",
                    "edge_type": edge_type,
                    "properties": row,
                });
                writer.write_all(serde_json::to_string(&line)?.as_bytes())?;
                writer.write_all(b"\n")?;
            }
        }

        metadata.total_vertices = stats.total_vertices as u64;
        metadata.total_edges = stats.total_edges as u64;
        Ok(())
    }
}
