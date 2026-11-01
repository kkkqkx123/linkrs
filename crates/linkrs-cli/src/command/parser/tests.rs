use crate::command::parser::parse_command;
use crate::command::parser::types::{Command, CopyDirection, HistoryAction, MetaCommand};
use crate::output::formatter::OutputFormat;

#[test]
fn test_parse_command_empty() {
    assert!(matches!(parse_command(""), Command::Empty));
    assert!(matches!(parse_command("   "), Command::Empty));
    assert!(matches!(parse_command("\t\n"), Command::Empty));
}

#[test]
fn test_parse_command_query() {
    let query = "MATCH (v:Person) RETURN v";
    match parse_command(query) {
        Command::Query(q) => assert_eq!(q, query),
        _ => panic!("Expected Query command"),
    }
}

#[test]
fn test_parse_command_query_trimmed() {
    match parse_command("  MATCH (v)  ") {
        Command::Query(q) => assert_eq!(q, "MATCH (v)"),
        _ => panic!("Expected Query command"),
    }
}

#[test]
fn test_parse_meta_command_quit() {
    assert!(matches!(
        parse_command("\\q"),
        Command::MetaCommand(MetaCommand::Quit)
    ));
    assert!(matches!(
        parse_command("\\quit"),
        Command::MetaCommand(MetaCommand::Quit)
    ));
}

#[test]
fn test_parse_meta_command_force_quit() {
    assert!(matches!(
        parse_command("\\q!"),
        Command::MetaCommand(MetaCommand::ForceQuit)
    ));
}

#[test]
fn test_parse_meta_command_help() {
    match parse_command("\\?") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_none());
        }
        _ => panic!("Expected Help command"),
    }

    match parse_command("\\help match") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert_eq!(topic, Some("match".to_string()));
        }
        _ => panic!("Expected Help command with topic"),
    }
}

#[test]
fn test_parse_meta_command_connect() {
    match parse_command("\\connect myspace") {
        Command::MetaCommand(MetaCommand::Connect { space }) => {
            assert_eq!(space, "myspace");
        }
        _ => panic!("Expected Connect command"),
    }

    match parse_command("\\c myspace") {
        Command::MetaCommand(MetaCommand::Connect { space }) => {
            assert_eq!(space, "myspace");
        }
        _ => panic!("Expected Connect command with alias"),
    }
}

#[test]
fn test_parse_meta_command_connect_error() {
    match parse_command("\\connect") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
            assert!(topic.unwrap().contains("Usage"));
        }
        _ => panic!("Expected Help command with error message"),
    }
}

#[test]
fn test_parse_meta_command_disconnect() {
    assert!(matches!(
        parse_command("\\disconnect"),
        Command::MetaCommand(MetaCommand::Disconnect)
    ));
}

#[test]
fn test_parse_meta_command_conninfo() {
    assert!(matches!(
        parse_command("\\conninfo"),
        Command::MetaCommand(MetaCommand::ConnInfo)
    ));
}

#[test]
fn test_parse_meta_command_show_spaces() {
    assert!(matches!(
        parse_command("\\show_spaces"),
        Command::MetaCommand(MetaCommand::ShowSpaces)
    ));
    assert!(matches!(
        parse_command("\\l"),
        Command::MetaCommand(MetaCommand::ShowSpaces)
    ));
}

#[test]
fn test_parse_meta_command_show_tags() {
    match parse_command("\\show_tags") {
        Command::MetaCommand(MetaCommand::ShowTags { pattern }) => {
            assert!(pattern.is_none());
        }
        _ => panic!("Expected ShowTags command"),
    }

    match parse_command("\\dt person*") {
        Command::MetaCommand(MetaCommand::ShowTags { pattern }) => {
            assert_eq!(pattern, Some("person*".to_string()));
        }
        _ => panic!("Expected ShowTags command with pattern"),
    }
}

