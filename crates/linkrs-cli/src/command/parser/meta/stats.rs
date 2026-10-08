use crate::command::parser::types::MetaCommand;

pub fn parse_statistics(arg: &str) -> Result<MetaCommand, String> {
    let target = if arg.trim().is_empty() {
        None
    } else {
        let lowered = arg.trim().to_lowercase();
        match lowered.as_str() {
            "queries" | "query" | "database" | "db" | "system" | "overview" | "session" => {
                Some(lowered)
            }
            _ => {
                return Err(
                    "Usage: \\statistics [queries|database|system|overview|session]".to_string(),
                );
            }
        }
    };
    Ok(MetaCommand::Statistics { target })
}
