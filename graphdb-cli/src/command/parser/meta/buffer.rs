use crate::command::parser::types::{HistoryAction, MetaCommand};

pub fn parse_edit(arg: &str) -> Result<MetaCommand, String> {
    let (file, line) = parse_edit_args(arg);
    Ok(MetaCommand::Edit { file, line })
}

pub fn parse_write_buffer(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\w <file_path>".to_string())
    } else {
        Ok(MetaCommand::WriteBuffer {
            file: arg.to_string(),
        })
    }
}

pub fn parse_history(arg: &str) -> Result<MetaCommand, String> {
    let action = parse_history_action(arg)?;
    Ok(MetaCommand::History { action })
}

fn parse_edit_args(arg: &str) -> (Option<String>, Option<usize>) {
    if arg.is_empty() {
        return (None, None);
    }

    let parts: Vec<&str> = arg.split_whitespace().collect();
    let mut file = None;
    let mut line = None;

    for part in parts {
        if let Some(l) = part.strip_prefix('+') {
            line = l.parse().ok();
        } else {
            file = Some(part.to_string());
        }
    }

    (file, line)
}

fn parse_history_action(arg: &str) -> Result<HistoryAction, String> {
    if arg.is_empty() {
        return Ok(HistoryAction::Show { count: Some(20) });
    }

    let parts: Vec<&str> = arg.splitn(2, char::is_whitespace).collect();
    let subcmd = parts[0].to_lowercase();
    let sub_arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match subcmd.as_str() {
        "clear" => Ok(HistoryAction::Clear),
        "search" => {
            if sub_arg.is_empty() {
                Err("Usage: \\history search <pattern>".to_string())
            } else {
                Ok(HistoryAction::Search {
                    pattern: sub_arg.to_string(),
                })
            }
        }
        "exec" => {
            if sub_arg.is_empty() {
                Err("Usage: \\history exec <id>".to_string())
            } else {
                let id = sub_arg
                    .parse()
                    .map_err(|_| format!("Invalid history ID: {}", sub_arg))?;
                Ok(HistoryAction::Exec { id })
            }
        }
        n => {
            if let Ok(count) = n.parse::<usize>() {
                Ok(HistoryAction::Show { count: Some(count) })
            } else {
                Err(format!("Unknown history action: {}", n))
            }
        }
    }
}