#[test]
fn test_parse_meta_command_show_edges() {
    match parse_command("\\show_edges") {
        Command::MetaCommand(MetaCommand::ShowEdges { pattern }) => {
            assert!(pattern.is_none());
        }
        _ => panic!("Expected ShowEdges command"),
    }

    match parse_command("\\de friend*") {
        Command::MetaCommand(MetaCommand::ShowEdges { pattern }) => {
            assert_eq!(pattern, Some("friend*".to_string()));
        }
        _ => panic!("Expected ShowEdges command with pattern"),
    }
}

#[test]
fn test_parse_meta_command_describe() {
    match parse_command("\\describe Person") {
        Command::MetaCommand(MetaCommand::Describe { object }) => {
            assert_eq!(object, "Person");
        }
        _ => panic!("Expected Describe command"),
    }

    match parse_command("\\d Person") {
        Command::MetaCommand(MetaCommand::Describe { object }) => {
            assert_eq!(object, "Person");
        }
        _ => panic!("Expected Describe command with alias"),
    }
}

#[test]
fn test_parse_meta_command_describe_error() {
    match parse_command("\\describe") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
            assert!(topic.unwrap().contains("Usage"));
        }
        _ => panic!("Expected Help command with error for describe"),
    }
}

#[test]
fn test_parse_meta_command_format() {
    match parse_command("\\format csv") {
        Command::MetaCommand(MetaCommand::Format { format }) => {
            assert!(matches!(format, OutputFormat::CSV));
        }
        _ => panic!("Expected Format command"),
    }
}

#[test]
fn test_parse_meta_command_format_error() {
    match parse_command("\\format") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
        }
        _ => panic!("Expected Help command with error for format"),
    }
}

#[test]
fn test_parse_meta_command_set() {
    match parse_command("\\set VAR value") {
        Command::MetaCommand(MetaCommand::Set {
            name,
            value,
            remote,
        }) => {
            assert_eq!(name, "VAR");
            assert_eq!(value, Some("value".to_string()));
            assert!(!remote);
        }
        _ => panic!("Expected Set command"),
    }

    match parse_command("\\set VAR") {
        Command::MetaCommand(MetaCommand::Set {
            name,
            value,
            remote,
        }) => {
            assert_eq!(name, "VAR");
            assert!(value.is_none());
            assert!(!remote);
        }
        _ => panic!("Expected Set command without value"),
    }

    match parse_command("\\set --remote FOO 1") {
        Command::MetaCommand(MetaCommand::Set {
            name,
            value,
            remote,
        }) => {
            assert_eq!(name, "FOO");
            assert_eq!(value, Some("1".to_string()));
            assert!(remote);
        }
        _ => panic!("Expected remote Set command"),
    }

    match parse_command("\\set --remote") {
        Command::MetaCommand(MetaCommand::ShowVariables { remote }) => {
            assert!(remote);
        }
        _ => panic!("Expected remote ShowVariables command"),
    }
}

#[test]
fn test_parse_meta_command_unset() {
    match parse_command("\\unset VAR") {
        Command::MetaCommand(MetaCommand::Unset { name, remote }) => {
            assert_eq!(name, "VAR");
            assert!(!remote);
        }
        _ => panic!("Expected Unset command"),
    }

    match parse_command("\\unset --remote VAR") {
        Command::MetaCommand(MetaCommand::Unset { name, remote }) => {
            assert_eq!(name, "VAR");
            assert!(remote);
        }
        _ => panic!("Expected remote Unset command"),
    }
}

#[test]
fn test_parse_meta_command_execute_script() {
    match parse_command("\\i script.sql") {
        Command::MetaCommand(MetaCommand::ExecuteScript { path }) => {
            assert_eq!(path, "script.sql");
        }
        _ => panic!("Expected ExecuteScript command"),
    }
}

