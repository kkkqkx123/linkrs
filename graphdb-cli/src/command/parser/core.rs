use crate::command::parser::types::{Command, MetaCommand};

pub fn parse_command(input: &str) -> Command {
    let trimmed = input.trim();

    if trimmed.is_empty() {
        return Command::Empty;
    }

    if trimmed.starts_with('\\') {
        match parse_meta_command(trimmed) {
            Ok(cmd) => Command::MetaCommand(cmd),
            Err(msg) => Command::MetaCommand(MetaCommand::Help { topic: Some(msg) }),
        }
    } else {
        Command::Query(trimmed.to_string())
    }
}

fn parse_meta_command(input: &str) -> Result<MetaCommand, String> {
    let trimmed = input.trim_start_matches('\\');
    let parts: Vec<&str> = trimmed.splitn(2, whitespace_or_end).collect();
    let cmd = parts[0].to_lowercase();
    let arg = parts.get(1).map(|s| s.trim()).unwrap_or("");

    match cmd.as_str() {
        "q" | "quit" => Ok(MetaCommand::Quit),
        "q!" => Ok(MetaCommand::ForceQuit),
        "?" => Ok(MetaCommand::Help { topic: None }),
        "help" => {
            let topic = if arg.is_empty() {
                None
            } else {
                Some(arg.to_string())
            };
            Ok(MetaCommand::Help { topic })
        }
        "connect" | "c" => crate::command::parser::meta::connection::parse(arg),
        "disconnect" => Ok(MetaCommand::Disconnect),
        "conninfo" => Ok(MetaCommand::ConnInfo),
        "show_spaces" | "l" => Ok(MetaCommand::ShowSpaces),
        "show_tags" | "dt" => crate::command::parser::meta::schema::parse_show_tags(arg),
        "show_edges" | "de" => crate::command::parser::meta::schema::parse_show_edges(arg),
        "show_indexes" | "di" => crate::command::parser::meta::schema::parse_show_indexes(arg),
        "show_users" | "du" => Ok(MetaCommand::ShowUsers),
        "show_functions" | "df" => Ok(MetaCommand::ShowFunctions),
        "describe" | "d" => crate::command::parser::meta::schema::parse_describe(arg),
        "describe_edge" => crate::command::parser::meta::schema::parse_describe_edge(arg),
        "format" => crate::command::parser::meta::control::parse_format(arg),
        "pager" => crate::command::parser::meta::control::parse_pager(arg),
        "timing" => Ok(MetaCommand::Timing),
        "set" => crate::command::parser::meta::variables::parse_set(arg),
        "unset" => crate::command::parser::meta::variables::parse_unset(arg),
        "i" => crate::command::parser::meta::io::parse_execute_script(arg),
        "ir" => crate::command::parser::meta::io::parse_execute_script_raw(arg),
        "o" => crate::command::parser::meta::io::parse_output_redirect(arg),
        "!" => crate::command::parser::meta::control::parse_shell_command(arg),
        "version" => Ok(MetaCommand::Version),
        "copyright" => Ok(MetaCommand::Copyright),
        "x" => Ok(MetaCommand::Format {
            format: crate::output::formatter::OutputFormat::Vertical,
        }),
        "begin" => Ok(MetaCommand::Begin),
        "commit" => Ok(MetaCommand::Commit),
        "rollback" => crate::command::parser::meta::transaction::parse_rollback(arg),
        "autocommit" => crate::command::parser::meta::transaction::parse_autocommit(arg),
        "isolation" => crate::command::parser::meta::transaction::parse_isolation(arg),
        "savepoint" => crate::command::parser::meta::transaction::parse_savepoint(arg),
        "release" => crate::command::parser::meta::transaction::parse_release(arg),
        "txstatus" => Ok(MetaCommand::TxStatus),
        "e" | "edit" => crate::command::parser::meta::buffer::parse_edit(arg),
        "p" => Ok(MetaCommand::PrintBuffer),
        "r" => Ok(MetaCommand::ResetBuffer),
        "w" => crate::command::parser::meta::buffer::parse_write_buffer(arg),
        "history" => crate::command::parser::meta::buffer::parse_history(arg),
        "if" => crate::command::parser::meta::control::parse_if(arg),
        "elif" => crate::command::parser::meta::control::parse_elif(arg),
        "else" => Ok(MetaCommand::Else),
        "endif" => Ok(MetaCommand::EndIf),
        "explain" => crate::command::parser::meta::analyze::parse_explain(arg),
        "profile" => crate::command::parser::meta::analyze::parse_profile(arg),
        "import" => crate::command::parser::meta::io::parse_import(arg),
        "export" => crate::command::parser::meta::io::parse_export(arg),
        "copy" => crate::command::parser::meta::io::parse_copy(arg),
        "dump" => crate::command::parser::meta::io::parse_dump(arg),
        "restore" => crate::command::parser::meta::io::parse_restore(arg),
        "export-space" => crate::command::parser::meta::io::parse_export_space(arg),
        "export-schema" => crate::command::parser::meta::io::parse_export_schema(arg),
        "import-schema" => crate::command::parser::meta::io::parse_import_schema(arg),
        _ => Err(format!("Unknown command: \\ {}", cmd)),
    }
}

fn whitespace_or_end(c: char) -> bool {
    c.is_whitespace()
}
