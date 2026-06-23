# 数据导入导出设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供数据导入导出功能，支持 CSV、JSON 等常见格式，方便用户批量导入数据和备份数据。

### 1.2 参考实现

- **psql**：`\copy` 命令，支持 CSV、TEXT 格式
- **MySQL**：`LOAD DATA INFILE`、`SELECT ... INTO OUTFILE`
- **neo4j-admin**：`neo4j-admin import`，支持 CSV 批量导入
- **MongoDB**：`mongoimport`、`mongoexport`

## 2. 功能需求

### 2.1 导入功能

| 功能              | 说明                                       |
| ----------------- | ------------------------------------------ |
| CSV 导入          | 从 CSV 文件导入顶点和边数据                |
| JSON 导入         | 从 JSON 文件导入数据                       |
| 批量导入          | 支持批量提交，提高导入效率                 |
| 字段映射          | 支持文件列名到属性的映射                   |
| 错误处理          | 支持跳过错误行、错误日志记录               |
| 进度显示          | 显示导入进度和统计信息                     |

### 2.2 导出功能

| 功能              | 说明                                       |
| ----------------- | ------------------------------------------ |
| CSV 导出          | 将查询结果导出为 CSV 文件                  |
| JSON 导出         | 将查询结果导出为 JSON 文件                 |
| 格式化选项        | 支持自定义分隔符、编码、是否包含表头等     |
| 大数据量导出      | 支持流式导出，避免内存溢出                 |

### 2.3 元命令

| 命令                           | 说明                           |
| ------------------------------ | ------------------------------ |
| `\import csv <file> <tag>`     | 导入 CSV 到指定 Tag            |
| `\import json <file> <tag>`    | 导入 JSON 到指定 Tag           |
| `\import edge csv <file> <edge>` | 导入 CSV 边数据              |
| `\export csv <file> <query>`   | 导出查询结果到 CSV             |
| `\export json <file> <query>`  | 导出查询结果到 JSON            |
| `\copy <tag> from '<file>'`    | psql 风格的导入命令            |
| `\copy <query> to '<file>'`    | psql 风格的导出命令            |

## 3. 架构设计

### 3.1 模块结构

```
src/
├── io/
│   ├── mod.rs              # 模块导出
│   ├── import.rs           # 导入功能
│   ├── export.rs           # 导出功能
│   ├── csv_handler.rs      # CSV 格式处理
│   ├── json_handler.rs     # JSON 格式处理
│   └── progress.rs         # 进度显示
└── command/
    └── executor.rs         # 集成导入导出命令
```

### 3.2 核心数据结构

```rust
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

pub enum ImportTarget {
    Vertex { tag: String },
    Edge { edge_type: String },
}

pub enum ImportFormat {
    Csv { delimiter: char, has_header: bool },
    Json { array_mode: bool },
}

pub enum ErrorHandling {
    Stop,
    Skip,
    SkipAndLog(PathBuf),
}

pub struct ImportStats {
    pub total_rows: usize,
    pub success_rows: usize,
    pub failed_rows: usize,
    pub skipped_rows: usize,
    pub duration_ms: u64,
    pub errors: Vec<ImportError>,
}

pub struct ImportError {
    pub row_number: usize,
    pub line: String,
    pub error: String,
}

pub struct ExportConfig {
    pub file_path: PathBuf,
    pub format: ExportFormat,
    pub encoding: String,
    pub include_header: bool,
    pub append_mode: bool,
}

pub enum ExportFormat {
    Csv { delimiter: char },
    Json { pretty: bool, array_wrapper: bool },
    JsonLines,
}

pub struct ExportStats {
    pub total_rows: usize,
    pub bytes_written: u64,
    pub duration_ms: u64,
}
```

## 4. CSV 导入实现

### 4.1 CSV 解析器

