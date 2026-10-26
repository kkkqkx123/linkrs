use clap::Parser;
use colored::Colorize;

use linkrs_cli::cli::Cli;
use linkrs_cli::command::executor::CommandExecutor;
use linkrs_cli::command::parser::types::HistoryAction;
use linkrs_cli::command::parser::{self};
use linkrs_cli::command::script::is_statement_complete;
use linkrs_cli::config::settings::Config;
use linkrs_cli::input::handler::InputHandler;
use linkrs_cli::output::formatter::{OutputFormat, OutputFormatter};
use linkrs_cli::session::manager::SessionManager;
use linkrs_cli::utils::error::Result;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli).await {
        eprintln!("{}: {}", "ERROR".red().bold(), e);
        std::process::exit(1);
    }
}

async fn run(mut cli: Cli) -> Result<()> {
    if let Some(subcommand) = cli.subcommand.take() {
        match subcommand {
            linkrs_cli::cli::Commands::Service { action } => match action {
                linkrs_cli::cli::ServiceAction::Start {
                    bin,
                    config,
                    data_dir,
                    http_port,
                    grpc_port,
                    foreground,
                } => {
                    linkrs_cli::service::start(
                        bin.as_deref(),
                        config.as_deref(),
                        data_dir.as_deref(),
                        http_port,
                        grpc_port,
                        foreground,
                    )
                    .map_err(|e| linkrs_cli::utils::error::CliError::CommandError(e))?;
                }
                linkrs_cli::cli::ServiceAction::Stop => {
                    linkrs_cli::service::stop()
                        .map_err(|e| linkrs_cli::utils::error::CliError::CommandError(e))?;
                }
                linkrs_cli::cli::ServiceAction::Restart => {
                    linkrs_cli::service::restart(None, None, None, None, None)
                        .map_err(|e| linkrs_cli::utils::error::CliError::CommandError(e))?;
                }
                linkrs_cli::cli::ServiceAction::Status => {
                    linkrs_cli::service::status()
                        .map_err(|e| linkrs_cli::utils::error::CliError::CommandError(e))?;
                }
            },
        }
        return Ok(());
    }

    let config = Config::load().unwrap_or_default();

    let host = if cli.host == "127.0.0.1" && config.connection.default_host != "127.0.0.1" {
        config.connection.default_host.clone()
    } else {
        cli.host.clone()
    };

    let port = if cli.port == 8080 && config.connection.default_port != 8080 {
        config.connection.default_port
    } else {
        cli.port
    };

    let mut session_mgr = SessionManager::new_http(&host, port)?;

    let user = if cli.user == "root" && config.connection.default_user != "root" {
        config.connection.default_user.clone()
    } else {
        cli.user.clone()
    };

    let password = if cli.password {
        read_password()?
    } else {
        String::new()
    };

    let mut formatter = OutputFormatter::new();

    if let Some(fmt) = OutputFormat::parse(&cli.format) {
        formatter.set_format(fmt);
    } else if let Some(fmt) = OutputFormat::parse(&config.output.format) {
        formatter.set_format(fmt);
    }

    let mut executor = CommandExecutor::with_options(formatter, cli.force, cli.single_transaction);

    match session_mgr.connect(&user, &password).await {
        Ok(()) => {
            if !cli.quiet {
                println!("Connected to Linkrs at {}:{} as {}", host, port, user);
            }
        }
        Err(e) => {
            eprintln!("{}: Failed to connect: {}", "ERROR".red().bold(), e);
            eprintln!("Starting in offline mode. Use \\login <username> to connect to a server.");
        }
    }

    if let Some(ref space) = cli.database {
        match session_mgr.switch_space(space).await {
            Ok(()) => {
                if !cli.quiet {
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

    if !cli.variables.is_empty() {
        for var_def in &cli.variables {
            if let Some((name, value)) = var_def.split_once('=') {
                if let Some(session) = session_mgr.session_mut() {
                    session.set_variable(name.to_string(), value.to_string())?;
                }
            }
        }
    }

    if let Some(ref cmd) = cli.command {
        let command = parser::parse_command(cmd);
        match executor.execute(command, &mut session_mgr).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                if !cli.force {
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    if let Some(ref file) = cli.file {
        let command =
            parser::Command::MetaCommand(parser::MetaCommand::ExecuteScript { path: file.clone() });
        match executor.execute(command, &mut session_mgr).await {
            Ok(_) => {}
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                if !cli.force {
                    std::process::exit(1);
                }
            }
        }
        return Ok(());
    }

    run_repl(&mut session_mgr, &mut executor).await
}

async fn run_repl(session_mgr: &mut SessionManager, executor: &mut CommandExecutor) -> Result<()> {
    let mut input_handler = InputHandler::new()?;

    loop {
        let prompt = session_mgr
            .session()
            .map(|s| s.prompt())
            .unwrap_or_else(|| "linkrs=# ".to_string());

        let line = match input_handler.read_line(&prompt)? {
            Some(line) => line,
            None => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let mut full_input = line;

        while !is_statement_complete(&full_input) {
            let cont_prompt = session_mgr
                .session()
                .map(|s| s.continuation_prompt())
                .unwrap_or_else(|| "linkrs-# ".to_string());

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

        // Record every submitted input so `\history` reflects this session.
        // HistoryManager dedups consecutive repeats internally.
        input_handler.add_history(&full_input, session_mgr.current_space());

        let command = parser::parse_command(&full_input);

        // `\history` is served from the line editor's history here, where the
        // entries live. The executor arm stays as a fallback for scripts and
        // non-interactive runs, which have no line editor.
        if let parser::Command::MetaCommand(parser::MetaCommand::History { action }) = command {
            match handle_history_action(&mut input_handler, &mut *executor, &action)? {
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

fn read_password() -> Result<String> {
    rpassword::prompt_password("Password: ").map_err(|e| {
        linkrs_cli::utils::error::CliError::auth(format!("Failed to read password: {}", e))
    })
}

/// Serve `\history` from the REPL line editor's history.
///
/// Returns the command text to run for `exec`, if any. A retrieved command
/// that is itself a `\history exec` is rejected so executions cannot loop.
fn handle_history_action(
    input_handler: &mut InputHandler,
    executor: &mut CommandExecutor,
    action: &HistoryAction,
) -> Result<Option<String>> {
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
