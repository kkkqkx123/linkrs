//! Parser for the `\snapshot` meta command.
//!
//! Syntax:
//! ```text
//! \snapshot list
//! \snapshot info --label <id>
//! \snapshot load --path <file>
//! \snapshot remove --label <id>
//! \snapshot export --label <id> --path <file>
//! \snapshot merge --labels <id,id,...>
//! ```

use crate::command::parser::types::{MetaCommand, SnapshotAction};

pub fn parse(arg: &str) -> Result<MetaCommand, String> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let (action, args) = parts
        .split_first()
        .ok_or_else(|| "Usage: \\snapshot <list|info|load|remove|export|merge>".to_string())?;
    let args = args.to_vec();

    let action = match action.to_lowercase().as_str() {
        "list" => SnapshotAction::List,
        "info" => SnapshotAction::Info {
            label: flag_u32(&args, "label")?,
        },
        "load" => SnapshotAction::Load {
            path: flag_str(&args, "path")?,
        },
        "remove" => SnapshotAction::Remove {
            label: flag_u32(&args, "label")?,
        },
        "export" => SnapshotAction::Export {
            label: flag_u32(&args, "label")?,
            path: flag_str(&args, "path")?,
        },
        "merge" => SnapshotAction::Merge {
            labels: flag_list(&args, "labels")?,
        },
        other => {
            return Err(format!(
                "Unknown snapshot subcommand '{}'; expected list|info|load|remove|export|merge",
                other
            ));
        }
    };
    Ok(MetaCommand::Snapshot { action })
}

fn flag_str(args: &[&str], name: &str) -> Result<String, String> {
    args.windows(2)
        .find(|w| w[0] == format!("--{}", name))
        .map(|w| w[1].to_string())
        .ok_or_else(|| format!("Missing --{} argument", name))
}

fn flag_u32(args: &[&str], name: &str) -> Result<u32, String> {
    let value = flag_str(args, name)?;
    value
        .parse()
        .map_err(|_| format!("--{} must be an integer, got '{}'", name, value))
}

fn flag_list(args: &[&str], name: &str) -> Result<Vec<u32>, String> {
    let value = flag_str(args, name)?;
    value
        .split(',')
        .map(|v| {
            v.trim()
                .parse()
                .map_err(|_| format!("--{} must be a comma-separated list of integers", name))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_snapshot_actions() {
        assert!(matches!(
            parse("list").unwrap(),
            MetaCommand::Snapshot {
                action: SnapshotAction::List
            }
        ));
        assert!(matches!(
            parse("info --label 7").unwrap(),
            MetaCommand::Snapshot {
                action: SnapshotAction::Info { label: 7 }
            }
        ));
        assert!(matches!(
            parse("load --path /tmp/s.lkcs").unwrap(),
            MetaCommand::Snapshot {
                action: SnapshotAction::Load { .. }
            }
        ));
        assert!(matches!(
            parse("remove --label 3").unwrap(),
            MetaCommand::Snapshot {
                action: SnapshotAction::Remove { label: 3 }
            }
        ));
        assert!(matches!(
            parse("export --label 1 --path /tmp/x.lkcs").unwrap(),
            MetaCommand::Snapshot {
                action: SnapshotAction::Export { .. }
            }
        ));
        match parse("merge --labels 1,2,3").unwrap() {
            MetaCommand::Snapshot {
                action: SnapshotAction::Merge { labels },
            } => assert_eq!(labels, vec![1, 2, 3]),
            other => panic!("unexpected: {:?}", other),
        }
        assert!(parse("bogus").is_err());
        assert!(parse("info").is_err());
    }
}
