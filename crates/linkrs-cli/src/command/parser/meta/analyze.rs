use crate::command::parser::types::MetaCommand;

pub fn parse_explain(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        return Err("Usage: \\explain [analyze] <query>".to_string());
    }

    let mut analyze = false;
    let mut query_parts = Vec::new();

    for part in arg.split_whitespace() {
        if part.to_lowercase() == "analyze" {
            analyze = true;
        } else {
            query_parts.push(part);
        }
    }

    if query_parts.is_empty() {
        return Err("Usage: \\explain [analyze] <query>".to_string());
    }

    Ok(MetaCommand::Explain {
        query: query_parts.join(" "),
        analyze,
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

pub fn parse_validate(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\validate <query>".to_string())
    } else {
        Ok(MetaCommand::Validate {
            query: arg.to_string(),
        })
    }
}
