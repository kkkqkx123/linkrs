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

/// Bounded timed replay of the current buffer. Both arguments are required
/// so the loop always returns to the REPL without signal handling.
pub fn parse_watch(arg: &str) -> Result<MetaCommand, String> {
    const USAGE: &str = "Usage: \\watch <interval_secs> <count>";
    let parts: Vec<&str> = arg.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(USAGE.to_string());
    }
    let interval_secs: f64 = parts[0]
        .parse()
        .map_err(|_| format!("Invalid interval: '{}'. {}", parts[0], USAGE))?;
    if !interval_secs.is_finite() || interval_secs <= 0.0 || interval_secs > 3600.0 {
        return Err(format!(
            "Interval must be within (0, 3600] seconds. {}",
            USAGE
        ));
    }
    let count: usize = parts[1]
        .parse()
        .map_err(|_| format!("Invalid count: '{}'. {}", parts[1], USAGE))?;
    if count == 0 || count > 1000 {
        return Err(format!("Count must be within [1, 1000]. {}", USAGE));
    }
    Ok(MetaCommand::WatchBuffer {
        interval_secs,
        count,
    })
}

/// Null placeholder for output rendering. Parsed with the output-control
/// branch alongside `\format`; no new `\set` variable name is introduced.
pub fn parse_null(arg: &str) -> Result<MetaCommand, String> {
    let text = arg.trim();
    if text.is_empty() {
        Ok(MetaCommand::SetNull { text: None })
    } else {
        Ok(MetaCommand::SetNull {
            text: Some(text.to_string()),
        })
    }
}
