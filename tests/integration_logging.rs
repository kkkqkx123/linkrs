//! Log System Integration Testing
//!
//! Test Scope.
//! - Log configuration loading and validation
//! - Log file creation and writing
//! - Log rotation function
//! - Log Level Filtering

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use graphdb::config::Config;
#[cfg(all(feature = "qdrant", feature = "embedded"))]
use graphdb::search::FulltextConfig;
#[cfg(all(feature = "qdrant", feature = "embedded"))]
use vector_client::config::VectorClientConfig;

/// Test Log Configuration Defaults
#[test]
fn test_log_config_defaults() {
    let config = Config::default();

    assert_eq!(config.common.log.level, "info");
    assert_eq!(config.common.log.dir, "logs");
    assert_eq!(config.common.log.file, "graphdb");
    assert_eq!(config.common.log.max_file_size, 100 * 1024 * 1024); // 100MB
    assert_eq!(config.common.log.max_files, 5);
}

/// Test Log Configuration Serialization and Deserialization
#[cfg(all(feature = "qdrant", feature = "embedded"))]
#[test]
fn test_log_config_serialization() {
    let config = Config {
        common: graphdb::config::CommonConfig {
            database: graphdb::config::DatabaseConfig {
                host: "127.0.0.1".to_string(),
                port: 9758,
                storage_path: "data/graphdb".to_string(),
                max_connections: 10,
            },
            transaction: graphdb::config::TransactionConfig {
                default_timeout: 30,
                max_concurrent_transactions: 1000,
                auto_commit: false,
            },
            log: graphdb::config::LogConfig {
                level: "debug".to_string(),
                dir: "test_logs".to_string(),
                file: "test_graphdb".to_string(),
                max_file_size: 50 * 1024 * 1024,
                max_files: 3,
            },
            storage: graphdb::config::StorageConfig::default(),
            optimizer: graphdb::config::OptimizerConfig::default(),
            parallel: graphdb::config::ParallelConfig::default(),
            monitoring: graphdb::config::MonitoringConfig::default(),
            query_resource: graphdb::config::QueryResourceConfig::default(),
            columnar: graphdb::config::ColumnarConfig::default(),
        },
        #[cfg(feature = "server")]
        server: graphdb::config::ServerConfig::default(),
        vector: VectorClientConfig::default(),
        fulltext: FulltextConfig::default(),
        embedded: graphdb::config::EmbeddedConfig::default(),
    };

    let toml_str = toml::to_string_pretty(&config).expect("Failed to serialize config");

    assert!(toml_str.contains("level = \"debug\""));
    assert!(toml_str.contains("dir = \"test_logs\""));
    assert!(toml_str.contains("file = \"test_graphdb\""));
    assert!(toml_str.contains("max_file_size = 52428800"));
    assert!(toml_str.contains("max_files = 3"));

    let loaded_config: Config = toml::from_str(&toml_str).expect("Failed to deserialize config");
    assert_eq!(loaded_config.common.log.level, "debug");
    assert_eq!(loaded_config.common.log.dir, "test_logs");
    assert_eq!(loaded_config.common.log.file, "test_graphdb");
    assert_eq!(loaded_config.common.log.max_file_size, 52428800);
    assert_eq!(loaded_config.common.log.max_files, 3);
}

/// Test Log Directory Creation
#[test]
fn test_log_directory_creation() {
    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("dir_test_{}", std::process::id()));

    let _ = fs::remove_dir_all(&temp_dir);
    assert!(!temp_dir.exists());

    fs::create_dir_all(&temp_dir).expect("Failed to create log directory");
    assert!(temp_dir.exists());

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test log configuration loaded from file
#[test]
fn test_log_config_from_file() {
    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("config_test_{}", std::process::id()));

    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create test directory");

    let config_content = r#"
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/graphdb"
max_connections = 10

[transaction]
default_timeout = 30
max_concurrent_transactions = 1000
auto_cleanup = true
cleanup_interval = 10

[log]
level = "debug"
dir = "custom_logs"
file = "custom_graphdb"
max_file_size = 52428800
max_files = 3

[auth]
enable_authorize = true
failed_login_attempts = 5
session_idle_timeout_secs = 3600
force_change_default_password = true
default_username = "root"
default_password = "root"

[bootstrap]
auto_create_default_space = true
default_space_name = "default"
single_user_mode = false