#[test]
fn test_parse_meta_command_output_redirect() {
    match parse_command("\\o output.txt") {
        Command::MetaCommand(MetaCommand::OutputRedirect { path }) => {
            assert_eq!(path, Some("output.txt".to_string()));
        }
        _ => panic!("Expected OutputRedirect command"),
    }

    match parse_command("\\o") {
        Command::MetaCommand(MetaCommand::OutputRedirect { path }) => {
            assert!(path.is_none());
        }
        _ => panic!("Expected OutputRedirect command to close"),
    }
}

#[test]
fn test_parse_meta_command_shell_command() {
    match parse_command("\\! ls -la") {
        Command::MetaCommand(MetaCommand::ShellCommand { command }) => {
            assert_eq!(command, "ls -la");
        }
        _ => panic!("Expected ShellCommand"),
    }
}

#[test]
fn test_parse_meta_command_begin() {
    assert!(matches!(
        parse_command("\\begin"),
        Command::MetaCommand(MetaCommand::Begin)
    ));
}

#[test]
fn test_parse_meta_command_commit() {
    assert!(matches!(
        parse_command("\\commit"),
        Command::MetaCommand(MetaCommand::Commit)
    ));
}

#[test]
fn test_parse_meta_command_rollback() {
    assert!(matches!(
        parse_command("\\rollback"),
        Command::MetaCommand(MetaCommand::Rollback)
    ));

    match parse_command("\\rollback to savepoint1") {
        Command::MetaCommand(MetaCommand::RollbackTo { name }) => {
            assert_eq!(name, "savepoint1");
        }
        _ => panic!("Expected RollbackTo command"),
    }
}

#[test]
fn test_parse_meta_command_autocommit() {
    match parse_command("\\autocommit") {
        Command::MetaCommand(MetaCommand::Autocommit { value }) => {
            assert!(value.is_none());
        }
        _ => panic!("Expected Autocommit command"),
    }

    match parse_command("\\autocommit on") {
        Command::MetaCommand(MetaCommand::Autocommit { value }) => {
            assert_eq!(value, Some("on".to_string()));
        }
        _ => panic!("Expected Autocommit command with value"),
    }
}

#[test]
fn test_parse_meta_command_isolation() {
    match parse_command("\\isolation") {
        Command::MetaCommand(MetaCommand::Isolation { level }) => {
            assert!(level.is_none());
        }
        _ => panic!("Expected Isolation command"),
    }

    match parse_command("\\isolation serializable") {
        Command::MetaCommand(MetaCommand::Isolation { level }) => {
            assert_eq!(level, Some("serializable".to_string()));
        }
        _ => panic!("Expected Isolation command with level"),
    }
}

#[test]
fn test_parse_meta_command_savepoint() {
    match parse_command("\\savepoint sp1") {
        Command::MetaCommand(MetaCommand::Savepoint { name }) => {
            assert_eq!(name, "sp1");
        }
        _ => panic!("Expected Savepoint command"),
    }
}

#[test]
fn test_parse_meta_command_release() {
    match parse_command("\\release sp1") {
        Command::MetaCommand(MetaCommand::ReleaseSavepoint { name }) => {
            assert_eq!(name, "sp1");
        }
        _ => panic!("Expected ReleaseSavepoint command"),
    }
}

#[test]
fn test_parse_meta_command_txstatus() {
    assert!(matches!(
        parse_command("\\txstatus"),
        Command::MetaCommand(MetaCommand::TxStatus)
    ));
}

#[test]
fn test_parse_meta_command_edit() {
    match parse_command("\\e") {
        Command::MetaCommand(MetaCommand::Edit { file, line }) => {
            assert!(file.is_none());
            assert!(line.is_none());
        }
        _ => panic!("Expected Edit command"),
    }

    match parse_command("\\edit file.sql") {
        Command::MetaCommand(MetaCommand::Edit { file, line }) => {
            assert_eq!(file, Some("file.sql".to_string()));
            assert!(line.is_none());
        }
        _ => panic!("Expected Edit command with file"),
    }

    match parse_command("\\e file.sql +10") {
        Command::MetaCommand(MetaCommand::Edit { file, line }) => {
            assert_eq!(file, Some("file.sql".to_string()));
            assert_eq!(line, Some(10));
        }
        _ => panic!("Expected Edit command with file and line"),
    }
}

