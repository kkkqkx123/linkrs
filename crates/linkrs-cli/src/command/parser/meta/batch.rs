use crate::command::parser::types::{BatchAction, MetaCommand};

pub fn parse_batch(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\batch create <space_id> <vertex|edge|mixed> [batch_size] | add <batch_id> <json_file> | execute <batch_id> | status <batch_id> | cancel <batch_id> | delete <batch_id>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "create" => {
            if parts.len() < 3 {
                return Err(
                    "Usage: \\batch create <space_id> <vertex|edge|mixed> [batch_size]".to_string(),
                );
            }
            let space_id: u64 = parts[1]
                .parse()
                .map_err(|_| "Invalid space_id, expected integer".to_string())?;
            let batch_type = parts[2].to_string();
            let batch_size: usize = parts
                .get(3)
                .map(|s| s.parse().unwrap_or(1000))
                .unwrap_or(1000);
            Ok(MetaCommand::Batch {
                action: BatchAction::Create {
                    space_id,
                    batch_type,
                    batch_size,
                },
            })
        }
        "add" => {
            if parts.len() < 3 {
                return Err("Usage: \\batch add <batch_id> <json_file>".to_string());
            }
            Ok(MetaCommand::Batch {
                action: BatchAction::Add {
                    batch_id: parts[1].to_string(),
                    file_path: parts[2].to_string(),
                },
            })
        }
        "execute" | "exec" => {
            if parts.len() < 2 {
                return Err("Usage: \\batch execute <batch_id>".to_string());
            }
            Ok(MetaCommand::Batch {
                action: BatchAction::Execute {
                    batch_id: parts[1].to_string(),
                },
            })
        }
        "status" => {
            if parts.len() < 2 {
                return Err("Usage: \\batch status <batch_id>".to_string());
            }
            Ok(MetaCommand::Batch {
                action: BatchAction::Status {
                    batch_id: parts[1].to_string(),
                },
            })
        }
        "cancel" => {
            if parts.len() < 2 {
                return Err("Usage: \\batch cancel <batch_id>".to_string());
            }
            Ok(MetaCommand::Batch {
                action: BatchAction::Cancel {
                    batch_id: parts[1].to_string(),
                },
            })
        }
        "delete" | "remove" | "rm" => {
            if parts.len() < 2 {
                return Err("Usage: \\batch delete <batch_id>".to_string());
            }
            Ok(MetaCommand::Batch {
                action: BatchAction::Delete {
                    batch_id: parts[1].to_string(),
                },
            })
        }
        other => Err(format!(
            "Unknown batch subcommand: {}. Expected create|add|execute|status|cancel|delete",
            other
        )),
    }
}
