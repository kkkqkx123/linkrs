use crate::command::parser::types::MetaCommand;

pub fn parse_statistics(arg: &str) -> Result<MetaCommand, String> {
    let target = if arg.trim().is_empty() {
        None
    } else {
        let lowered = arg.trim().to_lowercase();
        let starts_with_profile =
            lowered.starts_with("profile ") && !lowered["profile ".len()..].trim().is_empty();
        if starts_with_profile {
            Some(lowered)
        } else {
            match lowered.as_str() {
                "queries" | "query" | "database" | "db" | "system" | "overview" | "session"
                | "search" | "freeze" | "migration" => Some(lowered),
                _ => {
                    return Err(
                        "Usage: \\statistics [queries|database|system|overview|session|search|freeze|migration|profile <trace_id>]"
                            .to_string(),
                    );
                }
            }
        }
    };
    Ok(MetaCommand::Statistics { target })
}