#[test]
fn test_parse_meta_command_print_buffer() {
    assert!(matches!(
        parse_command("\\p"),
        Command::MetaCommand(MetaCommand::PrintBuffer)
    ));
}

#[test]
fn test_parse_meta_command_reset_buffer() {
    assert!(matches!(
        parse_command("\\r"),
        Command::MetaCommand(MetaCommand::ResetBuffer)
    ));
}

#[test]
fn test_parse_meta_command_write_buffer() {
    match parse_command("\\w output.sql") {
        Command::MetaCommand(MetaCommand::WriteBuffer { file }) => {
            assert_eq!(file, "output.sql");
        }
        _ => panic!("Expected WriteBuffer command"),
    }
}

#[test]
fn test_parse_meta_command_history() {
    match parse_command("\\history") {
        Command::MetaCommand(MetaCommand::History { action }) => match action {
            HistoryAction::Show { count } => assert_eq!(count, Some(20)),
            _ => panic!("Expected Show action"),
        },
        _ => panic!("Expected History command"),
    }

    match parse_command("\\history 50") {
        Command::MetaCommand(MetaCommand::History { action }) => match action {
            HistoryAction::Show { count } => assert_eq!(count, Some(50)),
            _ => panic!("Expected Show action with count"),
        },
        _ => panic!("Expected History command with count"),
    }

    match parse_command("\\history clear") {
        Command::MetaCommand(MetaCommand::History { action }) => match action {
            HistoryAction::Clear => {}
            _ => panic!("Expected Clear action"),
        },
        _ => panic!("Expected History command with clear"),
    }

    match parse_command("\\history search pattern") {
        Command::MetaCommand(MetaCommand::History { action }) => match action {
            HistoryAction::Search { pattern } => assert_eq!(pattern, "pattern"),
            _ => panic!("Expected Search action"),
        },
        _ => panic!("Expected History command with search"),
    }

    match parse_command("\\history exec 5") {
        Command::MetaCommand(MetaCommand::History { action }) => match action {
            HistoryAction::Exec { id } => assert_eq!(id, 5),
            _ => panic!("Expected Exec action"),
        },
        _ => panic!("Expected History command with exec"),
    }
}

#[test]
fn test_parse_meta_command_if() {
    match parse_command("\\if VAR") {
        Command::MetaCommand(MetaCommand::If { condition }) => {
            assert_eq!(condition, "VAR");
        }
        _ => panic!("Expected If command"),
    }
}

#[test]
fn test_parse_meta_command_elif() {
    match parse_command("\\elif VAR") {
        Command::MetaCommand(MetaCommand::Elif { condition }) => {
            assert_eq!(condition, "VAR");
        }
        _ => panic!("Expected Elif command"),
    }
}

#[test]
fn test_parse_meta_command_else() {
    assert!(matches!(
        parse_command("\\else"),
        Command::MetaCommand(MetaCommand::Else)
    ));
}

#[test]
fn test_parse_meta_command_endif() {
    assert!(matches!(
        parse_command("\\endif"),
        Command::MetaCommand(MetaCommand::EndIf)
    ));
}

#[test]
fn test_parse_meta_command_explain() {
    match parse_command("\\explain MATCH (v) RETURN v") {
        Command::MetaCommand(MetaCommand::Explain { query, analyze }) => {
            assert_eq!(query, "MATCH (v) RETURN v");
            assert!(!analyze);
        }
        _ => panic!("Expected Explain command"),
    }

    match parse_command("\\explain analyze MATCH (v) RETURN v") {
        Command::MetaCommand(MetaCommand::Explain { query, analyze }) => {
            assert_eq!(query, "MATCH (v) RETURN v");
            assert!(analyze);
        }
        _ => panic!("Expected Explain command with analyze"),
    }
}