```rust
use csv::ReaderBuilder;

pub struct CsvImporter {
    config: ImportConfig,
    batch_buffer: Vec<String>,
}

impl CsvImporter {
    pub fn new(config: ImportConfig) -> Self {
        Self {
            config,
            batch_buffer: Vec::new(),
        }
    }

    pub async fn import(
        &mut self,
        session: &mut SessionManager,
    ) -> Result<ImportStats> {
        let file = File::open(&self.config.file_path)?;
        let reader = BufReader::new(file);
        
        let mut csv_reader = ReaderBuilder::new()
            .delimiter(self.config.format.delimiter() as u8)
            .has_headers(self.config.format.has_header())
            .from_reader(reader);
        
        let headers = csv_reader.headers()?.clone();
        let mut stats = ImportStats::default();
        
        for (idx, result) in csv_reader.records().enumerate() {
            if idx < self.config.skip_rows {
                stats.skipped_rows += 1;
                continue;
            }
            
            match result {
                Ok(record) => {
                    match self.process_record(&headers, &record, session).await {
                        Ok(_) => stats.success_rows += 1,
                        Err(e) => {
                            stats.failed_rows += 1;
                            stats.errors.push(ImportError {
                                row_number: idx,
                                line: record.iter().join(","),
                                error: e.to_string(),
                            });
                            
                            if matches!(self.config.on_error, ErrorHandling::Stop) {
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    stats.failed_rows += 1;
                    if matches!(self.config.on_error, ErrorHandling::Stop) {
                        return Err(e.into());
                    }
                }
            }
            
            stats.total_rows += 1;
            self.show_progress(&stats);
        }
        
        self.flush_batch(session).await?;
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
        self.batch_buffer.push(query);
        
        if self.batch_buffer.len() >= self.config.batch_size {
            self.flush_batch(session).await?;
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
                let fields: Vec<String> = headers.iter()
                    .map(|h| self.map_field_name(h))
                    .collect();
                let values: Vec<String> = record.iter()
                    .map(|v| self.format_value(v))
                    .collect();
                
                Ok(format!(
                    "INSERT VERTEX {} ({}) VALUES \"{}\":({})",
                    tag,
                    fields.join(", "),
                    self.generate_vid(record)?,
                    values.join(", ")
                ))
            }
            ImportTarget::Edge { edge_type } => {
                let src_vid = record.get(0).ok_or_else(|| anyhow!("Missing source VID"))?;
                let dst_vid = record.get(1).ok_or_else(|| anyhow!("Missing destination VID"))?;
                let fields: Vec<String> = headers.iter().skip(2)
                    .map(|h| self.map_field_name(h))
                    .collect();
                let values: Vec<String> = record.iter().skip(2)
                    .map(|v| self.format_value(v))
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
}
```

### 4.2 字段映射

```rust
impl ImportConfig {
    fn map_field_name(&self, original: &str) -> String {
        if let Some(mapping) = &self.field_mapping {
            mapping.get(original).cloned().unwrap_or_else(|| original.to_string())
        } else {
            original.to_string()
        }
    }

    fn format_value(&self, value: &str) -> String {
        if value.is_empty() {
            "NULL".to_string()
        } else if value.parse::<i64>().is_ok() {
            value.to_string()
        } else if value.parse::<f64>().is_ok() {
            value.to_string()
        } else if value == "true" || value == "false" {
            value.to_string()
        } else {
            format!("\"{}\"", value.replace('\"', "\\\""))
        }
    }
}
```

## 5. JSON 导入实现

### 5.1 JSON 解析器

