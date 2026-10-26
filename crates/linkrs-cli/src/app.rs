use crate::command::executor::CommandExecutor;
use crate::command::parser::{self};
use crate::command::script::is_statement_complete;
use crate::input::handler::InputHandler;
use colored::Colorize;

use crate::session::SessionManager;
use crate::utils::error::Result;

#[derive(Debug, Default, Clone)]
pub struct RunOptions {
    pub database: Option<String>,
    pub command: Option<String>,
    pub file: Option<String>,
    pub format: Option<String>,
    pub config_format: Option<String>,
    pub quiet: bool,
    pub variables: Vec<String>,
    pub force: bool,
    pub single_transaction: bool,
}

pub async fn run_with(
    session_mgr: &mut SessionManager,
    executor: &mut CommandExecutor,
    options: &RunOptions,
) -> Result<()> {
    if let Some(ref space) = options.database {
        match session_mgr.switch_space(space).await {
            Ok(()) => {
                if !options.quiet {
                    println!("Space changed to '{}'", space);
                }
            }
            Err(e) => {
                eprintln!(
                    "{}: Failed to switch to space '{}': {}",
                    "WARNING".yellow().bold(),
                    space,
                    e
                );
            }
        }
    }

    if !options.variables.is_empty() {
        for var_def in &options.variables {
            if let Some((name, value)) = var_def.split_once('=') {
                if let Some(session) = session_mgr.session_mut() {
                    session.set_variable(name.to_string(), value.to_string())?;
                }
            }
        }
    }

    if let Some(ref cmd) = options.command {
        let command = parser::parse_command(cmd);
        match executor.execute(command, session_mgr).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                if !options.force {
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    if let Some(ref file) = options.file {
        let command =
            parser::Command::MetaCommand(parser::MetaCommand::ExecuteScript { path: file.clone() });
        match executor.execute(command, session_mgr).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                if !options.force {
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    run_repl(session_mgr, executor).await
}

pub async fn run_repl(
    session_mgr: &mut SessionManager,
    executor: &mut CommandExecutor,
) -> Result<()> {
    let mut input_handler = InputHandler::new()?;

    loop {
        let prompt = session_mgr.prompt();

        let line = match input_handler.read_line(&prompt)? {
            Some(line) => line,
            None => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let mut full_input = line;

        while !is_statement_complete(&full_input) {
            let cont_prompt = session_mgr.continuation_prompt();

            let next_line = match input_handler.read_line(&cont_prompt)? {
                Some(line) => line,
                None => break,
            };

            full_input.push('\n');
            full_input.push_str(&next_line);
        }

        if full_input.trim().eq_ignore_ascii_case("exit")
            || full_input.trim().eq_ignore_ascii_case("quit")
            || full_input.trim() == "\\q"
        {
            break;
        }

        input_handler.add_history(&full_input, session_mgr.current_space());

        let command = parser::parse_command(&full_input);

        if let parser::Command::MetaCommand(parser::MetaCommand::History { action }) = command {
            match handle_history_action(&mut input_handler, executor, &action)? {
                Some(retrieved) => {
                    let retrieved_command = parser::parse_command(&retrieved);
                    if let Err(e) = executor.execute(retrieved_command, session_mgr).await {
                        eprintln!("{}: {}", "ERROR".red().bold(), e);
                    }
                }
                None => {}
            }
            continue;
        }

        match executor.execute(command, session_mgr).await {
            Ok(should_exit) => {
                if should_exit {
                    break;
                }
            }
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
            }
        }
    }

    input_handler.save_history();
    let _ = session_mgr.disconnect().await;
    Ok(())
}

fn handle_history_action(
    input_handler: &mut InputHandler,
    executor: &mut CommandExecutor,
    action: &crate::command::parser::HistoryAction,
) -> Result<Option<String>> {
    use crate::command::parser::HistoryAction;
    match action {
        HistoryAction::Show { count } => {
            let entries = input_handler.history_manager().recent(count.unwrap_or(20));
            if entries.is_empty() {
                executor.write_output("(no history)")?;
            } else {
                let mut out = String::new();
                for entry in entries {
                    out.push_str(&format!("  {}  {}\n", entry.id, entry.command));
                }
                executor.write_output(out.trim_end())?;
            }
            Ok(None)
        }
        HistoryAction::Search { pattern } => {
            let entries = input_handler.history_manager().search(pattern);
            if entries.is_empty() {
                executor.write_output("(no matches)")?;
            } else {
                let mut out = String::new();
                for entry in entries {
                    out.push_str(&format!("  {}  {}\n", entry.id, entry.command));
                }
                executor.write_output(out.trim_end())?;
            }
            Ok(None)
        }
        HistoryAction::Clear => {
            input_handler.clear_history()?;
            executor.write_output("History cleared.")?;
            Ok(None)
        }
        HistoryAction::Exec { id } => {
            let command = input_handler
                .history_manager()
                .get_by_id(*id)
                .map(|entry| entry.command.clone());
            match command {
                Some(command) => {
                    if matches!(
                        parser::parse_command(&command),
                        parser::Command::MetaCommand(parser::MetaCommand::History {
                            action: HistoryAction::Exec { .. },
                        })
                    ) {
                        executor.write_output(
                            "Refusing to exec a history entry that is itself `\\history exec`.",
                        )?;
                        Ok(None)
                    } else {
                        Ok(Some(command))
                    }
                }
                None => {
                    executor.write_output(&format!("No history entry with ID {}", id))?;
                    Ok(None)
                }
            }
        }
    }
}
