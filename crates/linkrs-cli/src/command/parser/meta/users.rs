use crate::command::parser::types::{MetaCommand, SessionsAction, UsersAction};

/// Parse `\users <subcommand> ...`.
pub fn parse_users(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\users list | create <user> <password> | drop <user> | reset-password <user> | \
             enable <user> | disable <user> | grant <user> <role> <space> | revoke <user> <space>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "list" | "ls" => Ok(MetaCommand::Users {
            action: UsersAction::List,
        }),
        "create" | "add" => {
            if parts.len() < 3 {
                return Err("Usage: \\users create <user> <password>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Create {
                    username: parts[1].to_string(),
                    password: parts[2].to_string(),
                },
            })
        }
        "drop" | "remove" | "rm" => {
            if parts.len() < 2 {
                return Err("Usage: \\users drop <user>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Drop {
                    username: parts[1].to_string(),
                },
            })
        }
        "reset-password" | "passwd" | "reset" => {
            if parts.len() < 2 {
                return Err("Usage: \\users reset-password <user>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::ResetPassword {
                    username: parts[1].to_string(),
                },
            })
        }
        "enable" => {
            if parts.len() < 2 {
                return Err("Usage: \\users enable <user>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Enable {
                    username: parts[1].to_string(),
                },
            })
        }
        "disable" => {
            if parts.len() < 2 {
                return Err("Usage: \\users disable <user>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Disable {
                    username: parts[1].to_string(),
                },
            })
        }
        "grant" => {
            if parts.len() < 4 {
                return Err("Usage: \\users grant <user> <role> <space>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Grant {
                    username: parts[1].to_string(),
                    role: parts[2].to_string(),
                    space: parts[3].to_string(),
                },
            })
        }
        "revoke" => {
            if parts.len() < 3 {
                return Err("Usage: \\users revoke <user> <space>".to_string());
            }
            Ok(MetaCommand::Users {
                action: UsersAction::Revoke {
                    username: parts[1].to_string(),
                    space: parts[2].to_string(),
                },
            })
        }
        other => Err(format!(
            "Unknown users subcommand: {}. Expected list|create|drop|reset-password|enable|disable|grant|revoke",
            other
        )),
    }
}

/// Parse `\sessions [list|show <id>|kill <id>]`.
pub fn parse_sessions(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() || parts[0].eq_ignore_ascii_case("list") {
        return Ok(MetaCommand::Sessions {
            action: SessionsAction::List,
        });
    }
    match parts[0].to_lowercase().as_str() {
        "show" | "get" => {
            let session_id = parse_session_id(parts.get(1).copied(), "show")?;
            Ok(MetaCommand::Sessions {
                action: SessionsAction::Show { session_id },
            })
        }
        "kill" | "terminate" => {
            let session_id = parse_session_id(parts.get(1).copied(), "kill")?;
            Ok(MetaCommand::Sessions {
                action: SessionsAction::Kill { session_id },
            })
        }
        other => Err(format!(
            "Unknown sessions subcommand: {}. Expected list|show <id>|kill <id>",
            other
        )),
    }
}

fn parse_session_id(raw: Option<&str>, subcommand: &str) -> Result<i64, String> {
    let raw = raw.ok_or_else(|| format!("Usage: \\sessions {} <session_id>", subcommand))?;
    raw.parse::<i64>()
        .map_err(|_| format!("Invalid session id '{}', expected integer", raw))
}

/// Parse `\queries [list|kill <session_id> <query_id>]`.
pub fn parse_queries(arg: &str) -> Result<MetaCommand, String> {
    use crate::command::parser::types::QueriesAction;
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() || parts[0].eq_ignore_ascii_case("list") {
        return Ok(MetaCommand::Queries {
            action: QueriesAction::List,
        });
    }
    match parts[0].to_lowercase().as_str() {
        "kill" | "cancel" | "terminate" => {
            if parts.len() < 3 {
                return Err("Usage: \\queries kill <session_id> <query_id>".to_string());
            }
            let session_id = parts[1]
                .parse::<i64>()
                .map_err(|_| format!("Invalid session id '{}', expected integer", parts[1]))?;
            let query_id = parts[2]
                .parse::<i64>()
                .map_err(|_| format!("Invalid query id '{}', expected integer", parts[2]))?;
            Ok(MetaCommand::Queries {
                action: QueriesAction::Kill {
                    session_id,
                    query_id,
                },
            })
        }
        other => Err(format!(
            "Unknown queries subcommand: {}. Expected list|kill <session_id> <query_id>",
            other
        )),
    }
}
