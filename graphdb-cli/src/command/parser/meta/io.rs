use crate::command::parser::types::{CopyDirection, MetaCommand};
use crate::io::{ExportFormat, ImportFormat, ImportTarget};

pub fn parse_execute_script(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\i <file_path>".to_string())
    } else {
        Ok(MetaCommand::ExecuteScript {
            path: arg.to_string(),
        })
    }
}

pub fn parse_execute_script_raw(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\ir <file_path>".to_string())
    } else {
        Ok(MetaCommand::ExecuteScriptRaw {
            path: arg.to_string(),
        })
    }
}

pub fn parse_output_redirect(arg: &str) -> Result<MetaCommand, String> {
    let path = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::OutputRedirect { path })
}

pub fn parse_import(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();

    if parts.len() < 4 {
        return Err(
            "Usage: \\import <csv|json|jsonl> <file> <tag|edge> <name> [batch_size]".to_string(),
        );
    }

    let format = match parts[0].to_lowercase().as_str() {
        "csv" => ImportFormat::csv(),
        "json" => ImportFormat::json_array(),
        "jsonl" => ImportFormat::json_lines(),
        _ => return Err(format!("Unsupported format: {}", parts[0])),
    };

    let file_path = parts[1].to_string();

    let target = match parts[2].to_lowercase().as_str() {
        "tag" | "vertex" => ImportTarget::vertex(parts[3]),
        "edge" => ImportTarget::edge(parts[3]),
        _ => return Err(format!("Invalid target type: {}", parts[2])),
    };

    let batch_size = parts.get(4).and_then(|s| s.parse().ok());

    Ok(MetaCommand::Import {
        format,
        file_path,
        target,
        batch_size,
    })
}

pub fn parse_export(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();

    if parts.len() < 3 {
        return Err(
            "Usage: \\export <csv|json|jsonl> <file> <query> [--stream] [--chunk-size <n>]"
                .to_string(),
        );
    }

    let format = match parts[0].to_lowercase().as_str() {
        "csv" => ExportFormat::csv(),
        "json" => ExportFormat::json(),
        "jsonl" => ExportFormat::json_lines(),
        _ => return Err(format!("Unsupported format: {}", parts[0])),
    };

    let file_path = parts[1].to_string();

    let mut streaming = false;
    let mut chunk_size: Option<usize> = None;
    let mut query_parts: Vec<&str> = Vec::new();

    let mut i = 2;
    while i < parts.len() {
        match parts[i] {
            "--stream" | "-s" => {
                streaming = true;
            }
            "--chunk-size" | "-c" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --chunk-size".to_string());
                }
                chunk_size = Some(parts[i].parse().map_err(|_| "Invalid chunk size")?);
            }
            _ => {
                query_parts.push(parts[i]);
            }
        }
        i += 1;
    }

    if query_parts.is_empty() {
        return Err("Query is required".to_string());
    }

    let query = query_parts.join(" ");

    Ok(MetaCommand::Export {
        format,
        file_path,
        query,
        streaming,
        chunk_size,
    })
}

pub fn parse_copy(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();

    if parts.len() < 4 {
        return Err(
            "Usage: \\copy <target> from|to '<file>' [--stream] [--chunk-size <n>]".to_string(),
        );
    }

    let target = parts[0].to_string();
    let direction = match parts[1].to_lowercase().as_str() {
        "from" => CopyDirection::From,
        "to" => CopyDirection::To,
        _ => {
            return Err(format!(
                "Invalid direction: {}. Use 'from' or 'to'",
                parts[1]
            ))
        }
    };

    let file_path = parts[2].trim_matches('\'').to_string();

    let mut streaming = false;
    let mut chunk_size: Option<usize> = None;

    let mut i = 3;
    while i < parts.len() {
        match parts[i] {
            "--stream" | "-s" => {
                streaming = true;
            }
            "--chunk-size" | "-c" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --chunk-size".to_string());
                }
                chunk_size = Some(parts[i].parse().map_err(|_| "Invalid chunk size")?);
            }
            _ => {}
        }
        i += 1;
    }

     Ok(MetaCommand::Copy {
         direction,
         target,
         file_path,
         streaming,
         chunk_size,
     })
 }

 pub fn parse_dump(arg: &str) -> Result<MetaCommand, String> {
     let parts: Vec<&str> = arg.split_whitespace().collect();
     if parts.len() < 2 {
         return Err("Usage: \\dump <database> <output_path> [--format binary|jsonl] [--no-compress]".to_string());
     }
     let database = parts[0].to_string();
     let output_path = parts[1].to_string();
     let mut format = "binary".to_string();
     let mut compress = true;
     let mut i = 2;
     while i < parts.len() {
         match parts[i] {
             "--format" | "-f" => {
                 i += 1;
                 if i >= parts.len() {
                     return Err("Missing value for --format".to_string());
                 }
                 format = parts[i].to_string();
             }
             "--no-compress" => {
                 compress = false;
             }
             _ => {}
         }
         i += 1;
     }
     Ok(MetaCommand::Dump {
         database,
         output_path,
         format,
         compress,
     })
 }

 pub fn parse_restore(arg: &str) -> Result<MetaCommand, String> {
     let parts: Vec<&str> = arg.split_whitespace().collect();
     if parts.len() < 2 {
         return Err("Usage: \\restore <source_path> <database> [--overwrite] [--strict]".to_string());
     }
     let source_path = parts[0].to_string();
     let database = parts[1].to_string();
     let mut overwrite = false;
     let mut strict = false;
     let mut i = 2;
     while i < parts.len() {
         match parts[i] {
             "--overwrite" => {
                 overwrite = true;
             }
             "--strict" => {
                 strict = true;
             }
             _ => {}
         }
         i += 1;
     }
     Ok(MetaCommand::Restore {
         source_path,
         database,
         overwrite,
         strict,
     })
 }

pub fn parse_export_space(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 3 {
        return Err("Usage: \\export-space <space_name> <output_path> [--format csv|json|jsonl] [--tags t1,t2] [--edges e1,e2]".to_string());
    }

    let space_name = parts[0].to_string();
    let output_path = parts[1].to_string();
    let mut format = "csv".to_string();
    let mut tags = None;
    let mut edge_types = None;

    let mut i = 2;
    while i < parts.len() {
        match parts[i] {
            "--format" | "-f" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --format".to_string());
                }
                format = parts[i].to_string();
            }
            "--tags" | "-t" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --tags".to_string());
                }
                tags = Some(parts[i].to_string());
            }
            "--edges" | "-e" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --edges".to_string());
                }
                edge_types = Some(parts[i].to_string());
            }
            _ => {}
        }
        i += 1;
    }

    Ok(MetaCommand::ExportSpace {
        space_name,
        output_path,
        format,
        tags,
        edge_types,
    })
}

pub fn parse_export_schema(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() < 2 {
        return Err("Usage: \\export-schema <output_path> [--format json|yaml]".to_string());
    }

    let output_path = parts[0].to_string();
    let mut format = "json".to_string();

    let mut i = 1;
    while i < parts.len() {
        match parts[i] {
            "--format" | "-f" => {
                i += 1;
                if i >= parts.len() {
                    return Err("Missing value for --format".to_string());
                }
                format = parts[i].to_string();
            }
            _ => {}
        }
        i += 1;
    }

    Ok(MetaCommand::ExportSchema {
        output_path,
        format,
    })
}

pub fn parse_import_schema(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        return Err("Usage: \\import-schema <file_path>".to_string());
    }

    Ok(MetaCommand::ImportSchema {
        file_path: arg.to_string(),
    })
}



