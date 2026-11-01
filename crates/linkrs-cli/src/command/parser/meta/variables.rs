use crate::command::parser::types::MetaCommand;

pub fn parse_set(arg: &str) -> Result<MetaCommand, String> {
    let trimmed = arg.trim();
    // `\set --remote [name [value]]`: operate on server-side `$` variables.
    // Plain `\set` keeps the historical local substitution behaviour.
    let (remote, rest) = match trimmed.strip_prefix("--remote") {
        Some(rest) => (true, rest.trim()),
        None => (false, trimmed),
    };
    if rest.is_empty() {
        return Ok(MetaCommand::ShowVariables { remote });
    }
    let set_parts: Vec<&str> = rest.splitn(2, char::is_whitespace).collect();
    if set_parts.is_empty() || set_parts[0].is_empty() {
        Ok(MetaCommand::ShowVariables { remote })
    } else {
        let name = set_parts[0].to_string();
        let value = set_parts.get(1).map(|s| s.to_string());
        Ok(MetaCommand::Set {
            name,
            value,
            remote,
        })
    }
}

pub fn parse_unset(arg: &str) -> Result<MetaCommand, String> {
    let trimmed = arg.trim();
    let (remote, rest) = match trimmed.strip_prefix("--remote") {
        Some(rest) => (true, rest.trim()),
        None => (false, trimmed),
    };
    if rest.is_empty() {
        Err("Usage: \\unset [--remote] <variable_name>".to_string())
    } else {
        Ok(MetaCommand::Unset {
            name: rest.to_string(),
            remote,
        })
    }
}
