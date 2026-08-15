use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ExportConfig {
    pub file_path: PathBuf,
    pub format: ExportFormat,
    pub encoding: String,
    pub include_header: bool,
    pub append_mode: bool,
    pub streaming: bool,
    pub chunk_size: usize,
}

impl ExportConfig {
    pub fn new(file_path: PathBuf, format: ExportFormat) -> Self {
        Self {
            file_path,
            format,
            encoding: "utf-8".to_string(),
            include_header: true,
            append_mode: false,
            streaming: false,
            chunk_size: 1000,
        }
    }

    pub fn with_include_header(mut self, include: bool) -> Self {
        self.include_header = include;
        self
    }

    pub fn with_append_mode(mut self, append: bool) -> Self {
        self.append_mode = append;
        self
    }

    pub fn with_encoding(mut self, encoding: &str) -> Self {
        self.encoding = encoding.to_string();
        self
    }

    pub fn with_streaming(mut self, streaming: bool) -> Self {
        self.streaming = streaming;
        self
    }

    pub fn with_chunk_size(mut self, chunk_size: usize) -> Self {
        self.chunk_size = chunk_size;
        self
    }
}

#[derive(Debug, Clone)]
pub enum ExportFormat {
    Csv { delimiter: char },
    Json { pretty: bool, array_wrapper: bool },
    JsonLines,
}

impl ExportFormat {
    pub fn csv() -> Self {
        ExportFormat::Csv { delimiter: ',' }
    }

    pub fn csv_with_delimiter(delimiter: char) -> Self {
        ExportFormat::Csv { delimiter }
    }

    pub fn json() -> Self {
        ExportFormat::Json {
            pretty: false,
            array_wrapper: true,
        }
    }

    pub fn json_pretty() -> Self {
        ExportFormat::Json {
            pretty: true,
            array_wrapper: true,
        }
    }

    pub fn json_lines() -> Self {
        ExportFormat::JsonLines
    }

    pub fn delimiter(&self) -> char {
        match self {
            ExportFormat::Csv { delimiter } => *delimiter,
            _ => ',',
        }
    }

    pub fn is_pretty(&self) -> bool {
        match self {
            ExportFormat::Json { pretty, .. } => *pretty,
            _ => false,
        }
    }

    pub fn has_array_wrapper(&self) -> bool {
        match self {
            ExportFormat::Json { array_wrapper, .. } => *array_wrapper,
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExportStats {
    pub total_rows: usize,
    pub bytes_written: u64,
    pub duration_ms: u64,
}

impl ExportStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn format_summary(&self) -> String {
        let mut output = String::new();

        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str("Export Statistics\n");
        output.push_str("─────────────────────────────────────────────────────────────\n");
        output.push_str(&format!("Total rows:      {}\n", self.total_rows));
        output.push_str(&format!(
            "Bytes written:   {}\n",
            format_bytes(self.bytes_written)
        ));
        output.push_str(&format!(
            "Duration:        {:.3} s\n",
            self.duration_ms as f64 / 1000.0
        ));

        if self.duration_ms > 0 {
            let rate = self.total_rows as f64 / (self.duration_ms as f64 / 1000.0);
            output.push_str(&format!("Rate:            {:.0} rows/s\n", rate));
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
