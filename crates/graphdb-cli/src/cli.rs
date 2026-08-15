use clap::Parser;

#[derive(Parser, Debug)]
#[clap(
    name = "graphdb-cli",
    version = env!("CARGO_PKG_VERSION"),
    about = "GraphDB CLI - Interactive command-line client for GraphDB",
    long_about = "GraphDB CLI is an interactive command-line client for GraphDB,\
                  similar to PostgreSQL's psql. It supports GQL query execution,\
                  schema inspection, and various output formats."
)]
pub struct Cli {
    #[clap(short, long, default_value = "127.0.0.1", help = "Server host")]
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
}
