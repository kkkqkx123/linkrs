use crate::command::parser::types::MetaCommand;

pub fn parse_stream(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        return Err("Usage: \\stream <query>".to_string());
    }
    Ok(MetaCommand::Stream {
        query: arg.to_string(),
    })
}

pub fn parse_cursor(arg: &str) -> Result<MetaCommand, String> {
    let mut parts = arg.split_whitespace();
    match parts.next().map(|s| s.to_lowercase()) {
        Some(sub) if sub == "open" => {
            let query: String = parts.collect::<Vec<_>>().join(" ");
            if query.is_empty() {
                return Err("Usage: \\cursor open <query>".to_string());
            }
            Ok(MetaCommand::CursorOpen { query })
        }
        Some(sub) if sub == "fetch" => {
            let rest: Vec<&str> = parts.collect();
            let cursor_id = rest.first().and_then(|s| s.parse::<u64>().ok());
            let page_size = if cursor_id.is_some() {
                rest.get(1).and_then(|s| s.parse::<usize>().ok())
            } else {
                rest.first().and_then(|s| s.parse::<usize>().ok())
            };
            Ok(MetaCommand::CursorFetch {
                cursor_id,
                page_size,
            })
        }
        Some(sub) if sub == "close" => {
            let cursor_id = parts.next().and_then(|s| s.parse::<u64>().ok());
            Ok(MetaCommand::CursorClose { cursor_id })
        }
        _ => Err(
            "Usage: \\cursor open <query> | \\cursor fetch [cursor_id] [page_size] | \\cursor close [cursor_id]"
                .to_string(),
        ),
    }
}
