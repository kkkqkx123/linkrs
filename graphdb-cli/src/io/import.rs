use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ImportConfig {
    pub file_path: PathBuf,
    pub target_type: ImportTarget,
    pub format: ImportFormat,
    pub batch_size: usize,
    pub skip_rows: usize,
    pub field_mapping: Option<HashMap<String, String>>,
    pub on_error: ErrorHandling,
    pub encoding: String,
}

impl ImportConfig {
    pub fn new(file_path: PathBuf, target_type: ImportTarget) -> Self {
        Self {
            file_path,
            target_type,
            format: ImportFormat::default(),
            batch_size: 100,
            skip_rows: 0,
            field_mapping: None,
            on_error: ErrorHandling::Stop,
            encoding: "utf-8".to_string(),
        }
    }

    pub fn with_format(mut self, format: ImportFormat) -> Self {
        self.format = format;
        self
    }

    pub fn with_batch_size(mut self, size: usize) -> Self {
        self.batch_size = size;
        self
    }

    pub fn with_skip_rows(mut self, rows: usize) -> Self {
        self.skip_rows = rows;
        self
    }

    pub fn with_field_mapping(mut self, mapping: HashMap<String, String>) -> Self {
        self.field_mapping = Some(mapping);
        self
    }

    pub fn with_error_handling(mut self, handling: ErrorHandling) -> Self {
        self.on_error = handling;
        self
    }

    pub fn map_field_name(&self, original: &str) -> String {
        if let Some(mapping) = &self.field_mapping {
            mapping
                .get(original)
                .cloned()
                .unwrap_or_else(|| original.to_string())
        } else {
            original.to_string()
        }
    }

    pub fn format_value(&self, value: &str) -> String {
        if value.is_empty() {
            return "NULL".to_string();
        }

        format!("\"{}\"", value.replace('\"', "\\\""))
    }
}

#[derive(Debug, Clone)]
pub enum ImportTarget {
    Vertex { tag: String },
    Edge { edge_type: String },
}

impl ImportTarget {
    pub fn vertex(tag: &str) -> Self {
        ImportTarget::Vertex {
            tag: tag.to_string(),
        }
    }

    pub fn edge(edge_type: &str) -> Self {
        ImportTarget::Edge {
            edge_type: edge_type.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum ImportFormat {
    Csv { delimiter: char, has_header: bool },
    Json { array_mode: bool },
    JsonLines,
}

impl Default for ImportFormat {
    fn default() -> Self {
        ImportFormat::Csv {
            delimiter: ',',
            has_header: true,
        }
    }
}

impl ImportFormat {
    pub fn csv() -> Self {
        ImportFormat::Csv {
            delimiter: ',',
            has_header: true,
        }
    }

    pub fn csv_with_delimiter(delimiter: char) -> Self {
        ImportFormat::Csv {
            delimiter,
            has_header: true,
        }
    }

    pub fn json_array() -> Self {
        ImportFormat::Json { array_mode: true }
    }

    pub fn json_lines() -> Self {
        ImportFormat::Json { array_mode: false }
    }

    pub fn delimiter(&self) -> char {
        match self {
            ImportFormat::Csv { delimiter, .. } => *delimiter,
            _ => ',',
        }
    }

    pub fn has_header(&self) -> bool {
        match self {
            ImportFormat::Csv { has_header, .. } => *has_header,
            _ => false,
        }
    }

    pub fn is_array_mode(&self) -> bool {
        match self {
            ImportFormat::Json { array_mode } => *array_mode,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ErrorHandling {
    Stop,
    Skip,
    SkipAndLog(PathBuf),
}

#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    pub total_rows: usize,
    pub success_rows: usize,
    pub failed_rows: usize,
    pub skipped_rows: usize,
    pub duration_ms: u64,
    pub errors: Vec<ImportError>,
}

impl ImportStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn format_summary(&self) -> String {
        let mut output = String::new();

        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Import Statistics\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&format!("Total rows:      {}\n", self.total_rows));
        output.push_str(&format!("Success:         {}\n", self.success_rows));
        output.push_str(&format!("Failed:          {}\n", self.failed_rows));
        output.push_str(&format!("Skipped:         {}\n", self.skipped_rows));
        output.push_str(&format!(
            "Duration:        {:.3} s\n",
            self.duration_ms as f64 / 1000.0
        ));

        if self.duration_ms > 0 {
            let rate = self.success_rows as f64 / (self.duration_ms as f64 / 1000.0);
            output.push_str(&format!("Rate:            {:.0} rows/s\n", rate));
        }

        if !self.errors.is_empty() {
            output.push_str("\nErrors (showing first 5):\n");
            for err in self.errors.iter().take(5) {
                output.push_str(&format!("  Row {}: {}\n", err.row_number, err.error));
            }
            if self.errors.len() > 5 {
                output.push_str(&format!(
                    "  ... and {} more errors\n",
                    self.errors.len() - 5
                ));
            }
        }

        output
    }
}

#[derive(Debug, Clone)]
pub struct ImportError {
    pub row_number: usize,
    pub line: String,
    pub error: String,
}

impl ImportError {
    pub fn new(row_number: usize, line: String, error: String) -> Self {
        Self {
            row_number,
            line,
            error,
        }
    }
}
