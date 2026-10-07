use crate::command::parser::types::MetaCommand;

pub fn parse(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\connect <space_name>".to_string())
    } else {
        Ok(MetaCommand::Connect {
            space: arg.to_string(),
        })
    }
}

pub fn parse_login(arg: &str) -> Result<MetaCommand, String> {
    let mut parts = arg.split_whitespace();
    match parts.next() {
        Some(username) if !username.is_empty() => Ok(MetaCommand::Login {
            username: username.to_string(),
            password: parts.next().map(|s| s.to_string()),
        }),
        _ => Err("Usage: \\login <username> [password]".to_string()),
    }
}
