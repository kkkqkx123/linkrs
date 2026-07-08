use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use anyhow::Result;

use crate::io::{ExportConfig, ExportFormat, ExportStats};
use crate::session::manager::SessionManager;

pub struct JsonExporter {
    config: ExportConfig,
    start_time: Instant,
}

impl JsonExporter {
    pub fn new(config: ExportConfig) -> Self {
        Self {
            config,
            start_time: Instant::now(),
        }
    }

    pub async fn export(&self, query: &str, session: &mut SessionManager) -> Result<ExportStats> {
        if self.config.streaming {
            self.export_streaming(query, session).await
        } else {
            self.export_batch(query, session).await
        }
    }

    async fn export_batch(&self, query: &str, session: &mut SessionManager) -> Result<ExportStats> {
        let result = session.execute_query(query).await?;
        let mut stats = ExportStats::new();

        let file = File::create(&self.config.file_path)?;
        let mut writer = BufWriter::new(file);

        match &self.config.format {
            ExportFormat::Json {
                pretty,
                array_wrapper,
            } => {
                if *array_wrapper {
                    writer.write_all(b"[\n")?;
                }

                for (idx, row) in result.rows.iter().enumerate() {
                    let obj = self.row_to_json_object(&result.columns, row);

                    let json_str = if *pretty {
                        serde_json::to_string_pretty(&obj)?
                    } else {
                        serde_json::to_string(&obj)?
                    };

                    if *array_wrapper && idx > 0 {
                        writer.write_all(b",\n")?;
                    }
                    writer.write_all(json_str.as_bytes())?;

                    stats.total_rows += 1;
                }

                if *array_wrapper {
                    writer.write_all(b"\n]")?;
                }
            }
            ExportFormat::JsonLines => {
                for row in &result.rows {
                    let obj = self.row_to_json_object(&result.columns, row);
                    let json_str = serde_json::to_string(&obj)?;
                    writeln!(writer, "{}", json_str)?;
                    stats.total_rows += 1;
                }
            }
            _ => return Err(anyhow::anyhow!("Invalid format for JSON exporter")),
        }

        writer.flush()?;
        stats.bytes_written = writer.get_ref().metadata()?.len();
        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;

        Ok(stats)
    }

    async fn export_streaming(
        &self,
        query: &str,
        session: &mut SessionManager,
    ) -> Result<ExportStats> {
        let mut stats = ExportStats::new();
        let chunk_size = self.config.chunk_size;

        let file = File::create(&self.config.file_path)?;
        let mut writer = BufWriter::new(file);

        let mut offset = 0;
        let mut first_chunk = true;

        match &self.config.format {
            ExportFormat::Json {
                pretty,
                array_wrapper,
            } => {
                if *array_wrapper {
                    writer.write_all(b"[\n")?;
                }

                loop {
                    let paginated_query = format!("{} SKIP {} LIMIT {}", query, offset, chunk_size);
                    let result = session.execute_query(&paginated_query).await?;

                    if result.rows.is_empty() {
                        break;
                    }

                    for (idx, row) in result.rows.iter().enumerate() {
                        let obj = self.row_to_json_object(&result.columns, row);

                        let json_str = if *pretty {
                            serde_json::to_string_pretty(&obj)?
                        } else {
                            serde_json::to_string(&obj)?
                        };

                        if *array_wrapper && (!first_chunk || idx > 0) {
                            writer.write_all(b",\n")?;
                        }
                        writer.write_all(json_str.as_bytes())?;

                        stats.total_rows += 1;
                    }

                    offset += result.rows.len();
                    first_chunk = false;

                    if result.rows.len() < chunk_size {
                        break;
                    }
                }

                if *array_wrapper {
                    writer.write_all(b"\n]")?;
                }
            }
            ExportFormat::JsonLines => loop {
                let paginated_query = format!("{} SKIP {} LIMIT {}", query, offset, chunk_size);
                let result = session.execute_query(&paginated_query).await?;

                if result.rows.is_empty() {
                    break;
                }

                for row in &result.rows {
                    let obj = self.row_to_json_object(&result.columns, row);
                    let json_str = serde_json::to_string(&obj)?;
                    writeln!(writer, "{}", json_str)?;
                    stats.total_rows += 1;
                }

                offset += result.rows.len();

                if result.rows.len() < chunk_size {
                    break;
                }
            },
            _ => return Err(anyhow::anyhow!("Invalid format for JSON exporter")),
        }

        writer.flush()?;
        stats.bytes_written = writer.get_ref().metadata()?.len();
        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;

        Ok(stats)
    }

    fn row_to_json_object(
        &self,
        columns: &[String],
        row: &HashMap<String, serde_json::Value>,
    ) -> serde_json::Value {
        let mut obj = serde_json::Map::new();
        for col in columns {
            let value = row.get(col).cloned().unwrap_or(serde_json::Value::Null);
            obj.insert(col.clone(), value);
        }
        serde_json::Value::Object(obj)
    }
}
