use crate::command::parser::types::{MetaCommand, TransactionsAction};

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid transaction id: expected integer, got '{}'", value))
}

/// `\transactions list | metrics | kill <id> | retry-outbox <id>
///   | savepoint-create <txn> <name> | savepoint-list <txn>
///   | savepoint-rollback <txn> <spid> | savepoint-release <txn> <spid>`
pub fn parse_transactions(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    match parts.first().map(|s| s.to_lowercase()).as_deref() {
        Some("list") => Ok(MetaCommand::Transactions {
            action: TransactionsAction::List,
        }),
        Some("metrics") => Ok(MetaCommand::Transactions {
            action: TransactionsAction::Metrics,
        }),
        Some("kill") => {
            let id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\transactions kill <id>".to_string())?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::Kill { id: parse_id(id)? },
            })
        }
        Some("retry-outbox") | Some("retry_outbox") | Some("retry") => {
            let id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\transactions retry-outbox <id>".to_string())?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::RetryOutbox { id: parse_id(id)? },
            })
        }
        Some("savepoint-create") | Some("savepoint_create") => {
            let id = parts.get(1).ok_or_else(|| {
                "Usage: \\transactions savepoint-create <txn_id> <name>".to_string()
            })?;
            let name = parts.get(2).ok_or_else(|| {
                "Usage: \\transactions savepoint-create <txn_id> <name>".to_string()
            })?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::SavepointCreate {
                    id: parse_id(id)?,
                    name: name.to_string(),
                },
            })
        }
        Some("savepoint-list") | Some("savepoint_list") => {
            let id = parts.get(1).ok_or_else(|| {
                "Usage: \\transactions savepoint-list <txn_id>".to_string()
            })?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::SavepointList { id: parse_id(id)? },
            })
        }
        Some("savepoint-rollback") | Some("savepoint_rollback") => {
            let (id, savepoint_id) = parse_id_pair(&parts, "savepoint-rollback")?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::SavepointRollback { id, savepoint_id },
            })
        }
        Some("savepoint-release") | Some("savepoint_release") => {
            let (id, savepoint_id) = parse_id_pair(&parts, "savepoint-release")?;
            Ok(MetaCommand::Transactions {
                action: TransactionsAction::SavepointRelease { id, savepoint_id },
            })
        }
        _ => {
            Err("Usage: \\transactions list | metrics | kill <id> | retry-outbox <id> | savepoint-create <txn_id> <name> | savepoint-list <txn_id> | savepoint-rollback <txn_id> <spid> | savepoint-release <txn_id> <spid>".to_string())
        }
    }
}

fn parse_id_pair(parts: &[&str], subcommand: &str) -> Result<(u64, u64), String> {
    let id = parts.get(1).ok_or_else(|| {
        format!(
            "Usage: \\transactions {} <txn_id> <savepoint_id>",
            subcommand
        )
    })?;
    let savepoint_id = parts.get(2).ok_or_else(|| {
        format!(
            "Usage: \\transactions {} <txn_id> <savepoint_id>",
            subcommand
        )
    })?;
    Ok((parse_id(id)?, parse_id(savepoint_id)?))
}