```rust
pub struct JsonImporter {
    config: ImportConfig,
    batch_buffer: Vec<String>,
}

impl JsonImporter {
    pub async fn import(
        &mut self,
        session: &mut SessionManager,
    ) -> Result<ImportStats> {
        let content = fs::read_to_string(&self.config.file_path)?;
        let mut stats = ImportStats::default();
        
        if self.config.format.array_mode() {
            let items: Vec<serde_json::Value> = serde_json::from_str(&content)?;
            for (idx, item) in items.iter().enumerate() {
                match self.process_json_item(item, session).await {
                    Ok(_) => stats.success_rows += 1,
                    Err(e) => {
                        stats.failed_rows += 1;
                        stats.errors.push(ImportError {
                            row_number: idx,
                            line: item.to_string(),
                            error: e.to_string(),
                        });
                    }
                }
                stats.total_rows += 1;
            }
        } else {
            let file = File::open(&self.config.file_path)?;
            let reader = BufReader::new(file);
            
            for (idx, line) in reader.lines().enumerate() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                
                let item: serde_json::Value = serde_json::from_str(&line)?;
                match self.process_json_item(&item, session).await {
                    Ok(_) => stats.success_rows += 1,
                    Err(e) => {
                        stats.failed_rows += 1;
                        stats.errors.push(ImportError {
                            row_number: idx,
                            line: line.clone(),
                            error: e.to_string(),
                        });
                    }
                }
                stats.total_rows += 1;
            }
        }
        
        self.flush_batch(session).await?;
        Ok(stats)
    }

    fn build_insert_from_json(
        &self,
        json: &serde_json::Value,
    ) -> Result<String> {
        let obj = json.as_object().ok_or_else(|| anyhow!("Expected JSON object"))?;
        
        match &self.config.target_type {
            ImportTarget::Vertex { tag } => {
                let vid = obj.get("_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing _id field"))?;
                
                let mut fields = Vec::new();
                let mut values = Vec::new();
                
                for (key, value) in obj.iter() {
                    if key == "_id" {
                        continue;
                    }
                    fields.push(key.clone());
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
                let src = obj.get("_src")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing _src field"))?;
                let dst = obj.get("_dst")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow!("Missing _dst field"))?;
                
                let mut fields = Vec::new();
                let mut values = Vec::new();
                
                for (key, value) in obj.iter() {
                    if key == "_src" || key == "_dst" {
                        continue;
                    }
                    fields.push(key.clone());
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
```

## 6. 导出功能实现

### 6.1 CSV 导出

```rust
pub struct CsvExporter {
    config: ExportConfig,
}

impl CsvExporter {
    pub async fn export(
        &self,
        query: &str,
        session: &mut SessionManager,
    ) -> Result<ExportStats> {
        let result = session.execute_query(query).await?;
        let mut stats = ExportStats::default();
        
        let file = File::create(&self.config.file_path)?;
        let mut writer = BufWriter::new(file);
        
        let delimiter = self.config.format.delimiter();
        
        if self.config.include_header {
            let header = result.columns.join(&delimiter.to_string());
            writeln!(writer, "{}", header)?;
        }
        
        for row in &result.rows {
            let values: Vec<String> = result.columns.iter()
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
```

### 6.2 JSON 导出

```rust
pub struct JsonExporter {
    config: ExportConfig,
}

impl JsonExporter {
    pub async fn export(
        &self,
        query: &str,
        session: &mut SessionManager,
    ) -> Result<ExportStats> {
        let result = session.execute_query(query).await?;
        let mut stats = ExportStats::default();
        
        let file = File::create(&self.config.file_path)?;
        let mut writer = BufWriter::new(file);
        
        match &self.config.format {
            ExportFormat::Json { pretty, array_wrapper } => {
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
            _ => return Err(anyhow!("Invalid format for JSON exporter")),
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
```

## 7. 进度显示

### 7.1 进度条实现

```rust
pub struct ProgressBar {
    total: usize,
    current: usize,
    start_time: std::time::Instant,
    width: usize,
}

impl ProgressBar {
    pub fn new(total: usize) -> Self {
        Self {
            total,
            current: 0,
            start_time: std::time::Instant::now(),
            width: 50,
        }
    }

    pub fn update(&mut self, current: usize) {
        self.current = current;
        self.render();
    }

    pub fn increment(&mut self) {
        self.current += 1;
        self.render();
    }

    fn render(&self) {
        let percent = if self.total > 0 {
            self.current as f64 / self.total as f64
        } else {
            0.0
        };
        
        let filled = (percent * self.width as f64) as usize;
        let empty = self.width - filled;
        
        let elapsed = self.start_time.elapsed().as_secs();
        let rate = if elapsed > 0 {
            self.current as f64 / elapsed as f64
        } else {
            0.0
        };
        
        eprint!(
            "\r[{}{}] {}/{} ({:.1}%) {:.0} rows/s   ",
            "=".repeat(filled),
            " ".repeat(empty),
            self.current,
            self.total,
            percent * 100.0,
            rate
        );
        
        if self.current >= self.total {
            eprintln!();
        }
    }
}
```

