#[cfg(feature = "server")]
mod server_main {
    use clap::Parser;
    use linkrs::config::logging;
    use linkrs::config::Config;
    use linkrs_server::{execute_query, start_service_with_config_path};

    #[derive(Parser)]
    #[clap(version = "0.1.0", author = "Linkrs Contributors")]
    enum Cli {
        /// Start the Linkrs service
        Serve {
            #[clap(short, long)]
            config: Option<String>,
            #[clap(long)]
            data_dir: Option<String>,
            #[clap(long)]
            http_port: Option<u16>,
            #[clap(long)]
            grpc_port: Option<u16>,
            #[clap(long)]
            log_level: Option<String>,
        },
        /// Execute a query directly
        Query {
            #[clap(short, long)]
            query: String,
        },
    }

    pub fn main() {
        let cli = Cli::parse();

        let result = match cli {
            Cli::Serve {
                config,
                data_dir,
                http_port,
                grpc_port,
                log_level,
            } => {
                println!("Starting Linkrs service");
                println!("Process ID: {}", std::process::id());

                // Load configuration, retaining the file path for
                // config-write endpoints only when the file exists.
                let (cfg, config_path) = match config {
                    Some(config_path) => {
                        let path = std::path::PathBuf::from(&config_path);
                        match Config::load(&config_path) {
                            Ok(cfg) => (cfg, path.exists().then_some(path)),
                            Err(e) => {
                                eprintln!(
                                    "Failed to load configuration file: {}, using default configuration",
                                    e
                                );
                                (Config::default(), None)
                            }
                        }
                    }
                    None => match Config::load_user_config() {
                        Ok(cfg) => {
                            let path = Config::user_config_path().ok().filter(|p| p.exists());
                            (cfg, path)
                        }
                        Err(e) => {
                            eprintln!(
                                "Failed to load user configuration file, using default configuration: {}",
                                e
                            );
                            (Config::default(), None)
                        }
                    },
                };

                let mut cfg = cfg;
                if let Some(dir) = data_dir {
                    cfg.common.database.storage_path = dir;
                }
                if let Some(port) = http_port {
                    cfg.server.http.port = port;
                }
                if let Some(port) = grpc_port {
                    cfg.server.grpc.port = port;
                }
                if let Some(level) = log_level {
                    cfg.common.log.level = level;
                }
                if let Err(error) = cfg.validate() {
                    eprintln!("Invalid effective configuration: {}", error);
                    std::process::exit(1);
                }

                // Initialize logging system
                if let Err(e) = logging::init(&cfg) {
                    eprintln!("Failed to initialize logging system: {}", e);
                }

                // Create Tokio runtime and start service
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("Failed to create tokio runtime: {}", e);
                        return;
                    }
                };
                let result =
                    rt.block_on(async { start_service_with_config_path(cfg, config_path).await });

                // Ensure logging is flushed before exiting
                logging::shutdown();
                result
            }
            Cli::Query { query } => {
                println!("Executing query: {}", query);
                println!("Process ID: {}", std::process::id());

                // Use default configuration to initialize logging
                let cfg = Config::default();
                if let Err(e) = logging::init(&cfg) {
                    eprintln!("Failed to initialize logging system: {}", e);
                }

                // Execute query directly using tokio runtime
                let rt = match tokio::runtime::Runtime::new() {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("Failed to create tokio runtime: {}", e);
                        return;
                    }
                };
                let result = rt.block_on(execute_query(&query));

                // Ensure logging is flushed before exiting
                logging::shutdown();
                result
            }
        };

        if let Err(e) = result {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}

#[cfg(feature = "server")]
fn main() {
    server_main::main()
}

#[cfg(not(feature = "server"))]
fn main() {
    eprintln!("Error: server feature is not enabled, cannot run server program");
    eprintln!("Please recompile using the following command:");
    eprintln!("  cargo run --features server");
    std::process::exit(1);
}
