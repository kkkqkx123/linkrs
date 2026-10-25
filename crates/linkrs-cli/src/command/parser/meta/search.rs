use crate::command::parser::types::{FulltextAction, MetaCommand, VectorAction};

/// Parse a `space_id tag_name field_name` triple shared by fulltext and
/// vector subcommands.
fn parse_triple(parts: &[&str]) -> Result<(u64, String, String), String> {
    if parts.len() < 3 {
        return Err("Expected <space_id> <tag_name> <field_name>".to_string());
    }
    let space_id: u64 = parts[0]
        .parse()
        .map_err(|_| format!("Invalid space_id '{}', expected integer", parts[0]))?;
    Ok((space_id, parts[1].to_string(), parts[2].to_string()))
}

/// Parse `\fulltext <subcommand> ...`.
pub fn parse_fulltext(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\fulltext search <space_id> <tag> <field> <query> [limit] | indexes | \
             rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "search" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\fulltext search <space_id> <tag> <field> <query> [limit]".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let limit: usize = parts.get(5).map(|s| s.parse().unwrap_or(10)).unwrap_or(10);
            let query = parts[4].to_string();
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    query,
                    limit,
                },
            })
        }
        "indexes" | "list" => Ok(MetaCommand::Fulltext {
            action: FulltextAction::ListIndexes,
        }),
        "rebuild" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Rebuild {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "status" => {
            let rebuild_id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\fulltext status <rebuild_id>".to_string())?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::RebuildStatus {
                    rebuild_id: rebuild_id.to_string(),
                },
            })
        }
        "clear" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Clear {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        other => Err(format!(
            "Unknown fulltext subcommand: {}. Expected search|indexes|rebuild|status|clear",
            other
        )),
    }
}

/// Parse `\vector <subcommand> ...`.
pub fn parse_vector(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\vector create <space_id> <tag> <field> <vector_size> | indexes | \
             info <space_id> <tag> <field> | drop <space_id> <tag> <field> | \
             search <space_id> <tag> <field> <v1,v2,...> [limit] | count <space_id> <tag> <field> | \
             rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "create" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\vector create <space_id> <tag> <field> <vector_size>".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let vector_size: usize = parts[4]
                .parse()
                .map_err(|_| format!("Invalid vector_size '{}', expected integer", parts[4]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Create {
                    space_id,
                    tag_name,
                    field_name,
                    vector_size,
                },
            })
        }
        "indexes" | "list" => Ok(MetaCommand::Vector {
            action: VectorAction::ListIndexes,
        }),
        "info" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Info {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "drop" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Drop {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "search" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\vector search <space_id> <tag> <field> <v1,v2,...> [limit]".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let vector = parse_vector_values(parts[4])?;
            let limit: usize = parts
                .get(5)
                .map(|s| s.parse().unwrap_or(10))
                .unwrap_or(10);
            Ok(MetaCommand::Vector {
                action: VectorAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    vector,
                    limit,
                },
            })
        }
        "count" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Count {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "rebuild" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Rebuild {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "status" => {
            let rebuild_id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\vector status <rebuild_id>".to_string())?;
            Ok(MetaCommand::Vector {
                action: VectorAction::RebuildStatus {
                    rebuild_id: rebuild_id.to_string(),
                },
            })
        }
        "clear" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Clear {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        other => Err(format!(
            "Unknown vector subcommand: {}. Expected create|indexes|info|drop|search|count|rebuild|status|clear",
            other
        )),
    }
}

/// Parse a comma-separated float vector like `0.1,0.2,0.3`.
fn parse_vector_values(raw: &str) -> Result<Vec<f32>, String> {
    raw.split(',')
        .map(|token| {
            token
                .trim()
                .parse::<f32>()
                .map_err(|_| format!("Invalid vector component '{}', expected float", token))
        })
        .collect()
}
