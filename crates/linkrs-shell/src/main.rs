use clap::Parser;
use colored::Colorize;
use linkrs_cli::app::{run_with, RunOptions};
use linkrs_cli::cli::ExecArgs;
use linkrs_cli::command::executor::CommandExecutor;
use linkrs_cli::output::formatter::{OutputFormat, OutputFormatter};
mod backend;

use backend::EmbeddedBackend;
use linkrs_cli::session::SessionManager;

#[derive(Parser)]
#[clap(
    name = "linkrs",
    version = "0.1.0",
    author = "Linkrs Contributors",
    about = "Linkrs shell - embedded interactive client for a local database"
)]
struct Args {
    /// Database file path or :memory:
    #[arg(default_value = ":memory:")]
    database_path: String,

    /// Open an in-memory database
    #[arg(long)]
    in_memory: bool,

    /// Open the database in read-only mode (shared lock, writes rejected)
    #[arg(long)]
    readonly: bool,

    #[command(flatten)]
    exec: ExecArgs,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let read_only = args.readonly
        || std::env::var("LINKRS_READONLY")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    let backend = if args.in_memory || args.database_path == ":memory:" {
        match EmbeddedBackend::open_in_memory() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                std::process::exit(1);
            }
        }
    } else {
        if read_only && !args.exec.quiet {
            println!("Opening database in read-only mode.");
        }
        match EmbeddedBackend::open_with_options(&args.database_path, read_only) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                std::process::exit(1);
            }
        }
    };
    let mut session_mgr = SessionManager::new_embedded(Box::new(backend));

    if let Err(e) = session_mgr.connect("embedded", "").await {
        eprintln!("{}: {}", "WARNING".yellow().bold(), e);
    }

    let mut formatter = OutputFormatter::new();
    if let Some(fmt) = OutputFormat::parse(&args.exec.format) {
        formatter.set_format(fmt);
    }

    let mut executor = CommandExecutor::with_options(formatter, args.exec.force, args.exec.single_transaction);

    let options = RunOptions {
        database: args.exec.database.clone(),
        command: args.exec.command.clone(),
        file: args.exec.file.clone(),
        format: Some(args.exec.format.clone()),
        config_format: None,
        quiet: args.exec.quiet,
        variables: args.exec.variables.clone(),
        force: args.exec.force,
        single_transaction: args.exec.single_transaction,
    };

    let run_result = run_with(&mut session_mgr, &mut executor, &options).await;
    drop(executor);
    // The embedded session owns tokio blocking resources; dropping it inside
    // the async runtime panics, so release it from a blocking thread.
    tokio::task::spawn_blocking(move || drop(session_mgr))
        .await
        .expect("embedded session drop task panicked");
    if let Err(e) = run_result {
        eprintln!("{}: {}", "ERROR".red().bold(), e);
        std::process::exit(1);
    }
}
