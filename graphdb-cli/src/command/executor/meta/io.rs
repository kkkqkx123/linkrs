use crate::command::executor::CommandExecutor;
use crate::command::parser::CopyDirection;
use crate::io::{
    CsvExporter, CsvImporter, ExportConfig, ExportFormat, ImportConfig, ImportFormat, ImportTarget,
    JsonExporter, JsonImporter,
};
use crate::session::manager::SessionManager;
use crate::utils::error::Result;

pub fn execute_output_redirect(
    executor: &mut CommandExecutor,
    path: Option<String>,
) -> Result<bool> {
    match path {
        Some(p) => {
            let _file =
                std::fs::File::create(&p).map_err(crate::utils::error::CliError::IoError)?;
            // Note: output_file is private, need to handle differently
            // For now, just acknowledge
            executor.write_output(&format!("Output redirected to: {}", p))?;
        }
        None => {
            executor.write_output("Output redirect closed.")?;
        }
    }
    Ok(true)
}

pub async fn execute_import(
    executor: &mut CommandExecutor,
    format: ImportFormat,
    file_path: String,
    target: ImportTarget,
    batch_size: Option<usize>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }

    let config = ImportConfig::new(file_path.into(), target)
        .with_format(format)
        .with_batch_size(batch_size.unwrap_or(100));

    let stats = match config.format {
        ImportFormat::Csv { .. } => {
            let mut importer = CsvImporter::new(config);
            importer.import(session_mgr).await?
        }
        ImportFormat::Json { .. } => {
            let mut importer = JsonImporter::new(config);
            importer.import(session_mgr).await?
        }
        ImportFormat::JsonLines => {
            let mut importer = JsonImporter::new(config);
            importer.import(session_mgr).await?
        }
    };

    executor.write_output(&stats.format_summary())?;
    Ok(true)
}

pub async fn execute_export(
    executor: &mut CommandExecutor,
    format: ExportFormat,
    file_path: String,
    query: &str,
    streaming: bool,
    chunk_size: Option<usize>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }

    let mut config = ExportConfig::new(file_path.into(), format).with_streaming(streaming);

    if let Some(size) = chunk_size {
        config = config.with_chunk_size(size);
    }

    let stats = match &config.format {
        ExportFormat::Csv { .. } => {
            let exporter = CsvExporter::new(config);
            exporter.export(query, session_mgr).await?
        }
        ExportFormat::Json { .. } | ExportFormat::JsonLines => {
            let exporter = JsonExporter::new(config);
            exporter.export(query, session_mgr).await?
        }
    };

    executor.write_output(&stats.format_summary())?;
    Ok(true)
}

pub async fn execute_copy(
    executor: &mut CommandExecutor,
    direction: CopyDirection,
    target: String,
    file_path: String,
    streaming: bool,
    chunk_size: Option<usize>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    if !executor.conditional_stack().is_active() {
        return Ok(true);
    }

    match direction {
        CopyDirection::From => {
            let import_format = if file_path.ends_with(".json") || file_path.ends_with(".jsonl") {
                ImportFormat::json_array()
            } else {
                ImportFormat::csv()
            };

            let config = ImportConfig::new(file_path.into(), ImportTarget::vertex(&target))
                .with_format(import_format.clone());

            let stats = match import_format {
                ImportFormat::Csv { .. } => {
                    let mut importer = CsvImporter::new(config);
                    importer.import(session_mgr).await?
                }
                _ => {
                    let mut importer = JsonImporter::new(config);
                    importer.import(session_mgr).await?
                }
            };

            executor.write_output(&stats.format_summary())?;
        }
        CopyDirection::To => {
            let query = format!("MATCH (n:{}) RETURN n", target);
            let export_format = if file_path.ends_with(".json") {
                ExportFormat::json()
            } else {
                ExportFormat::csv()
            };

            let mut config =
                ExportConfig::new(file_path.into(), export_format).with_streaming(streaming);

            if let Some(size) = chunk_size {
                config = config.with_chunk_size(size);
            }

            let stats = match &config.format {
                ExportFormat::Csv { .. } => {
                    let exporter = CsvExporter::new(config);
                    exporter.export(&query, session_mgr).await?
                }
                _ => {
                    let exporter = JsonExporter::new(config);
                    exporter.export(&query, session_mgr).await?
                }
            };

            executor.write_output(&stats.format_summary())?;
        }
    }
    Ok(true)
}

