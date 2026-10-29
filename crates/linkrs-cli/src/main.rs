use clap::Parser;
use colored::Colorize;

use linkrs_cli::app::{run_with, RunOptions};
use linkrs_cli::cli::Cli;
use linkrs_cli::command::executor::CommandExecutor;
use linkrs_cli::config::settings::Config;
use linkrs_cli::output::formatter::{OutputFormat, OutputFormatter};
use linkrs_cli::session::SessionManager;
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

    if let Some(fmt) = OutputFormat::parse(&cli.exec.format) {
        formatter.set_format(fmt);
    } else if let Some(fmt) = OutputFormat::parse(&config.output.format) {
        formatter.set_format(fmt);
    }

    let mut executor =
        CommandExecutor::with_options(formatter, cli.exec.force, cli.exec.single_transaction);

    match session_mgr.connect(&user, &password).await {
        Ok(()) => {
            if !cli.exec.quiet {
                println!("Connected to Linkrs at {}:{} as {}", host, port, user);
            }
        }
        Err(e) => {
            eprintln!("{}: Failed to connect: {}", "ERROR".red().bold(), e);
            eprintln!("Starting in offline mode. Use \\login <username> to connect to a server.");
        }
    }

    let options = RunOptions {
        database: cli.exec.database.clone(),
        command: cli.exec.command.clone(),
        file: cli.exec.file.clone(),
        format: None,
        config_format: Some(config.output.format.clone()),
        quiet: cli.exec.quiet,
        variables: cli.exec.variables.clone(),
        force: cli.exec.force,
        single_transaction: cli.exec.single_transaction,
    };

    run_with(&mut session_mgr, &mut executor, &options).await
}

fn read_password() -> Result<String> {
    rpassword::prompt_password("Password: ").map_err(|e| {
        linkrs_cli::utils::error::CliError::auth(format!("Failed to read password: {}", e))
    })
}
