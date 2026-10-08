use crate::command::parser::types::MetaCommand;
use crate::output::formatter::OutputFormat;

pub fn parse_format(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\format <table|csv|json|vertical|html>".to_string())
    } else {
        match OutputFormat::parse(arg) {
            Some(fmt) => Ok(MetaCommand::Format { format: fmt }),
            None => Err(format!(
                "Unknown format: '{}'. Available: table, csv, json, vertical, html",
                arg
            )),
        }
    }
}

pub fn parse_pager(arg: &str) -> Result<MetaCommand, String> {
    let command = if arg.is_empty() {
        None
    } else {
        Some(arg.to_string())
    };
    Ok(MetaCommand::Pager { command })
}

pub fn parse_shell_command(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\! <shell_command>".to_string())
    } else {
        Ok(MetaCommand::ShellCommand {
            command: arg.to_string(),
        })
    }
}

pub fn parse_if(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\if <condition>".to_string())
    } else {
        Ok(MetaCommand::If {
            condition: arg.to_string(),
        })
    }
}

pub fn parse_elif(arg: &str) -> Result<MetaCommand, String> {
    if arg.is_empty() {
        Err("Usage: \\elif <condition>".to_string())
    } else {
        Ok(MetaCommand::Elif {
            condition: arg.to_string(),
        })
    }
}
