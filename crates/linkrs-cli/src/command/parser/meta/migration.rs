use crate::command::parser::types::{MetaCommand, MigrationAction};

fn parse_u64(value: &str, what: &str) -> Result<u64, String> {
    value
        .parse()
        .map_err(|_| format!("Invalid {} '{}'", what, value))
}

/// Parse `plan|dry-run|execute|rollback|status|history|watch` subcommands.
pub fn parse_migration(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\migration plan <space> <label> <from> <to> [edge] [-f FILE] | \
             dry-run <file> | execute <file> [--yes] | rollback <file> [--yes] | \
             status <space> <label> [edge] | history <space> <label> [edge] | \
             watch <space> <label> [edge]"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "plan" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\migration plan <space> <label> <from> <to> [edge] [-f FILE]"
                        .to_string(),
                );
            }
            let mut is_edge = false;
            let mut expand_contract = false;
            let mut output = None;
            let mut positional = Vec::new();
            let mut i = 5;
            while i < parts.len() {
                match parts[i] {
                    "edge" => is_edge = true,
                    "--expand-contract" => expand_contract = true,
                    "-f" | "--file" => {
                        i += 1;
                        output = Some(
                            parts
                                .get(i)
                                .copied()
                                .ok_or_else(|| "-f requires a file path".to_string())?
                                .to_string(),
                        );
                    }
                    other => positional.push(other),
                }
                i += 1;
            }
            if !positional.is_empty() {
                return Err(format!("Unexpected argument(s): {}", positional.join(" ")));
            }
            Ok(MetaCommand::Migration {
                action: MigrationAction::Plan {
                    space: parts[1].to_string(),
                    label: parts[2].to_string(),
                    from: parse_u64(parts[3], "from_version")?,
                    to: parse_u64(parts[4], "to_version")?,
                    is_edge,
                    expand_contract,
                    output,
                },
            })
        }
        "dry-run" | "dry_run" => Ok(MetaCommand::Migration {
            action: MigrationAction::DryRun {
                file: require_file(parts.get(1))?,
            },
        }),
        "execute" => {
            let (file, yes) = parse_file_with_yes(&parts[1..])?;
            Ok(MetaCommand::Migration {
                action: MigrationAction::Execute { file, yes },
            })
        }
        "rollback" => {
            let (file, yes) = parse_file_with_yes(&parts[1..])?;
            Ok(MetaCommand::Migration {
                action: MigrationAction::Rollback { file, yes },
            })
        }
        "status" | "history" | "watch" => {
            if parts.len() < 3 {
                return Err(format!(
                    "Usage: \\migration {} <space> <label> [edge]",
                    parts[0]
                ));
            }
            let is_edge = parts
                .get(3)
                .map(|v| v.eq_ignore_ascii_case("edge"))
                .unwrap_or(false);
            let action = match parts[0].to_lowercase().as_str() {
                "status" => MigrationAction::Status {
                    space: parts[1].to_string(),
                    label: parts[2].to_string(),
                    is_edge,
                },
                "history" => MigrationAction::History {
                    space: parts[1].to_string(),
                    label: parts[2].to_string(),
                    is_edge,
                },
                _ => MigrationAction::Watch {
                    space: parts[1].to_string(),
                    label: parts[2].to_string(),
                    is_edge,
                },
            };
            Ok(MetaCommand::Migration { action })
        }
        other => Err(format!(
            "Unknown migration subcommand: {}. Expected plan|dry-run|execute|rollback|status|history|watch",
            other
        )),
    }
}

fn require_file(value: Option<&&str>) -> Result<String, String> {
    value
        .copied()
        .map(|s| s.to_string())
        .ok_or_else(|| "A plan file path is required".to_string())
}

fn parse_file_with_yes(parts: &[&str]) -> Result<(String, bool), String> {
    let mut file = None;
    let mut yes = false;
    for part in parts {
        match *part {
            "--yes" => yes = true,
            "-f" | "--file" => {}
            other => {
                if file.is_some() && other != "--yes" && other != "-f" && other != "--file" {
                    return Err(format!("Unexpected argument: {}", other));
                }
                if file.is_none() {
                    file = Some(other.to_string());
                }
            }
        }
    }
    Ok((
        file.ok_or_else(|| "A plan file path is required".to_string())?,
        yes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plan_with_flags() {
        let cmd = parse_migration("plan s t 1 2 edge --expand-contract -f plan.json").unwrap();
        match cmd {
            MetaCommand::Migration {
                action: MigrationAction::Plan {
                    space,
                    label,
                    from,
                    to,
                    is_edge,
                    expand_contract,
                    output,
                },
            } => {
                assert_eq!(space, "s");
                assert_eq!(label, "t");
                assert_eq!((from, to), (1, 2));
                assert!(is_edge);
                assert!(expand_contract);
                assert_eq!(output.as_deref(), Some("plan.json"));
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn parse_execute_requires_file() {
        assert!(parse_migration("execute").is_err());
        match parse_migration("execute plan.json --yes").unwrap() {
            MetaCommand::Migration {
                action: MigrationAction::Execute { file, yes },
            } => {
                assert_eq!(file, "plan.json");
                assert!(yes);
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }

    #[test]
    fn parse_status_history_watch() {
        for (sub, is_edge) in [("status", false), ("history", true), ("watch", false)] {
            let arg = if is_edge {
                format!("{} s t edge", sub)
            } else {
                format!("{} s t", sub)
            };
            let cmd = parse_migration(&arg).unwrap();
            match cmd {
                MetaCommand::Migration {
                    action: MigrationAction::Status { .. },
                } if sub == "status" => {}
                MetaCommand::Migration {
                    action: MigrationAction::History { .. },
                } if sub == "history" => {}
                MetaCommand::Migration {
                    action: MigrationAction::Watch { .. },
                } if sub == "watch" => {}
                other => panic!("unexpected command for {}: {:?}", sub, other),
            }
        }
    }

    #[test]
    fn parse_unknown_subcommand_fails() {
        assert!(parse_migration("frobnicate").is_err());
        assert!(parse_migration("").is_err());
    }
}
