//! Log System Integration Testing
//!
//! Test Scope.
//! - Log configuration loading and validation
//! - Log file creation and writing
//! - Log rotation function
//! - Log Level Filtering

mod common;

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
            monitoring: graphdb::config::MonitoringConfig::default(),
            query_resource: graphdb::config::QueryResourceConfig::default(),
        },
        #[cfg(feature = "server")]
        server: graphdb::config::ServerConfig::default(),
        vector: VectorClientConfig::default(),
        fulltext: FulltextConfig::default(),
        embedded: graphdb::config::EmbeddedConfig::default(),
    };

    // Serialization to TOML
    let toml_str = toml::to_string_pretty(&config).expect("序列化配置失败");

    // Verifying TOML Include Logging Configuration
    assert!(toml_str.contains("level = \"debug\""));
    assert!(toml_str.contains("dir = \"test_logs\""));
    assert!(toml_str.contains("file = \"test_graphdb\""));
    assert!(toml_str.contains("max_file_size = 52428800"));
    assert!(toml_str.contains("max_files = 3"));

    // deserialization
    let loaded_config: Config = toml::from_str(&toml_str).expect("反序列化配置失败");
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

    // Make sure the catalog does not exist
    let _ = fs::remove_dir_all(&temp_dir);
    assert!(!temp_dir.exists());

    // Create a catalog
    fs::create_dir_all(&temp_dir).expect("创建日志目录失败");
    assert!(temp_dir.exists());

    // clear up
    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test log configuration loaded from file
#[test]
fn test_log_config_from_file() {
    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("config_test_{}", std::process::id()));

    // Clean up and create a catalog
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("创建测试目录失败");

    // Creating Test Profiles
    let config_content = r#"
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/graphdb"
max_connections = 10

[transaction]
default_timeout = 30
max_concurrent_transactions = 1000
enable_2pc = false
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
    fs::write(&config_path, config_content).expect("写入配置文件失败");

    // Load Configuration
    let config = Config::load(&config_path).expect("加载配置失败");

    // Verify Logging Configuration
    assert_eq!(config.common.log.level, "debug");
    assert_eq!(config.common.log.dir, "custom_logs");
    assert_eq!(config.common.log.file, "custom_graphdb");
    assert_eq!(config.common.log.max_file_size, 52428800);
    assert_eq!(config.common.log.max_files, 3);

    // clear up
    let _ = fs::remove_dir_all(&temp_dir);
}

/// Integration test: verifying flexi_logger functionality
/// Note: Since flexi_logger uses a global logger, all functionality is verified in a single test
#[test]
fn test_flexi_logger_integration() {
    use flexi_logger::{Cleanup, Criterion, FileSpec, Logger, Naming, WriteMode};

    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("integration_test_{}", std::process::id()));

    // Clean up and create a test directory
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("创建测试目录失败");

    // ========== Test 1: Basic Log Write ==========
    {
        let test_dir = temp_dir.join("basic");
        fs::create_dir_all(&test_dir).expect("创建测试目录失败");

        let _logger = Logger::try_with_str("info")
            .expect("创建 logger 失败")
            .log_to_file(
                FileSpec::default()
                    .basename("basic_test")
                    .directory(&test_dir),
            )
            .write_mode(WriteMode::Direct)
            .start()
            .expect("启动 logger 失败");

        log::info!("基本日志写入测试");
        log::warn!("警告日志测试");
        log::error!("错误日志测试");

        // Waiting for logs to be written
        std::thread::sleep(Duration::from_millis(500));

        // Find generated log files (flexi_logger may use rCURRENT suffix)
        let log_files: Vec<_> = fs::read_dir(&test_dir)
            .expect("读取目录失败")
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

        // Read the first log file
        let log_file = &log_files[0];
        let content = fs::read_to_string(log_file.path()).expect("读取日志文件失败");
        assert!(
            content.contains("基本日志写入测试"),
            "The log shall contain the message log"
        );
        assert!(
            content.contains("警告日志测试"),
            "Logs should contain warning logs"
        );
        assert!(
            content.contains("错误日志测试"),
            "Logs should contain error logs"
        );
    }

    // ========== Test 2: Log Level Filtering ==========
    {
        let test_dir = temp_dir.join("level_filter");
        fs::create_dir_all(&test_dir).expect("创建测试目录失败");

        // Note: Since the global logger has already been set up, this is tested in a different way here
        // Actually flexi_logger does not support reinitialization in the same process
        // So let's just verify that the configuration can be loaded correctly

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

        // Verify that the configuration is correct
        assert_eq!(config.common.log.level, "warn");
        assert!(config.common.log.dir.contains("level_filter"));
    }

    // ========== Test 3: Log Rotation Configuration Validation ==========
    {
        let test_dir = temp_dir.join("rotation");
        fs::create_dir_all(&test_dir).expect("创建测试目录失败");

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

        // Verifying Rotation Configuration
        assert_eq!(config.common.log.max_file_size, 10 * 1024 * 1024);
        assert_eq!(config.common.log.max_files, 3);

        // Verify that the rotation configuration of flexi_logger can be built correctly
        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("创建 logger 失败")
            .log_to_file(file_spec)
            .rotate(
                Criterion::Size(config.common.log.max_file_size),
                Naming::Numbers,
                Cleanup::KeepLogFiles(config.common.log.max_files),
            );
        // Note: You don't actually start the logger, because the global logger already exists.
    }

    // ========== Test 4: Asynchronous Write Configuration Validation ==========
    {
        let test_dir = temp_dir.join("async");
        fs::create_dir_all(&test_dir).expect("创建测试目录失败");

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

        // Verify that the asynchronous configuration can be built correctly
        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("创建 logger 失败")
            .log_to_file(file_spec)
            .write_mode(WriteMode::Async);
        // Note: You don't actually start the logger, because the global logger already exists.
    }

    // ========== Test 5: Log Cleaning Policy Configuration Validation ==========
    {
        let test_dir = temp_dir.join("cleanup");
        fs::create_dir_all(&test_dir).expect("创建测试目录失败");

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

        // Verify Cleanup Configuration
        assert_eq!(config.common.log.max_files, max_files);

        // Verify that the flexi_logger cleanup configuration can be built correctly
        let file_spec = FileSpec::default()
            .basename(&config.common.log.file)
            .directory(&config.common.log.dir);

        let _logger_builder = Logger::try_with_str(&config.common.log.level)
            .expect("创建 logger 失败")
            .log_to_file(file_spec)
            .rotate(
                Criterion::Size(config.common.log.max_file_size),
                Naming::Numbers,
                Cleanup::KeepLogFiles(config.common.log.max_files),
            );
        // Note: You don't actually start the logger, because the global logger already exists.
    }

    // Clean up all test directories
    let _ = fs::remove_dir_all(&temp_dir);
}