#[test]
fn test_parse_meta_command_profile() {
    match parse_command("\\profile MATCH (v) RETURN v") {
        Command::MetaCommand(MetaCommand::Profile { query }) => {
            assert_eq!(query, "MATCH (v) RETURN v");
        }
        _ => panic!("Expected Profile command"),
    }
}

#[test]
fn test_parse_meta_command_import() {
    match parse_command("\\import csv data.csv tag Person") {
        Command::MetaCommand(MetaCommand::Import {
            file_path,
            target,
            batch_size,
            ..
        }) => {
            assert_eq!(file_path, "data.csv");
            assert!(matches!(target, crate::io::ImportTarget::Vertex { .. }));
            assert!(batch_size.is_none());
        }
        _ => panic!("Expected Import command"),
    }

    match parse_command("\\import json data.json edge Friend 100") {
        Command::MetaCommand(MetaCommand::Import {
            batch_size, target, ..
        }) => {
            assert_eq!(batch_size, Some(100));
            assert!(matches!(target, crate::io::ImportTarget::Edge { .. }));
        }
        _ => panic!("Expected Import command with batch size"),
    }

    // Server-side import is opt-in and defaults to off.
    match parse_command("\\import csv data.csv tag Person") {
        Command::MetaCommand(MetaCommand::Import { server, .. }) => {
            assert!(!server);
        }
        _ => panic!("Expected Import command"),
    }
    match parse_command("\\import csv data.csv tag Person --server") {
        Command::MetaCommand(MetaCommand::Import {
            server, batch_size, ..
        }) => {
            assert!(server);
            assert!(batch_size.is_none());
        }
        _ => panic!("Expected server Import command"),
    }
    match parse_command("\\import csv data.csv tag Person 200 --server") {
        Command::MetaCommand(MetaCommand::Import {
            server, batch_size, ..
        }) => {
            assert!(server);
            assert_eq!(batch_size, Some(200));
        }
        _ => panic!("Expected server Import command with batch size"),
    }
}

#[test]
fn test_parse_meta_command_export() {
    match parse_command("\\export csv output.csv MATCH (v) RETURN v") {
        Command::MetaCommand(MetaCommand::Export {
            file_path, query, ..
        }) => {
            assert_eq!(file_path, "output.csv");
            assert_eq!(query, "MATCH (v) RETURN v");
        }
        _ => panic!("Expected Export command"),
    }

    // Server-side export is opt-in and defaults to off.
    match parse_command("\\export csv output.csv MATCH (v) RETURN v") {
        Command::MetaCommand(MetaCommand::Export { server, .. }) => {
            assert!(!server);
        }
        _ => panic!("Expected Export command"),
    }
    match parse_command("\\export csv output.csv MATCH (v) RETURN v --server") {
        Command::MetaCommand(MetaCommand::Export { server, query, .. }) => {
            assert!(server);
            assert_eq!(query, "MATCH (v) RETURN v");
        }
        _ => panic!("Expected server Export command"),
    }
}

#[test]
fn test_parse_meta_command_copy() {
    match parse_command("\\copy Person from 'data.csv'") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
            assert!(topic.as_ref().unwrap().contains("Usage"));
        }
        _ => panic!("Expected Help command with usage error"),
    }

    match parse_command("\\copy Person to 'data.csv'") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
        }
        _ => panic!("Expected Help command with usage error"),
    }
}

#[test]
fn test_parse_meta_command_x() {
    match parse_command("\\x") {
        Command::MetaCommand(MetaCommand::Format { format }) => {
            assert!(matches!(format, OutputFormat::Vertical));
        }
        _ => panic!("Expected Format Vertical command"),
    }
}

