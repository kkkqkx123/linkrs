use crate::command::parser::types::{MetaCommand, SyncAction};

fn parse_u64(value: &str, name: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid {}: expected integer, got '{}'", name, value))
}

fn parse_usize(value: &str, name: &str) -> Result<usize, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid {}: expected integer, got '{}'", name, value))
}

pub fn parse_sync(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\sync status | diagnostics | dead-letters [target] [index_id] [generation] [limit] [offset] | requeue [target] [index_id] [generation] [limit] | retry | degraded [target] [index_id] [generation] | clear <target> <index_id> <generation> <start_lsn> <end_lsn> | retention-status | retention-run [grace_lsn_distance] [max_age_ms]"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "status" => Ok(MetaCommand::Sync {
            action: SyncAction::Status,
        }),
        "diagnostics" | "diag" => Ok(MetaCommand::Sync {
            action: SyncAction::Diagnostics,
        }),
        "dead-letters" | "dead_letters" | "dlq" => {
            let target = parts.get(1).map(|s| s.to_string());
            let index_id = parts
                .get(2)
                .map(|s| parse_u64(s, "index_id"))
                .transpose()?;
            let generation = parts
                .get(3)
                .map(|s| parse_u64(s, "generation"))
                .transpose()?;
            let limit = parts
                .get(4)
                .map(|s| parse_usize(s, "limit"))
                .transpose()?
                .unwrap_or(100);
            let offset = parts
                .get(5)
                .map(|s| parse_usize(s, "offset"))
                .transpose()?
                .unwrap_or(0);
            Ok(MetaCommand::Sync {
                action: SyncAction::DeadLetters {
                    target,
                    index_id,
                    generation,
                    limit,
                    offset,
                },
            })
        }
        "requeue" => {
            let target = parts.get(1).map(|s| s.to_string());
            let index_id = parts
                .get(2)
                .map(|s| parse_u64(s, "index_id"))
                .transpose()?;
            let generation = parts
                .get(3)
                .map(|s| parse_u64(s, "generation"))
                .transpose()?;
            let limit = parts
                .get(4)
                .map(|s| parse_usize(s, "limit"))
                .transpose()?
                .unwrap_or(100);
            Ok(MetaCommand::Sync {
                action: SyncAction::Requeue {
                    target,
                    index_id,
                    generation,
                    limit,
                },
            })
        }
        "retry" => Ok(MetaCommand::Sync {
            action: SyncAction::Retry,
        }),
        "degraded" | "degraded-ranges" | "ranges" => {
            let target = parts.get(1).map(|s| s.to_string());
            let index_id = parts
                .get(2)
                .map(|s| parse_u64(s, "index_id"))
                .transpose()?;
            let generation = parts
                .get(3)
                .map(|s| parse_u64(s, "generation"))
                .transpose()?;
            Ok(MetaCommand::Sync {
                action: SyncAction::DegradedRanges {
                    target,
                    index_id,
                    generation,
                },
            })
        }
        "clear" => {
            if parts.len() < 6 {
                return Err(
                    "Usage: \\sync clear <target> <index_id> <generation> <start_lsn> <end_lsn>"
                        .to_string(),
                );
            }
            Ok(MetaCommand::Sync {
                action: SyncAction::DegradedClear {
                    target: parts[1].to_string(),
                    index_id: parse_u64(parts[2], "index_id")?,
                    generation: parse_u64(parts[3], "generation")?,
                    start_lsn: parse_u64(parts[4], "start_lsn")?,
                    end_lsn: parse_u64(parts[5], "end_lsn")?,
                },
            })
        }
        "retention-status" | "retention_status" => Ok(MetaCommand::Sync {
            action: SyncAction::RetentionStatus,
        }),
        "retention-run" | "retention_run" | "retention" => {
            let grace_lsn_distance = parts
                .get(1)
                .map(|s| parse_u64(s, "grace_lsn_distance"))
                .transpose()?;
            let max_age_ms = parts
                .get(2)
                .map(|s| parse_u64(s, "max_age_ms"))
                .transpose()?;
            Ok(MetaCommand::Sync {
                action: SyncAction::RetentionRun {
                    grace_lsn_distance,
                    max_age_ms,
                },
            })
        }
        other => Err(format!(
            "Unknown sync subcommand: {}. Expected status|diagnostics|dead-letters|requeue|retry|degraded|clear|retention-status|retention-run",
            other
        )),
    }
}
