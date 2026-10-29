use crate::command::parser::types::{MetaCommand, TransactionsAction};

fn parse_id(value: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid transaction id: expected integer, got '{}'", value))
}

/// `\transactions list | metrics | kill <id> | retry-outbox <id>`
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
        _ => {
            Err("Usage: \\transactions list | metrics | kill <id> | retry-outbox <id>".to_string())
        }
    }
}