#[test]
fn test_parse_meta_command_unknown() {
    match parse_command("\\unknown") {
        Command::MetaCommand(MetaCommand::Help { topic }) => {
            assert!(topic.is_some());
            assert!(topic.unwrap().contains("Unknown"));
        }
        _ => panic!("Expected Help command with error for unknown command"),
    }
}

#[test]
fn test_copy_direction_enum() {
    let from = CopyDirection::From;
    let to = CopyDirection::To;

    assert!(matches!(from, CopyDirection::From));
    assert!(matches!(to, CopyDirection::To));
}

#[test]
fn test_history_action_enum() {
    let show = HistoryAction::Show { count: Some(10) };
    let search = HistoryAction::Search {
        pattern: "test".to_string(),
    };
    let clear = HistoryAction::Clear;
    let exec = HistoryAction::Exec { id: 5 };

    assert!(matches!(show, HistoryAction::Show { .. }));
    assert!(matches!(search, HistoryAction::Search { .. }));
    assert!(matches!(clear, HistoryAction::Clear));
    assert!(matches!(exec, HistoryAction::Exec { .. }));
}

#[test]
fn test_parse_meta_command_export_space() {
    let result =
        crate::command::parser::parse_command("\\export-space mydb /backup/mydb --format csv");
    match result {
        Command::MetaCommand(MetaCommand::ExportSpace { space_name, .. }) => {
            assert_eq!(space_name, "mydb");
        }
        _ => panic!("Expected ExportSpace command"),
    }
}

#[test]
fn test_parse_users_subcommands() {
    use crate::command::parser::types::UsersAction;

    let result = crate::command::parser::parse_command("\\users create alice secret");
    match result {
        Command::MetaCommand(MetaCommand::Users {
            action: UsersAction::Create { username, password },
        }) => {
            assert_eq!(username, "alice");
            assert_eq!(password, "secret");
        }
        _ => panic!("Expected Users create command"),
    }

    let result = crate::command::parser::parse_command("\\users grant alice admin my_space");
    match result {
        Command::MetaCommand(MetaCommand::Users {
            action:
                UsersAction::Grant {
                    username,
                    role,
                    space,
                },
        }) => {
            assert_eq!(username, "alice");
            assert_eq!(role, "admin");
            assert_eq!(space, "my_space");
        }
        _ => panic!("Expected Users grant command"),
    }
}

#[test]
fn test_parse_sessions() {
    use crate::command::parser::types::SessionsAction;

    let result = crate::command::parser::parse_command("\\sessions list");
    assert!(matches!(
        result,
        Command::MetaCommand(MetaCommand::Sessions {
            action: SessionsAction::List
        })
    ));

    let result = crate::command::parser::parse_command("\\sessions kill 42");
    match result {
        Command::MetaCommand(MetaCommand::Sessions {
            action: SessionsAction::Kill { session_id },
        }) => assert_eq!(session_id, 42),
        _ => panic!("Expected Sessions kill command"),
    }
}

#[test]
fn test_parse_validate() {
    let result = crate::command::parser::parse_command("\\validate MATCH (p:person) RETURN p");
    match result {
        Command::MetaCommand(MetaCommand::Validate { query }) => {
            assert_eq!(query, "MATCH (p:person) RETURN p");
        }
        _ => panic!("Expected Validate command"),
    }
}