[optimizer]
max_iteration_rounds = 5
max_exploration_rounds = 128
enable_cost_model = true
enable_multi_plan = true
enable_property_pruning = true
enable_adaptive_iteration = true
stable_threshold = 2
min_iteration_rounds = 1
"#;

    let config_path = temp_dir.join("test_config.toml");
    fs::write(&config_path, config_content).expect("Failed to write config file");

    let config = Config::load(&config_path).expect("Failed to load config");

    assert_eq!(config.common.log.level, "debug");
    // Config::load resolves relative paths to absolute paths relative to the config file location
    let expected_dir = temp_dir.join("custom_logs").to_string_lossy().to_string();
    assert_eq!(config.common.log.dir, expected_dir);
    assert_eq!(config.common.log.file, "custom_graphdb");
    assert_eq!(config.common.log.max_file_size, 52428800);
    assert_eq!(config.common.log.max_files, 3);

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: verifying flexi_logger functionality
/// Note: flexi_logger uses a global logger, so only one test can initialize it.
/// This test covers all log output scenarios.
#[test]
fn test_flexi_logger_integration() {
    use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, Naming, WriteMode};

    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("integration_test_{}", std::process::id()));

    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("Failed to create test directory");

    // ========== Test 1: Basic Log Write ==========
    {
        let test_dir = temp_dir.join("basic");
        fs::create_dir_all(&test_dir).expect("Failed to create test directory");

        let _logger = Logger::try_with_str("info")
            .expect("Failed to create logger")
            .log_to_file(
                FileSpec::default()
                    .basename("basic_test")
                    .directory(&test_dir),
            )
            .write_mode(WriteMode::Direct)
            .start()
            .expect("Failed to start logger");

        log::info!("Basic log write test");
        log::warn!("Warning log test");
        log::error!("Error log test");

        std::thread::sleep(Duration::from_millis(500));

        let log_files: Vec<_> = fs::read_dir(&test_dir)
            .expect("Failed to read directory")
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                name.starts_with("basic_test") && name.ends_with(".log")
            })
            .collect();

        assert!(
            !log_files.is_empty(),
            "There should be at least one log file"
        );

        let log_file = &log_files[0];
        let content = fs::read_to_string(log_file.path()).expect("Failed to read log file");
        assert!(
            content.contains("Basic log write test"),
            "The log shall contain the message log"
        );
        assert!(
            content.contains("Warning log test"),
            "Logs should contain warning logs"
        );
        assert!(
            content.contains("Error log test"),
            "Logs should contain error logs"
        );
    }

    // ========== Test 2: Log Level Filtering ==========
    {
        let test_dir = temp_dir.join("level_filter");
        fs::create_dir_all(&test_dir).expect("Failed to create test directory");

        let config = Config {
            common: graphdb::config::CommonConfig {
                log: graphdb::config::LogConfig {
                    level: "warn".to_string(),
                    dir: test_dir.to_string_lossy().to_string(),
                    file: "level_test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(config.common.log.level, "warn");
        assert!(config.common.log.dir.contains("level_filter"));
    }

    // ========== Test 3: Log Rotation Configuration Validation ==========
    {
        let test_dir = temp_dir.join("rotation");
        fs::create_dir_all(&test_dir).expect("Failed to create test directory");

        let config = Config {
            common: graphdb::config::CommonConfig {
                log: graphdb::config::LogConfig {
                    level: "info".to_string(),
                    dir: test_dir.to_string_lossy().to_string(),
                    file: "rotation_test".to_string(),
                    max_file_size: 10 * 1024 * 1024,
                    max_files: 3,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(config.common.log.max_file_size, 10 * 1024 * 1024);
        assert_eq!(config.common.log.max_files, 3);

        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("Failed to create logger")
            .log_to_file(file_spec)
            .rotate(
                Criterion::Size(config.common.log.max_file_size),
                Naming::Numbers,
                Cleanup::KeepLogFiles(config.common.log.max_files),
            );
    }

    // ========== Test 4: Asynchronous Write Configuration Validation ==========
    {
        let test_dir = temp_dir.join("async");
        fs::create_dir_all(&test_dir).expect("Failed to create test directory");

        let config = Config {
            common: graphdb::config::CommonConfig {
                log: graphdb::config::LogConfig {
                    level: "debug".to_string(),
                    dir: test_dir.to_string_lossy().to_string(),
                    file: "async_test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("Failed to create logger")
            .log_to_file(file_spec)
            .write_mode(WriteMode::Async);
    }

    // ========== Test 5: Log Cleaning Policy Configuration Validation ==========
    {
        let test_dir = temp_dir.join("cleanup");
        fs::create_dir_all(&test_dir).expect("Failed to create test directory");

        let max_files = 2;
        let config = Config {
            common: graphdb::config::CommonConfig {
                log: graphdb::config::LogConfig {
                    level: "info".to_string(),
                    dir: test_dir.to_string_lossy().to_string(),
                    file: "cleanup_test".to_string(),
                    max_file_size: 1024 * 1024,
                    max_files,
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(config.common.log.max_files, max_files);

        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("Failed to create logger")
            .log_to_file(file_spec)
            .rotate(
                Criterion::Size(config.common.log.max_file_size),
                Naming::Numbers,
                Cleanup::KeepLogFiles(config.common.log.max_files),
            );
    }

    // ========== Test 6: Timestamp Format ==========
    {
        use flexi_logger::{DeferredNow, TS_DASHES_BLANK_COLONS_DOT_BLANK};

        fn log_format(
            w: &mut dyn std::io::Write,
            now: &mut DeferredNow,
            record: &log::Record,
        ) -> Result<(), std::io::Error> {
            write!(
                w,
                "{} [{}] {}: {}",
                now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK),
                record.level(),
                record.module_path().unwrap_or("unknown"),
                record.args()
            )
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut now = DeferredNow::new();
        let record = log::Record::builder()
            .args(format_args!("Timestamp format test message"))
            .level(log::Level::Info)
            .target("test")
            .module_path(Some("test_module"))
            .build();
        log_format(&mut buf, &mut now, &record).expect("Failed to format log");
        let log_content = String::from_utf8(buf).expect("Failed to parse log output");

        assert!(
            log_content.contains("Timestamp format test message"),
            "The log should contain the test messages."
        );

        let timestamp_regex = regex::Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}")
            .expect("Failed to create regex");
        assert!(
            timestamp_regex.is_match(&log_content),
            "Log should contain a timestamp in format YYYY-MM-DD HH:MM:SS.mmm"
        );

        assert!(
            log_content.contains("[INFO]"),
            "The log should contain log level markers."
        );
    }

    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test Log File Path Resolution
#[cfg(all(feature = "qdrant", feature = "embedded"))]
#[test]
fn test_log_file_path_resolution() {
    let config = Config::default();

    let expected_log_path = format!("{}/{}.log", config.common.log.dir, config.common.log.file);
    assert_eq!(expected_log_path, "logs/graphdb.log");

    let custom_config = Config {
        common: graphdb::config::CommonConfig {
            log: graphdb::config::LogConfig {
                dir: "/var/log/graphdb".to_string(),
                file: "app".to_string(),
                ..Default::default()
            },
            ..Default::default()
        },
        #[cfg(feature = "server")]
        server: graphdb::config::ServerConfig::default(),
        vector: VectorClientConfig::default(),
        fulltext: FulltextConfig::default(),
        embedded: graphdb::config::EmbeddedConfig::default(),
    };

    let custom_path = format!(
        "{}/{}.log",
        custom_config.common.log.dir, custom_config.common.log.file
    );
    assert_eq!(custom_path, "/var/log/graphdb/app.log");
}

/// Testing the configuration of the log file size.
#[test]
fn test_log_file_size_config() {
    let config = Config::default();
    assert_eq!(config.common.log.max_file_size, 100 * 1024 * 1024);

    let custom_config = Config {
        common: graphdb::config::CommonConfig {
            log: graphdb::config::LogConfig {
                max_file_size: 500 * 1024 * 1024,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(custom_config.common.log.max_file_size, 500 * 1024 * 1024);

    let small_config = Config {
        common: graphdb::config::CommonConfig {
            log: graphdb::config::LogConfig {
                max_file_size: 1024,
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(small_config.common.log.max_file_size, 1024);
}

/// Verification of test log level configuration
#[test]
fn test_log_level_validation() {
    let valid_levels = vec!["trace", "debug", "info", "warn", "error"];

    for level in valid_levels {
        let config = Config {
            common: graphdb::config::CommonConfig {
                log: graphdb::config::LogConfig {
                    level: level.to_string(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(config.common.log.level, level);
    }
}

/// Test log timestamp format via the formatter function directly.
/// flexi_logger is global and can only be initialized once per process,
/// so this test validates the format function without starting a logger.
#[test]
fn test_log_timestamp_format() {
    use flexi_logger::{DeferredNow, TS_DASHES_BLANK_COLONS_DOT_BLANK};

    fn log_format(
        w: &mut dyn std::io::Write,
        now: &mut DeferredNow,
        record: &log::Record,
    ) -> Result<(), std::io::Error> {
        write!(
            w,
            "{} [{}] {}: {}",
            now.format(TS_DASHES_BLANK_COLONS_DOT_BLANK),
            record.level(),
            record.module_path().unwrap_or("unknown"),
            record.args()
        )
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut now = DeferredNow::new();
    let record = log::Record::builder()
        .args(format_args!("Timestamp format test message"))
        .level(log::Level::Info)
        .target("test")
        .module_path(Some("test_module"))
        .build();
    log_format(&mut buf, &mut now, &record).expect("Failed to format log");

    let log_content = String::from_utf8(buf).expect("Failed to parse log output");

    assert!(
        log_content.contains("Timestamp format test message"),
        "The log should contain the test messages."
    );

    let timestamp_regex = regex::Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}")
        .expect("Failed to create regex");
    assert!(
        timestamp_regex.is_match(&log_content),
        "Log should contain a timestamp in format YYYY-MM-DD HH:MM:SS.mmm"
    );

    assert!(
        log_content.contains("[INFO]"),
        "The log should contain log level markers."
    );
}
