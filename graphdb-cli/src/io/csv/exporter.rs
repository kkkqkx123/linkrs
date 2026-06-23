use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use anyhow::Result;

use crate::io::{ExportConfig, ExportStats};
use crate::session::manager::SessionManager;

pub struct CsvExporter {
    config: ExportConfig,
    start_time: Instant,
}

impl CsvExporter {
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

        let delimiter = self.config.format.delimiter();

        if self.config.include_header {
            let header = result.columns.join(&delimiter.to_string());
            writeln!(writer, "{}", header)?;
        }

        for row in &result.rows {
            let values: Vec<String> = result
                .columns
                .iter()
                .map(|col| {
                    row.get(col)
                        .map(|v| self.format_csv_value(v))
                        .unwrap_or_default()
                })
                .collect();

            writeln!(writer, "{}", values.join(&delimiter.to_string()))?;
            stats.total_rows += 1;
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
        let mut columns: Option<Vec<String>> = None;
        let delimiter = self.config.format.delimiter();

        loop {
            let paginated_query = format!("{} SKIP {} LIMIT {}", query, offset, chunk_size);
            let result = session.execute_query(&paginated_query).await?;

            if result.rows.is_empty() {
                break;
            }

            if columns.is_none() {
                columns = Some(result.columns.clone());
                if self.config.include_header {
                    let header = result.columns.join(&delimiter.to_string());
                    writeln!(writer, "{}", header)?;
                }
            }

            for row in &result.rows {
                let values: Vec<String> = result
                    .columns
                    .iter()
                    .map(|col| {
                        row.get(col)
                            .map(|v| self.format_csv_value(v))
                            .unwrap_or_default()
                    })
                    .collect();

                writeln!(writer, "{}", values.join(&delimiter.to_string()))?;
                stats.total_rows += 1;
            }

            offset += result.rows.len();

            if result.rows.len() < chunk_size {
                break;
            }
        }

        writer.flush()?;
        stats.bytes_written = writer.get_ref().metadata()?.len();
        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;

        Ok(stats)
    }

    fn format_csv_value(&self, value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => String::new(),
            serde_json::Value::Bool(b) => b.to_string(),
            serde_json::Value::Number(n) => n.to_string(),
            serde_json::Value::String(s) => {
                if s.contains(',') || s.contains('"') || s.contains('\n') {
                    format!("\"{}\"", s.replace('\"', "\"\""))
                } else {
                    s.clone()
                }
            }
            serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
                serde_json::to_string(value).unwrap_or_default()
            }
        }
    }
}
