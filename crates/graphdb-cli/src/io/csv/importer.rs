use std::fs::File;
use std::io::BufReader;
use std::time::Instant;

use anyhow::Result;
use csv::ReaderBuilder;

use crate::io::{
    BatchProcessor, ErrorHandling, ImportConfig, ImportError, ImportStats, ImportTarget,
};
use crate::session::manager::SessionManager;

pub struct CsvImporter {
    config: ImportConfig,
    batch_processor: BatchProcessor,
    start_time: Instant,
}

impl CsvImporter {
    pub fn new(config: ImportConfig) -> Self {
        let batch_processor = BatchProcessor::new(config.batch_size);
        Self {
            config,
            batch_processor,
            start_time: Instant::now(),
        }
    }

    pub async fn import(&mut self, session: &mut SessionManager) -> Result<ImportStats> {
        let file = File::open(&self.config.file_path)?;
        let reader = BufReader::new(file);

        let mut csv_reader = ReaderBuilder::new()
            .delimiter(self.config.format.delimiter() as u8)
            .has_headers(self.config.format.has_header())
            .from_reader(reader);

        let headers = csv_reader.headers()?.clone();
        let mut stats = ImportStats::new();

        for (idx, result) in csv_reader.records().enumerate() {
            if idx < self.config.skip_rows {
                stats.skipped_rows += 1;
                continue;
            }

            stats.total_rows += 1;

            match result {
                Ok(record) => match self.process_record(&headers, &record, session).await {
                    Ok(_) => stats.success_rows += 1,
                    Err(e) => {
                        stats.failed_rows += 1;
                        stats.errors.push(ImportError::new(
                            idx,
                            record.iter().collect::<Vec<_>>().join(","),
                            e.to_string(),
                        ));

                        if matches!(self.config.on_error, ErrorHandling::Stop) {
                            self.batch_processor.flush(session).await?;
                            return Err(e);
                        }
                    }
                },
                Err(e) => {
                    stats.failed_rows += 1;
                    if matches!(self.config.on_error, ErrorHandling::Stop) {
                        self.batch_processor.flush(session).await?;
                        return Err(e.into());
                    }
                }
            }
        }

        self.batch_processor.flush(session).await?;
        stats.duration_ms = self.start_time.elapsed().as_millis() as u64;
        Ok(stats)
    }

    async fn process_record(
        &mut self,
        headers: &csv::StringRecord,
        record: &csv::StringRecord,
        session: &mut SessionManager,
    ) -> Result<()> {
        let query = self.build_insert_query(headers, record)?;
        if self.batch_processor.add(query) {
            self.batch_processor.flush(session).await?;
        }

        Ok(())
    }

    fn build_insert_query(
        &self,
        headers: &csv::StringRecord,
        record: &csv::StringRecord,
    ) -> Result<String> {
        match &self.config.target_type {
            ImportTarget::Vertex { tag } => {
                let fields: Vec<String> = headers
                    .iter()
                    .map(|h| self.config.map_field_name(h))
                    .collect();
                let values: Vec<String> =
                    record.iter().map(|v| self.config.format_value(v)).collect();

                let vid = self.generate_vid(record)?;

                Ok(format!(
                    "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                    tag,
                    fields.join(", "),
                    vid,
                    values.join(", ")
                ))
            }
            ImportTarget::Edge { edge_type } => {
                let src_vid = record
                    .get(0)
                    .ok_or_else(|| anyhow::anyhow!("Missing source VID"))?;
                let dst_vid = record
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Missing destination VID"))?;

                let fields: Vec<String> = headers
                    .iter()
                    .skip(2)
                    .map(|h| self.config.map_field_name(h))
                    .collect();
                let values: Vec<String> = record
                    .iter()
                    .skip(2)
                    .map(|v| self.config.format_value(v))
                    .collect();

                Ok(format!(
                    "INSERT EDGE {} ({}) VALUES \"{}\"->\"{}\":({})",
                    edge_type,
                    fields.join(", "),
                    src_vid,
                    dst_vid,
                    values.join(", ")
                ))
            }
        }
    }

    fn generate_vid(&self, record: &csv::StringRecord) -> Result<String> {
        if let Some(vid) = record.get(0) {
            Ok(vid.to_string())
        } else {
            Ok(format!("vid_{}", uuid::Uuid::new_v4()))
        }
    }
}
