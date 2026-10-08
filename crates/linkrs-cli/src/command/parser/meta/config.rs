use crate::command::parser::types::{ConfigAction, MetaCommand};

pub fn parse_config(arg: &str) -> Result<MetaCommand, String> {
    let trimmed = arg.trim();
    if trimmed.is_empty() {
        return Ok(MetaCommand::Config {
            action: ConfigAction::Show { section: None },
        });
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let sub = parts.next().unwrap_or("").to_lowercase();
    let rest = parts.next().unwrap_or("").trim();
    match sub.as_str() {
        "show" => {
            let section = if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            Ok(MetaCommand::Config {
                action: ConfigAction::Show { section },
            })
        }
        "get" => {
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() != 2 {
                return Err("Usage: \\config get <section> <key>".to_string());
            }
            Ok(MetaCommand::Config {
                action: ConfigAction::Get {
                    section: fields[0].to_string(),
                    key: fields[1].to_string(),
                },
            })
        }
        "set" => {
            let fields: Vec<&str> = rest.splitn(3, char::is_whitespace).collect();
            if fields.len() != 3 {
                return Err("Usage: \\config set <section> <key> <value>".to_string());
            }
            Ok(MetaCommand::Config {
                action: ConfigAction::Set {
                    section: fields[0].to_string(),
                    key: fields[1].to_string(),
                    value: fields[2].to_string(),
                },
            })
        }
        "reset" => {
            let fields: Vec<&str> = rest.split_whitespace().collect();
            if fields.len() != 2 {
                return Err("Usage: \\config reset <section> <key>".to_string());
            }
            Ok(MetaCommand::Config {
                action: ConfigAction::Reset {
                    section: fields[0].to_string(),
                    key: fields[1].to_string(),
                },
            })
        }
        _ => Err(
            "Usage: \\config [show [section] | get <section> <key> | set <section> <key> <value> | reset <section> <key>]"
                .to_string(),
        ),
    }
}