/// Test Log File Path Resolution
#[cfg(all(feature = "qdrant", feature = "embedded"))]
#[test]
fn test_log_file_path_resolution() {
    let config = Config::default();

    // Verify the combination of the log directory and file name.
    let expected_log_path = format!("{}/{}.log", config.common.log.dir, config.common.log.file);
    assert_eq!(expected_log_path, "logs/graphdb.log");

    // Testing the custom configuration
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
    // The default value for the test is 100MB.
    let config = Config::default();
    assert_eq!(config.common.log.max_file_size, 100 * 1024 * 1024);

    // Testing custom sizes
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

    // Testing the configuration of small files (for testing purposes)
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

/// Test log timestamp format
/// Verify that the log output contains timestamps, and that the timestamp format is correct.
#[test]
fn test_log_timestamp_format() {
    use flexi_logger::{
        DeferredNow, FileSpec, Logger, WriteMode, TS_DASHES_BLANK_COLONS_DOT_BLANK,
    };

    let temp_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("test-logs")
        .join(format!("timestamp_test_{}", std::process::id()));

    // Clean up and create a test directory.
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("创建测试目录失败");

    let test_dir = temp_dir.join("timestamp");
    fs::create_dir_all(&test_dir).expect("创建测试目录失败");

    // Custom log formatting functions that are consistent with the actual log system.
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
            &record.args()
        )
    }

    // Initialize the logger using a custom format (including a timestamp).
    let _logger = Logger::try_with_str("info")
        .expect("创建 logger 失败")
        .log_to_file(
            FileSpec::default()
                .basename("timestamp_test")
                .directory(&test_dir),
        )
        .format_for_files(log_format)
        .write_mode(WriteMode::Direct)
        .start()
        .expect("启动 logger 失败");

    // Write to the test log
    log::info!("时间戳格式测试日志");
    log::warn!("警告日志时间戳测试");
    log::error!("错误日志时间戳测试");

    // Waiting for the log to be written.
    std::thread::sleep(Duration::from_millis(500));

    // Find the generated log file.
    let log_files: Vec<_> = fs::read_dir(&test_dir)
        .expect("读取目录失败")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("timestamp_test") && name.ends_with(".log")
        })
        .collect();

    assert!(
        !log_files.is_empty(),
        "There should be at least one log file."
    );

    // Read the first log file.
    let log_file = &log_files[0];
    let content = fs::read_to_string(log_file.path()).expect("读取日志文件失败");

    // Verify the content of the log files.
    assert!(
        content.contains("时间戳格式测试日志"),
        "The log should contain the test messages."
    );

    // 验证时间戳格式：YYYY-MM-DD HH:MM:SS.mmm
    let timestamp_regex = regex::Regex::new(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}\.\d{3}")
        .expect("创建正则表达式失败");
    assert!(
        timestamp_regex.is_match(&content),
        "日志应包含时间戳，格式为 YYYY-MM-DD HH:MM:SS.mmm"
    );

    // Verify the log level labels.
    assert!(
        content.contains("[INFO]") || content.contains("[WARN]") || content.contains("[ERROR]"),
        "The log should contain log level markers."
    );

    // Clean up
    let _ = fs::remove_dir_all(&temp_dir);
}