## 8. 元命令实现

### 8.1 命令解析

```rust
pub enum MetaCommand {
    Import {
        format: ImportFormat,
        file_path: String,
        target: ImportTarget,
        options: ImportOptions,
    },
    Export {
        format: ExportFormat,
        file_path: String,
        query: String,
        options: ExportOptions,
    },
    Copy {
        direction: CopyDirection,
        target: String,
        file_path: String,
        options: CopyOptions,
    },
}

pub enum CopyDirection {
    From,
    To,
}

fn parse_import_command(input: &str) -> Result<MetaCommand> {
    let parts: Vec<&str> = input.split_whitespace().collect();
    
    if parts.len() < 4 {
        return Err(anyhow!("Usage: \\import <format> <file> <tag|edge> <name> [options]"));
    }
    
    let format = match parts[1].to_lowercase().as_str() {
        "csv" => ImportFormat::Csv { delimiter: ',', has_header: true },
        "json" => ImportFormat::Json { array_mode: true },
        "jsonl" => ImportFormat::Json { array_mode: false },
        _ => return Err(anyhow!("Unsupported format: {}", parts[1])),
    };
    
    let file_path = parts[2].to_string();
    
    let target = match parts[3].to_lowercase().as_str() {
        "tag" | "vertex" => ImportTarget::Vertex { tag: parts[4].to_string() },
        "edge" => ImportTarget::Edge { edge_type: parts[4].to_string() },
        _ => return Err(anyhow!("Invalid target type: {}", parts[3])),
    };
    
    let options = parse_import_options(&parts[5..])?;
    
    Ok(MetaCommand::Import { format, file_path, target, options })
}
```

### 8.2 命令执行

```rust
async fn execute_meta(&mut self, meta: MetaCommand, session_mgr: &mut SessionManager) -> Result<bool> {
    match meta {
        MetaCommand::Import { format, file_path, target, options } => {
            if !self.conditional_stack.is_active() {
                return Ok(true);
            }
            
            let config = ImportConfig {
                file_path: PathBuf::from(&file_path),
                target_type: target,
                format,
                batch_size: options.batch_size.unwrap_or(100),
                skip_rows: options.skip_rows.unwrap_or(0),
                field_mapping: options.field_mapping,
                on_error: options.on_error.unwrap_or(ErrorHandling::Stop),
                encoding: options.encoding.unwrap_or_else(|| "utf-8".to_string()),
            };
            
            let stats = match format {
                ImportFormat::Csv { .. } => {
                    let mut importer = CsvImporter::new(config);
                    importer.import(session_mgr).await?
                }
                ImportFormat::Json { .. } => {
                    let mut importer = JsonImporter::new(config);
                    importer.import(session_mgr).await?
                }
            };
            
            self.write_output(&format_import_stats(&stats))?;
            Ok(true)
        }
        
        MetaCommand::Export { format, file_path, query, options } => {
            if !self.conditional_stack.is_active() {
                return Ok(true);
            }
            
            let config = ExportConfig {
                file_path: PathBuf::from(&file_path),
                format,
                encoding: options.encoding.unwrap_or_else(|| "utf-8".to_string()),
                include_header: options.include_header.unwrap_or(true),
                append_mode: options.append.unwrap_or(false),
            };
            
            let stats = match &config.format {
                ExportFormat::Csv { .. } => {
                    let exporter = CsvExporter { config };
                    exporter.export(&query, session_mgr).await?
                }
                ExportFormat::Json { .. } | ExportFormat::JsonLines => {
                    let exporter = JsonExporter { config };
                    exporter.export(&query, session_mgr).await?
                }
            };
            
            self.write_output(&format_export_stats(&stats))?;
            Ok(true)
        }
        
        _ => self.execute_other_meta(meta, session_mgr).await,
    }
}
```

## 9. 统计信息格式化

