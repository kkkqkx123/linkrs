use crate::command::parser::types::{FulltextAction, MetaCommand, VectorAction};

/// Parse a `space_id tag_name field_name` triple shared by fulltext and
/// vector subcommands.
fn parse_triple(parts: &[&str]) -> Result<(u64, String, String), String> {
    if parts.len() < 3 {
        return Err("Expected <space_id> <tag_name> <field_name>".to_string());
    }
    let space_id: u64 = parts[0]
        .parse()
        .map_err(|_| format!("Invalid space_id '{}', expected integer", parts[0]))?;
    Ok((space_id, parts[1].to_string(), parts[2].to_string()))
}

/// Parse `\fulltext <subcommand> ...`.
pub fn parse_fulltext(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\fulltext search <space_id> <tag> <field> <query> [limit] | indexes | \
             rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "search" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\fulltext search <space_id> <tag> <field> <query> [limit]".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            // Query may contain multiple words: collect everything between the
            // field name and an optional trailing limit.
            let rest = &parts[4..];
            let (query, limit) = if let Some(last) = rest.last() {
                if rest.len() > 1 && last.chars().all(|c| c.is_ascii_digit()) {
                    let limit: usize = last
                        .parse()
                        .map_err(|_| format!("Invalid limit '{}'", last))?;
                    (rest[..rest.len() - 1].join(" "), limit)
                } else {
                    (rest.join(" "), 10)
                }
            } else {
                return Err("Usage: \\fulltext search <space_id> <tag> <field> <query> [limit]".to_string());
            };
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    query,
                    limit,
                },
            })
        }
        "indexes" | "list" => Ok(MetaCommand::Fulltext {
            action: FulltextAction::ListIndexes,
        }),
        "inconsistent" => Ok(MetaCommand::Fulltext {
            action: FulltextAction::Inconsistent,
        }),
        "rebuild" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Rebuild {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "status" => {
            let rebuild_id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\fulltext status <rebuild_id>".to_string())?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::RebuildStatus {
                    rebuild_id: rebuild_id.to_string(),
                },
            })
        }
        "clear" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Fulltext {
                action: FulltextAction::Clear {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        other => Err(format!(
            "Unknown fulltext subcommand: {}. Expected search|indexes|inconsistent|rebuild|status|clear",
            other
        )),
    }
}

