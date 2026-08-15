use crate::command::parser::types::MetaCommand;

pub fn parse_set(arg: &str) -> Result<MetaCommand, String> {
    let set_parts: Vec<&str> = arg.splitn(2, char::is_whitespace).collect();
    if set_parts.is_empty() || set_parts[0].is_empty() {
        Ok(MetaCommand::ShowVariables)
    } else {
        let name = set_parts[0].to_string();
        let value = set_parts.get(1).map(|s| s.to_string());
        Ok(MetaCommand::Set { name, value })
    }
}

pub fn parse_unset(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\unset <variable_name>".to_string())
    } else {
        Ok(MetaCommand::Unset {
            name: arg.to_string(),
        })
    }
}
