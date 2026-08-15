use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::time::Instant;

use anyhow::Result;

use crate::io::{
    BatchProcessor, ErrorHandling, ImportConfig, ImportError, ImportStats, ImportTarget,
};
use crate::session::manager::SessionManager;

pub struct JsonImporter {
    config: ImportConfig,
    batch_processor: BatchProcessor,
    start_time: Instant,
}

impl JsonImporter {
    pub fn new(config: ImportConfig) -> Self {
        let batch_processor = BatchProcessor::new(config.batch_size);
        Self {
            config,
            batch_processor,
            start_time: Instant::now(),
        }
    }

    pub async fn import(&mut self, session: &mut SessionManager) -> Result<ImportStats> {
        let content = fs::read_to_string(&self.config.file_path)?;
        let mut stats = ImportStats::new();

        if self.config.format.is_array_mode() {
            let items: Vec<serde_json::Value> = serde_json::from_str(&content)?;
            for (idx, item) in items.iter().enumerate() {
                stats.total_rows += 1;
                match self.process_json_item(item, session).await {
                    Ok(_) => stats.success_rows += 1,
                    Err(e) => {
                        stats.failed_rows += 1;
                        stats
                            .errors
                            .push(ImportError::new(idx, item.to_string(), e.to_string()));

                        if matches!(self.config.on_error, ErrorHandling::Stop) {
                            self.batch_processor.flush(session).await?;
                            return Err(e);
                        }
                    }
                }
            }
        } else {
            let file = File::open(&self.config.file_path)?;
            let reader = BufReader::new(file);

            for (idx, line) in reader.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }

                stats.total_rows += 1;
                let item: serde_json::Value = serde_json::from_str(&line)?;
                match self.process_json_item(&item, session).await {
                    Ok(_) => stats.success_rows += 1,
                    Err(e) => {
                        stats.failed_rows += 1;
                        stats
                            .errors
                            .push(ImportError::new(idx, line.clone(), e.to_string()));

                        if matches!(self.config.on_error, ErrorHandling::Stop) {
                            self.batch_processor.flush(session).await?;
                            return Err(e);
                        }
                    }
                }
            }
        }

        self.batch_processor.flush(session).await?;
        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;
        Ok(stats)
    }

    async fn process_json_item(
        &mut self,
        json: &serde_json::Value,
        session: &mut SessionManager,
    ) -> Result<()> {
        let query = self.build_insert_from_json(json)?;
        if self.batch_processor.add(query) {
            self.batch_processor.flush(session).await?;
        }

        Ok(())
    }

    fn build_insert_from_json(&self, json: &serde_json::Value) -> Result<String> {
        let obj = json
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("Expected JSON object"))?;

        match &self.config.target_type {
            ImportTarget::Vertex { tag } => {
                let vid = obj
                    .get("_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing _id field"))?;

                let mut fields = Vec::new();
                let mut values = Vec::new();

                for (key, value) in obj.iter() {
                    if key == "_id" {
                        continue;
                    }
                    fields.push(self.config.map_field_name(key));
                    values.push(json_value_to_gql(value));
                }

                Ok(format!(
                    "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                    tag,
                    fields.join(", "),
                    vid,
                    values.join(", ")
                ))
            }
            ImportTarget::Edge { edge_type } => {
                let src = obj
                    .get("_src")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing _src field"))?;
                let dst = obj
                    .get("_dst")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing _dst field"))?;

                let mut fields = Vec::new();
                let mut values = Vec::new();

                for (key, value) in obj.iter() {
                    if key == "_src" || key == "_dst" {
                        continue;
                    }
                    fields.push(self.config.map_field_name(key));
                    values.push(json_value_to_gql(value));
                }

                Ok(format!(
                    "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
                    edge_type,
                    fields.join(", "),
                    src,
                    dst,
                    values.join(", ")
                ))
            }
        }
    }
}

fn json_value_to_gql(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("\"{}\"", s.replace('\"', "\\\"")),
        serde_json::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(json_value_to_gql).collect();
            format!("[{}]", items.join(", "))
        }
        serde_json::Value::Object(_) => {
            format!("\"{}\"", serde_json::to_string(value).unwrap_or_default())
        }
    }
}