/// Parse `\vector <subcommand> ...`.
pub fn parse_vector(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.is_empty() {
        return Err(
            "Usage: \\vector create <space_id> <tag> <field> <vector_size> | indexes | \
             info <space_id> <tag> <field> | drop <space_id> <tag> <field> | \
             search <space_id> <tag> <field> <v1,v2,...> [limit] | \
             get <space_id> <tag> <field> <point_id> | \
             scroll <space_id> <tag> <field> [limit] [offset] | count <space_id> <tag> <field> | \
             set-payload <space_id> <tag> <field> <id1,id2> <json> | \
             merge-payload <space_id> <tag> <field> <id1,id2> <json> | \
             delete-payload <space_id> <tag> <field> <id1,id2> <key1,key2> | \
             rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>"
                .to_string(),
        );
    }
    match parts[0].to_lowercase().as_str() {
        "create" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\vector create <space_id> <tag> <field> <vector_size>".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let vector_size: usize = parts[4]
                .parse()
                .map_err(|_| format!("Invalid vector_size '{}', expected integer", parts[4]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Create {
                    space_id,
                    tag_name,
                    field_name,
                    vector_size,
                },
            })
        }
        "indexes" | "list" => Ok(MetaCommand::Vector {
            action: VectorAction::ListIndexes,
        }),
        "info" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Info {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "drop" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Drop {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "search" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\vector search <space_id> <tag> <field> <v1,v2,...> [limit]".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let vector = parse_vector_values(parts[4])?;
            let limit: usize = match parts.get(5) {
                Some(s) => s
                    .parse()
                    .map_err(|_| format!("Invalid limit '{}', expected integer", s))?,
                None => 10,
            };
            Ok(MetaCommand::Vector {
                action: VectorAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    vector,
                    limit,
                },
            })
        }
        "get" => {
            if parts.len() < 5 {
                return Err(
                    "Usage: \\vector get <space_id> <tag> <field> <point_id>".to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Get {
                    space_id,
                    tag_name,
                    field_name,
                    point_id: parts[4].to_string(),
                },
            })
        }
        "scroll" => {
            if parts.len() < 4 {
                return Err(
                    "Usage: \\vector scroll <space_id> <tag> <field> [limit] [offset]"
                        .to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let limit: usize = match parts.get(4) {
                Some(s) => s
                    .parse()
                    .map_err(|_| format!("Invalid limit '{}', expected integer", s))?,
                None => 10,
            };
            Ok(MetaCommand::Vector {
                action: VectorAction::Scroll {
                    space_id,
                    tag_name,
                    field_name,
                    limit,
                    offset: parts.get(5).map(|s| s.to_string()),
                },
            })
        }
        "set-payload" | "merge-payload" => {
            let set = parts[0].eq_ignore_ascii_case("set-payload");
            let usage = if set {
                "Usage: \\vector set-payload <space_id> <tag> <field> <id1,id2> <json>"
            } else {
                "Usage: \\vector merge-payload <space_id> <tag> <field> <id1,id2> <json>"
            };
            if parts.len() < 6 {
                return Err(usage.to_string());
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            let point_ids = parse_id_list(parts[4])?;
            // Payload may contain whitespace, so everything after the id
            // list is treated as the JSON document.
            let payload_raw = parts[5..].join(" ");
            // Payload must be a JSON object; reject anything else instead of
            // silently coercing so the server never sees a misshaped request.
            let payload: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&payload_raw).map_err(|_| {
                    format!("Invalid payload '{}', expected a JSON object", payload_raw)
                })?;
            let action = if set {
                VectorAction::SetPayload {
                    space_id,
                    tag_name,
                    field_name,
                    point_ids,
                    payload,
                }
            } else {
                VectorAction::MergePayloadFields {
                    space_id,
                    tag_name,
                    field_name,
                    point_ids,
                    payload,
                }
            };
            Ok(MetaCommand::Vector { action })
        }
        "delete-payload" => {
            if parts.len() < 6 {
                return Err(
                    "Usage: \\vector delete-payload <space_id> <tag> <field> <id1,id2> <key1,key2>"
                        .to_string(),
                );
            }
            let (space_id, tag_name, field_name) = parse_triple(&parts[1..])?;
            Ok(MetaCommand::Vector {
                action: VectorAction::DeletePayload {
                    space_id,
                    tag_name,
                    field_name,
                    point_ids: parse_id_list(parts[4])?,
                    keys: parse_id_list(parts[5])?,
                },
            })
        }
        "count" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Count {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "rebuild" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Rebuild {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        "status" => {
            let rebuild_id = parts
                .get(1)
                .ok_or_else(|| "Usage: \\vector status <rebuild_id>".to_string())?;
            Ok(MetaCommand::Vector {
                action: VectorAction::RebuildStatus {
                    rebuild_id: rebuild_id.to_string(),
                },
            })
        }
        "clear" => {
            let (space_id, tag_name, field_name) = parse_triple(parts.get(1..).unwrap_or(&[]))?;
            Ok(MetaCommand::Vector {
                action: VectorAction::Clear {
                    space_id,
                    tag_name,
                    field_name,
                },
            })
        }
        other => Err(format!(
            "Unknown vector subcommand: {}. Expected create|indexes|info|drop|search|get|scroll|count|set-payload|merge-payload|delete-payload|rebuild|status|clear",
            other
        )),
    }
}

/// Parse a comma-separated float vector like `0.1,0.2,0.3`.
fn parse_vector_values(raw: &str) -> Result<Vec<f32>, String> {
    raw.split(',')
        .map(|token| {
            token
                .trim()
                .parse::<f32>()
                .map_err(|_| format!("Invalid vector component '{}', expected float", token))
        })
        .collect()
}

/// Parse a comma-separated id/key list like `id1,id2`. Empty entries are
/// rejected so a stray trailing comma cannot silently drop an id.
fn parse_id_list(raw: &str) -> Result<Vec<String>, String> {
    let ids: Vec<String> = raw
        .split(',')
        .map(|token| token.trim().to_string())
        .collect();
    if ids.iter().any(|id| id.is_empty()) {
        return Err(format!("Invalid id list '{}', expected id1,id2,...", raw));
    }
    Ok(ids)
}
