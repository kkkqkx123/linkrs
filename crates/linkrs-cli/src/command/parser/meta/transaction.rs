use crate::command::parser::types::MetaCommand;

pub fn parse_rollback(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() >= 2 && parts[0].to_lowercase() == "to" {
        Ok(MetaCommand::RollbackTo {
            name: parts[1].to_string(),
        })
    } else {
        Ok(MetaCommand::Rollback)
    }
}

pub fn parse_autocommit(arg: &str) -> Result<MetaCommand, String> {
    let value = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::Autocommit { value })
}

pub fn parse_isolation(arg: &str) -> Result<MetaCommand, String> {
    let level = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::Isolation { level })
}

pub fn parse_savepoint(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\savepoint <name>".to_string())
    } else {
        let name = arg.split_whitespace().next().unwrap().to_string();
        Ok(MetaCommand::Savepoint { name })
    }
}

pub fn parse_release(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\release <savepoint_name>".to_string())
    } else {
        let name = arg.split_whitespace().next().unwrap().to_string();
        Ok(MetaCommand::ReleaseSavepoint { name })
    }
}