#[test]
fn test_parse_fulltext() {
    use crate::command::parser::types::FulltextAction;

    let result = crate::command::parser::parse_command("\\fulltext search 1 person name hello 20");
    match result {
        Command::MetaCommand(MetaCommand::Fulltext {
            action:
                FulltextAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    query,
                    limit,
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(tag_name, "person");
            assert_eq!(field_name, "name");
            assert_eq!(query, "hello");
            assert_eq!(limit, 20);
        }
        _ => panic!("Expected Fulltext search command"),
    }

    let result = crate::command::parser::parse_command("\\fulltext indexes");
    assert!(matches!(
        result,
        Command::MetaCommand(MetaCommand::Fulltext {
            action: FulltextAction::ListIndexes
        })
    ));
}

#[test]
fn test_parse_vector() {
    use crate::command::parser::types::VectorAction;

    let result =
        crate::command::parser::parse_command("\\vector search 1 person embedding 0.1,0.2,0.3 5");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action:
                VectorAction::Search {
                    space_id,
                    tag_name,
                    field_name,
                    vector,
                    limit,
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(tag_name, "person");
            assert_eq!(field_name, "embedding");
            assert_eq!(vector, vec![0.1, 0.2, 0.3]);
            assert_eq!(limit, 5);
        }
        _ => panic!("Expected Vector search command"),
    }

    let result = crate::command::parser::parse_command("\\vector create 1 person embedding 128");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action:
                VectorAction::Create {
                    space_id,
                    vector_size,
                    ..
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(vector_size, 128);
        }
        _ => panic!("Expected Vector create command"),
    }

    let result = crate::command::parser::parse_command("\\vector get 1 person embedding point-1");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action:
                VectorAction::Get {
                    space_id,
                    tag_name,
                    field_name,
                    point_id,
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(tag_name, "person");
            assert_eq!(field_name, "embedding");
            assert_eq!(point_id, "point-1");
        }
        _ => panic!("Expected Vector get command"),
    }

    let result =
        crate::command::parser::parse_command("\\vector scroll 1 person embedding 50 next-9");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action:
                VectorAction::Scroll {
                    space_id,
                    limit,
                    offset,
                    ..
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(limit, 50);
            assert_eq!(offset, Some("next-9".to_string()));
        }
        _ => panic!("Expected Vector scroll command"),
    }

    let result = crate::command::parser::parse_command("\\vector scroll 1 person embedding");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action: VectorAction::Scroll { limit, offset, .. },
        }) => {
            assert_eq!(limit, 10);
            assert_eq!(offset, None);
        }
        _ => panic!("Expected Vector scroll with defaults"),
    }

    let result = crate::command::parser::parse_command(
        "\\vector set-payload 1 person embedding a,b {\"color\": \"red\"}",
    );
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action:
                VectorAction::SetPayload {
                    space_id,
                    point_ids,
                    payload,
                    ..
                },
        }) => {
            assert_eq!(space_id, 1);
            assert_eq!(point_ids, vec!["a".to_string(), "b".to_string()]);
            assert_eq!(
                payload["color"],
                serde_json::Value::String("red".to_string())
            );
        }
        _ => panic!("Expected Vector set-payload command"),
    }

    let result = crate::command::parser::parse_command(
        "\\vector merge-payload 1 person embedding a {\"n\": 1}",
    );
    assert!(matches!(
        result,
        Command::MetaCommand(MetaCommand::Vector {
            action: VectorAction::MergePayloadFields { .. },
        })
    ));

    let result =
        crate::command::parser::parse_command("\\vector delete-payload 1 person embedding a,b x,y");
    match result {
        Command::MetaCommand(MetaCommand::Vector {
            action: VectorAction::DeletePayload {
                point_ids, keys, ..
            },
        }) => {
            assert_eq!(point_ids, vec!["a".to_string(), "b".to_string()]);
            assert_eq!(keys, vec!["x".to_string(), "y".to_string()]);
        }
        _ => panic!("Expected Vector delete-payload command"),
    }

    assert!(matches!(
        crate::command::parser::parse_command("\\vector set-payload 1 person embedding a [1,2]"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
    assert!(matches!(
        crate::command::parser::parse_command("\\vector delete-payload 1 person embedding a, x"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
}

#[test]
fn test_parse_transactions_subcommands() {
    use crate::command::parser::types::TransactionsAction;

    match parse_command("\\transactions list") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::List,
        }) => {}
        _ => panic!("Expected Transactions::List"),
    }

    match parse_command("\\transactions metrics") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::Metrics,
        }) => {}
        _ => panic!("Expected Transactions::Metrics"),
    }

    match parse_command("\\transactions kill 42") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::Kill { id },
        }) => assert_eq!(id, 42),
        _ => panic!("Expected Transactions::Kill"),
    }

    match parse_command("\\transactions retry-outbox 7") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::RetryOutbox { id },
        }) => assert_eq!(id, 7),
        _ => panic!("Expected Transactions::RetryOutbox"),
    }

    // Missing id and unknown subcommand surface as help text, not a command.
    assert!(matches!(
        parse_command("\\transactions kill"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
    assert!(matches!(
        parse_command("\\transactions frobnicate"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
}

#[test]
fn test_parse_transactions_savepoint_subcommands() {
    use crate::command::parser::types::TransactionsAction;

    match parse_command("\\transactions savepoint-create 42 mysp") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::SavepointCreate { id, name },
        }) => {
            assert_eq!(id, 42);
            assert_eq!(name, "mysp");
        }
        _ => panic!("Expected Transactions::SavepointCreate"),
    }

    match parse_command("\\transactions savepoint-list 42") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::SavepointList { id },
        }) => assert_eq!(id, 42),
        _ => panic!("Expected Transactions::SavepointList"),
    }

    match parse_command("\\transactions savepoint-rollback 42 7") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::SavepointRollback { id, savepoint_id },
        }) => {
            assert_eq!(id, 42);
            assert_eq!(savepoint_id, 7);
        }
        _ => panic!("Expected Transactions::SavepointRollback"),
    }

    match parse_command("\\transactions savepoint-release 42 7") {
        Command::MetaCommand(MetaCommand::Transactions {
            action: TransactionsAction::SavepointRelease { id, savepoint_id },
        }) => {
            assert_eq!(id, 42);
            assert_eq!(savepoint_id, 7);
        }
        _ => panic!("Expected Transactions::SavepointRelease"),
    }

    // Missing arguments surface as help text, not a command.
    assert!(matches!(
        parse_command("\\transactions savepoint-create 42"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
    assert!(matches!(
        parse_command("\\transactions savepoint-rollback 42"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
    assert!(matches!(
        parse_command("\\transactions savepoint-rollback abc 7"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
}

#[test]
fn test_parse_migration_watch_follow() {
    use crate::command::parser::types::MigrationAction;

    // Plain watch keeps polling semantics.
    match parse_command("\\migration watch myspace mytag") {
        Command::MetaCommand(MetaCommand::Migration {
            action: MigrationAction::Watch { follow, .. },
        }) => assert!(!follow),
        _ => panic!("Expected Migration::Watch"),
    }

    // --follow selects the SSE progress stream; edge marker still parses.
    match parse_command("\\migration watch myspace mytag --follow") {
        Command::MetaCommand(MetaCommand::Migration {
            action:
                MigrationAction::Watch {
                    space,
                    label,
                    is_edge,
                    follow,
                },
        }) => {
            assert_eq!(space, "myspace");
            assert_eq!(label, "mytag");
            assert!(!is_edge);
            assert!(follow);
        }
        _ => panic!("Expected Migration::Watch with follow"),
    }
    match parse_command("\\migration watch myspace mytag edge --follow") {
        Command::MetaCommand(MetaCommand::Migration {
            action: MigrationAction::Watch {
                is_edge, follow, ..
            },
        }) => {
            assert!(is_edge);
            assert!(follow);
        }
        _ => panic!("Expected edge Migration::Watch with follow"),
    }
}

#[test]
fn test_parse_sync_requeue_rejects_mixed_filters_and_ids() {
    // event_ids and filter arguments are mutually exclusive.
    assert!(matches!(
        parse_command("\\sync requeue tgt 1 2 100 9,10"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
    // Invalid event ids fail at parse time instead of being silently dropped.
    assert!(matches!(
        parse_command("\\sync requeue tgt 1 2 100 abc"),
        Command::MetaCommand(MetaCommand::Help { topic: Some(_) })
    ));
}
