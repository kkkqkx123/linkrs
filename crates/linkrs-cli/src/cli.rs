use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[clap(
    name = "linkrs-cli",
    version = env!("CARGO_PKG_VERSION"),
    about = "Linkrs CLI - Interactive command-line client for Linkrs",
    long_about = "Linkrs CLI is an interactive command-line client for Linkrs,\
                  similar to PostgreSQL's psql. It supports GQL query execution,\
                  schema inspection, and various output formats."
)]
pub struct Cli {
    #[clap(long, default_value = "127.0.0.1", help = "Server host")]
    pub host: String,

    #[clap(short, long, default_value_t = 8080, help = "Server port")]
    pub port: u16,

    #[clap(
        short,
        long,
        default_value = "root",
        help = "Username for authentication"
    )]
    pub user: String,

    #[clap(short = 'W', long, help = "Prompt for password")]
    pub password: bool,

    #[clap(short, long, help = "Space name to connect to")]
    pub database: Option<String>,

    #[clap(short, long, help = "Execute single command and exit")]
    pub command: Option<String>,

    #[clap(short = 'f', long = "file", help = "Execute commands from file")]
    pub file: Option<String>,

    #[clap(short, long, help = "Output file for query results")]
    pub output: Option<String>,

    #[clap(
        long,
        default_value = "table",
        help = "Output format (table, csv, json, vertical, html)"
    )]
    pub format: String,

    #[clap(short, long, help = "Quiet mode - suppress non-essential output")]
    pub quiet: bool,

    #[clap(
        short = '1',
        long = "single-transaction",
        help = "Execute commands in a single transaction"
    )]
    pub single_transaction: bool,

    #[clap(long = "force", help = "Continue processing after errors")]
    pub force: bool,

    #[clap(
        short = 'v',
        long = "variable",
        value_name = "NAME=VALUE",
        help = "Set variable before execution"
    )]
    pub variables: Vec<String>,

    #[command(subcommand)]
    pub subcommand: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Manage a local linkrs-server process
    Service {
        #[command(subcommand)]
        action: ServiceAction,
    },
}

#[derive(Subcommand, Debug)]
pub enum ServiceAction {
    /// Start linkrs-server
    Start {
        #[clap(long, help = "Path to linkrs-server binary")]
        bin: Option<String>,
        #[clap(long, help = "Config file path")]
        config: Option<String>,
        #[clap(long, help = "Data directory")]
        data_dir: Option<String>,
        #[clap(long, help = "HTTP port")]
        http_port: Option<u16>,
        #[clap(long, help = "gRPC port")]
        grpc_port: Option<u16>,
        #[clap(long, help = "Run in foreground")]
        foreground: bool,
    },
    /// Stop the managed linkrs-server process
    Stop,
    /// Restart the managed linkrs-server process
    Restart,
    /// Show managed process status
    Status,
}
