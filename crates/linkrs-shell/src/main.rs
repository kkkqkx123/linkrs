use clap::Parser;
use colored::Colorize;
use linkrs_cli::app::{run_with, RunOptions};
use linkrs_cli::command::executor::CommandExecutor;
use linkrs_cli::output::formatter::{OutputFormat, OutputFormatter};
mod backend;

use backend::EmbeddedBackend;
use linkrs_cli::session::SessionManager;


#[derive(Parser)]
#[clap(version = "0.1.0", author = "Linkrs Contributors")]
struct Args {
    /// Database file path or :memory:
    #[arg(default_value = ":memory:")]
    database: String,

    /// Open an in-memory database
    #[arg(long)]
    in_memory: bool,

    /// Switch to a space after opening
    #[arg(short = 'd', long)]
    space: Option<String>,

    /// Execute a single command and exit
    #[arg(short = 'c', long)]
    command: Option<String>,

    /// Execute commands from a file and exit
    #[arg(short = 'f', long)]
    file: Option<String>,

    /// Output format
    #[arg(long)]
    format: Option<String>,

    /// Quiet mode
    #[arg(short, long)]
    quiet: bool,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let backend = if args.in_memory || args.database == ":memory:" {
        match EmbeddedBackend::open_in_memory() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{}: {}", "ERROR".red().bold(), e);
                std::process::exit(1);
            }
        }
    } else {
        match EmbeddedBackend::open(&args.database) {
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
    if let Some(fmt) = args.format.as_deref().and_then(OutputFormat::parse) {
        formatter.set_format(fmt);
    }

    let mut executor = CommandExecutor::with_options(formatter, false, false);

    let options = RunOptions {
        database: args.space,
        command: args.command,
        file: args.file,
        format: args.format,
        config_format: None,
        quiet: args.quiet,
        variables: Vec::new(),
        force: false,
        single_transaction: false,
    };

    let run_result = run_with(&mut session_mgr, &mut executor, &options).await;
    drop(executor);
    let _ = std::thread::spawn(move || drop(session_mgr)).join();
    if let Err(e) = run_result {
        eprintln!("{}: {}", "ERROR".red().bold(), e);
        std::process::exit(1);
    }
}
