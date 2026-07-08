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
