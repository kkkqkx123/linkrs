//! CLI dump configuration

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CliDumpConfig {
    pub database: String,
    pub output_path: PathBuf,
    pub format: CliDumpFormat,
    pub compress: bool,
    pub include_schema: bool,
    pub include_data: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliDumpFormat {
    Binary,
    JsonLines,
}

impl Default for CliDumpConfig {
    fn default() -> Self {
        Self {
            database: String::new(),
            output_path: PathBuf::from("dump"),
            format: CliDumpFormat::Binary,
            compress: true,
            include_schema: true,
            include_data: true,
        }
    }
}

impl CliDumpConfig {
    pub fn format_extension(&self) -> &str {
        match self.format {
            CliDumpFormat::Binary => "dump",
            CliDumpFormat::JsonLines => "jsonl",
        }
    }

    pub fn output_with_extension(&self) -> PathBuf {
        let mut path = self.output_path.clone();
        if let Some(stem) = path.file_stem() {
            let mut new_name = stem.to_os_string();
            new_name.push(".");
            new_name.push(self.format_extension());
            if self.compress {
                new_name.push(".zst");
            }
            path.set_file_name(new_name);
        }
        path
    }
}
