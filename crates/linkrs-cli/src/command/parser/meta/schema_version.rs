use crate::command::parser::types::MetaCommand;

pub fn parse_schema(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\schema versions <space> <label> [edge] | changes <space> <label> <from> <to> [edge] | breaking <space> <label> <from> <to> [edge]".to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "versions" | "history" => {
            if parts.len() < 3 {
                return Err("Usage: \\schema versions <space> <label> [edge]".to_string());
            }
            let is_edge = parts
                .get(3)
                .map(|v| v.eq_ignore_ascii_case("edge"))
                .unwrap_or(false);
            Ok(MetaCommand::SchemaVersions {
                space: parts[1].to_string(),
                label: parts[2].to_string(),
                is_edge,
            })
        }
        "changes" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\schema changes <space> <label> <from> <to> [edge]".to_string()
                );
            }
            let from: u64 = parts[3]
                .parse()
                .map_err(|_| format!("Invalid from_version '{}'", parts[3]))?;
            let to: u64 = parts[4]
                .parse()
                .map_err(|_| format!("Invalid to_version '{}'", parts[4]))?;
            let is_edge = parts
                .get(5)
                .map(|v| v.eq_ignore_ascii_case("edge"))
                .unwrap_or(false);
            Ok(MetaCommand::SchemaChanges {
                space: parts[1].to_string(),
                label: parts[2].to_string(),
                from,
                to,
                is_edge,
            })
        }
        "breaking" | "breaking-changes" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\schema breaking <space> <label> <from> <to> [edge]".to_string(),
                );
            }
            let from: u64 = parts[3]
                .parse()
                .map_err(|_| format!("Invalid from_version '{}'", parts[3]))?;
            let to: u64 = parts[4]
                .parse()
                .map_err(|_| format!("Invalid to_version '{}'", parts[4]))?;
            let is_edge = parts
                .get(5)
                .map(|v| v.eq_ignore_ascii_case("edge"))
                .unwrap_or(false);
            Ok(MetaCommand::SchemaBreaking {
                space: parts[1].to_string(),
                label: parts[2].to_string(),
                from,
                to,
                is_edge,
            })
        }
        other => Err(format!(
            "Unknown schema subcommand: {}. Expected versions|changes|breaking",
            other
        )),
    }
}
