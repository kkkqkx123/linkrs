use crate::analysis::explain::ExplainFormat;
use crate::command::parser::types::MetaCommand;

pub fn parse_explain(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        return Err("Usage: \\explain [analyze] [format=json|dot] <query>".to_string());
    }

    let mut analyze = false;
    let mut format = ExplainFormat::Text;
    let mut query_parts = Vec::new();

    for part in arg.split_whitespace() {
        if part.to_lowercase() == "analyze" {
            analyze = true;
        } else if part.to_lowercase().starts_with("format=") {
            let fmt = part.split('=').nth(1).unwrap_or("text");
            format = fmt.parse().unwrap();
        } else {
            query_parts.push(part);
        }
    }

    if query_parts.is_empty() {
        return Err("Usage: \\explain [analyze] [format=json|dot] <query>".to_string());
    }

    Ok(MetaCommand::Explain {
        query: query_parts.join(" "),
        analyze,
        format,
    })
}

pub fn parse_profile(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\profile <query>".to_string())
    } else {
        Ok(MetaCommand::Profile {
            query: arg.to_string(),
        })
    }
}
