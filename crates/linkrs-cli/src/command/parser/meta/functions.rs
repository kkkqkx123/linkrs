use crate::command::parser::types::MetaCommand;

pub fn parse_functions(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(MetaCommand::ShowFunctions);
    }
    match parts[0].to_lowercase().as_str() {
        "list" | "ls" => Ok(MetaCommand::ShowFunctions),
        "info" | "show" | "describe" => {
            if parts.len() < 2 {
                return Err("Usage: \\functions info <name>".to_string());
            }
            Ok(MetaCommand::ShowFunction {
                name: parts[1].to_string(),
            })
        }
        "register" | "add" => {
            if parts.len() < 3 {
                return Err("Usage: \\functions register <name> <implementation_path>".to_string());
            }
            Ok(MetaCommand::RegisterFunction {
                name: parts[1].to_string(),
                implementation: parts[2].to_string(),
            })
        }
        "unregister" | "remove" | "rm" | "drop" => {
            if parts.len() < 2 {
                return Err("Usage: \\functions unregister <name>".to_string());
            }
            Ok(MetaCommand::UnregisterFunction {
                name: parts[1].to_string(),
            })
        }
        other => Err(format!(
            "Unknown functions subcommand: {}. Expected list|info|register|unregister",
            other
        )),
    }
}
