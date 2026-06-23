use std::str::FromStr;

use crate::client::QueryResult;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutputFormat {
    Table,
    Vertical,
    CSV,
    JSON,
    HTML,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "table" => Ok(OutputFormat::Table),
            "vertical" | "expanded" => Ok(OutputFormat::Vertical),
            "csv" => Ok(OutputFormat::CSV),
            "json" => Ok(OutputFormat::JSON),
            "html" => Ok(OutputFormat::HTML),
            other => Err(format!("Unknown output format: {}", other)),
        }
    }
}

impl OutputFormat {
    pub fn parse(s: &str) -> Option<Self> {
        s.parse().ok()
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            OutputFormat::Table => "table",
            OutputFormat::Vertical => "vertical",
            OutputFormat::CSV => "csv",
            OutputFormat::JSON => "json",
            OutputFormat::HTML => "html",
        }
    }
}

pub struct OutputFormatter {
    format: OutputFormat,
    timing_enabled: bool,
    null_string: String,
}

impl Default for OutputFormatter {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputFormatter {
    pub fn new() -> Self {
        Self {
            format: OutputFormat::Table,
            timing_enabled: false,
            null_string: "NULL".to_string(),
        }
    }

    pub fn set_format(&mut self, format: OutputFormat) {
        self.format = format;
    }

    pub fn format(&self) -> OutputFormat {
        self.format
    }

    pub fn set_timing(&mut self, enabled: bool) {
        self.timing_enabled = enabled;
    }

    pub fn timing_enabled(&self) -> bool {
        self.timing_enabled
    }

    pub fn format_result(&self, result: &QueryResult) -> String {
        let formatted = match self.format {
            OutputFormat::Table => crate::output::table::format_table(result, &self.null_string),
            OutputFormat::Vertical => {
                crate::output::table::format_vertical(result, &self.null_string)
            }
            OutputFormat::CSV => crate::output::csv::format_csv(result, &self.null_string),
            OutputFormat::JSON => crate::output::json::format_json(result),
            OutputFormat::HTML => crate::output::table::format_table(result, &self.null_string),
        };

        let mut output = formatted;

        if self.timing_enabled && result.execution_time_ms > 0 {
            output.push_str(&format!("\nExecution time: {}ms", result.execution_time_ms));
        }

        output
    }

    pub fn format_error(&self, message: &str) -> String {
        use colored::Colorize;
        format!("{}: {}", "ERROR".red().bold(), message)
    }

    pub fn format_info(&self, message: &str) -> String {
        message.to_string()
    }

    pub fn format_spaces(&self, spaces: &[crate::client::SpaceInfo]) -> String {
        if spaces.is_empty() {
            return "(0 spaces)".to_string();
        }

        let mut builder = tabled::builder::Builder::default();
        builder.push_record(["ID", "Name", "VID Type", "Comment"]);

        for space in spaces {
            builder.push_record([
                space.id.to_string(),
                space.name.clone(),
                space.vid_type.clone(),
                space.comment.clone().unwrap_or_default(),
            ]);
        }

        let mut table = builder.build();
        table.with(tabled::settings::Style::rounded());

        format!("{}\n\n({} spaces)", table, spaces.len())
    }

    pub fn format_tags(&self, tags: &[crate::client::TagInfo]) -> String {
        if tags.is_empty() {
            return "(0 tags)".to_string();
        }

        let mut builder = tabled::builder::Builder::default();
        builder.push_record(["Tag Name", "Fields"]);

        for tag in tags {
            builder.push_record([tag.name.clone(), tag.fields.len().to_string()]);
        }

        let mut table = builder.build();
        table.with(tabled::settings::Style::rounded());

        format!("{}\n\n({} tags)", table, tags.len())
    }

    pub fn format_edge_types(&self, edge_types: &[crate::client::EdgeTypeInfo]) -> String {
        if edge_types.is_empty() {
            return "(0 edge types)".to_string();
        }

        let mut builder = tabled::builder::Builder::default();
        builder.push_record(["Edge Type", "Fields"]);

        for et in edge_types {
            builder.push_record([et.name.clone(), et.fields.len().to_string()]);
        }

        let mut table = builder.build();
        table.with(tabled::settings::Style::rounded());

        format!("{}\n\n({} edge types)", table, edge_types.len())
    }

    pub fn format_describe_tag(&self, tag: &crate::client::TagInfo) -> String {
        let mut output = format!("Tag: {}\n", tag.name);

        if tag.fields.is_empty() {
            output.push_str("(no fields)");
            return output;
        }

        let mut builder = tabled::builder::Builder::default();
        builder.push_record(["Field Name", "Type", "Nullable", "Default"]);

        for f in &tag.fields {
            builder.push_record([
                f.name.clone(),
                f.data_type.clone(),
                if f.nullable { "YES" } else { "NO" }.to_string(),
                f.default_value.clone().unwrap_or_else(|| "-".to_string()),
            ]);
        }

        let mut table = builder.build();
        table.with(tabled::settings::Style::rounded());

        output.push_str(&table.to_string());
        output
    }

    pub fn format_describe_edge(&self, edge: &crate::client::EdgeTypeInfo) -> String {
        let mut output = format!("Edge Type: {}\n", edge.name);

        if edge.fields.is_empty() {
            output.push_str("(no fields)");
            return output;
        }

        let mut builder = tabled::builder::Builder::default();
        builder.push_record(["Field Name", "Type", "Nullable", "Default"]);

        for f in &edge.fields {
            builder.push_record([
                f.name.clone(),
                f.data_type.clone(),
                if f.nullable { "YES" } else { "NO" }.to_string(),
                f.default_value.clone().unwrap_or_else(|| "-".to_string()),
            ]);
        }

        let mut table = builder.build();
        table.with(tabled::settings::Style::rounded());

        output.push_str(&table.to_string());
        output
    }
}