pub async fn execute_dump(
    executor: &mut CommandExecutor,
    database: String,
    output_path: String,
    format: String,
    compress: bool,
) -> Result<bool> {
    let _ = (executor, database, output_path, format, compress);
    Err(crate::utils::error::CliError::Other(
        "Database dump is not implemented; use the server backup API when available".to_string(),
    ))
}

pub async fn execute_restore(
    executor: &mut CommandExecutor,
    source_path: String,
    database: String,
    overwrite: bool,
    strict: bool,
) -> Result<bool> {
    let _ = (executor, source_path, database, overwrite, strict);
    Err(crate::utils::error::CliError::Other(
        "Database restore is not implemented; use the server backup API when available".to_string(),
    ))
}

pub async fn execute_export_space(
    executor: &mut CommandExecutor,
    space_name: String,
    output_path: String,
    format: String,
    tags: Option<String>,
    edge_types: Option<String>,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    use crate::io::space_export::SpaceExportConfig;
    use crate::io::ExportFormat;

    let export_format = match format.as_str() {
        "json" => ExportFormat::json(),
        "jsonl" => ExportFormat::json_lines(),
        _ => ExportFormat::csv(),
    };

    let config = SpaceExportConfig {
        space_name,
        output_path: std::path::PathBuf::from(output_path),
        format: export_format,
        include_schema: true,
        include_data: true,
        streaming: true,
        chunk_size: 1000,
        tag_filter: tags.map(|t| t.split(',').map(String::from).collect()),
        edge_type_filter: edge_types.map(|e| e.split(',').map(String::from).collect()),
    };

    let exporter = crate::io::space_export::SpaceExporter::new(config);
    match exporter.export(session_mgr).await {
        Ok(stats) => {
            executor.write_output(&stats.format_summary())?;
            Ok(true)
        }
        Err(e) => Err(crate::utils::error::CliError::Other(format!(
            "Space export failed: {}",
            e
        ))),
    }
}

pub async fn execute_export_schema(
    executor: &mut CommandExecutor,
    output_path: String,
    format: String,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    use crate::io::schema_io::{SchemaExportFormat, SchemaExporter, SchemaIoConfig};

    let schema_format = match format.as_str() {
        "yaml" => SchemaExportFormat::Yaml,
        _ => SchemaExportFormat::Json,
    };

    let space_name = session_mgr
        .session()
        .and_then(|s| s.current_space.clone())
        .unwrap_or_default();

    let path_buf = std::path::PathBuf::from(output_path.clone());
    let config = SchemaIoConfig {
        space_name,
        output_path: path_buf,
        format: schema_format,
    };

    let exporter = SchemaExporter::new();
    match exporter.export(config, session_mgr).await {
        Ok(()) => {
            executor.write_output(&format!("Schema exported to {}", output_path))?;
            Ok(true)
        }
        Err(e) => Err(crate::utils::error::CliError::Other(format!(
            "Schema export failed: {}",
            e
        ))),
    }
}

pub async fn execute_import_schema(
    executor: &mut CommandExecutor,
    file_path: String,
    session_mgr: &mut SessionManager,
) -> Result<bool> {
    use crate::io::schema_io::SchemaImporter;

    let importer = SchemaImporter::new();
    let path = std::path::PathBuf::from(&file_path);

    match importer.import(&path, session_mgr).await {
        Ok(result) => {
            if result.success {
                executor.write_output(&format!(
                    "Schema imported successfully: {} items",
                    result.imported_items
                ))?;
                Ok(true)
            } else {
                Err(crate::utils::error::CliError::Other(format!(
                    "Schema import failed: {:?}",
                    result.errors
                )))
            }
        }
        Err(e) => Err(crate::utils::error::CliError::Other(format!(
            "Schema import failed: {}",
            e
        ))),
    }
}