```rust
fn format_import_stats(stats: &ImportStats) -> String {
    let mut output = String::new();
    
    output.push_str("─────────────────────────────────────────────────────────────\n");
    output.push_str("Import Statistics\n");
    output.push_str("─────────────────────────────────────────────────────────────\n");
    output.push_str(&format!("Total rows:      {}\n", stats.total_rows));
    output.push_str(&format!("Success:         {}\n", stats.success_rows));
    output.push_str(&format!("Failed:          {}\n", stats.failed_rows));
    output.push_str(&format!("Skipped:         {}\n", stats.skipped_rows));
    output.push_str(&format!("Duration:        {:.3} s\n", stats.duration_ms as f64 / 1000.0));
    
    if stats.total_rows > 0 {
        let rate = stats.success_rows as f64 / (stats.duration_ms as f64 / 1000.0);
        output.push_str(&format!("Rate:            {:.0} rows/s\n", rate));
    }
    
    if !stats.errors.is_empty() {
        output.push_str(&format!("\nErrors (showing first 5):\n"));
        for err in stats.errors.iter().take(5) {
            output.push_str(&format!("  Row {}: {}\n", err.row_number, err.error));
        }
        if stats.errors.len() > 5 {
            output.push_str(&format!("  ... and {} more errors\n", stats.errors.len() - 5));
        }
    }
    
    output
}

fn format_export_stats(stats: &ExportStats) -> String {
    let mut output = String::new();
    
    output.push_str("─────────────────────────────────────────────────────────────\n");
    output.push_str("Export Statistics\n");
    output.push_str("─────────────────────────────────────────────────────────────\n");
    output.push_str(&format!("Total rows:      {}\n", stats.total_rows));
    output.push_str(&format!("Bytes written:   {}\n", format_bytes(stats.bytes_written)));
    output.push_str(&format!("Duration:        {:.3} s\n", stats.duration_ms as f64 / 1000.0));
    
    if stats.duration_ms > 0 {
        let rate = stats.total_rows as f64 / (stats.duration_ms as f64 / 1000.0);
        output.push_str(&format!("Rate:            {:.0} rows/s\n", rate));
    }
    
    output
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
```

## 10. 测试用例

### 10.1 CSV 导入

| 输入文件                          | 预期结果                       |
| --------------------------------- | ------------------------------ |
| 标准 CSV 文件                     | 成功导入，显示统计信息         |
| 包含特殊字符的 CSV                | 正确处理引号和转义             |
| 字段映射配置                      | 按映射导入                     |
| 错误行处理（Skip 模式）           | 跳过错误行继续导入             |
| 错误行处理（Stop 模式）           | 遇到错误停止导入               |

### 10.2 JSON 导入

| 输入文件                          | 预期结果                       |
| --------------------------------- | ------------------------------ |
| JSON 数组文件                     | 成功导入所有元素               |
| JSON Lines 文件                   | 逐行解析导入                   |
| 嵌套 JSON 对象                    | 序列化为字符串存储             |

### 10.3 导出功能

| 操作                              | 预期结果                       |
| --------------------------------- | ------------------------------ |
| 导出查询结果为 CSV                | 生成正确的 CSV 文件            |
| 导出查询结果为 JSON               | 生成正确的 JSON 文件           |
| 大数据量导出                      | 流式导出，内存稳定             |

## 11. 实现步骤

### Step 1: 添加依赖（0.5 天）

- 添加 `csv` crate
- 添加 `serde_json`（已有）

### Step 2: 实现导入框架（1 天）

- 定义数据结构
- 实现 `ImportConfig`
- 实现批量提交逻辑

### Step 3: 实现 CSV 导入（1.5 天）

- 实现 CSV 解析
- 实现字段映射
- 实现错误处理

### Step 4: 实现 JSON 导入（1 天）

- 实现 JSON 数组解析
- 实现 JSON Lines 解析
- 实现 JSON 到 GQL 值转换

### Step 5: 实现导出功能（1.5 天）

- 实现 CSV 导出
- 实现 JSON 导出
- 实现流式导出

### Step 6: 实现元命令（1 天）

- 添加命令解析
- 集成到命令执行器
- 实现进度显示

### Step 7: 测试（1 天）

- 单元测试
- 集成测试
- 性能测试
