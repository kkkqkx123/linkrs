use graphdb_config::config::Config;
use std::env;
use std::fs;
use tempfile::TempDir;

#[test]
fn load_config_resolves_relative_paths_from_file_directory() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).expect("Failed to create config directory");

    let config_path = config_dir.join("graphdb.toml");
    fs::write(
        &config_path,
        r#"
[database]
storage_path = "data/graphdb"

[log]
dir = "logs"

[monitoring.slow_query_log]
log_file_path = "logs/slow_query.log"

[fulltext]
index_path = "fulltext"
"#,
    )
    .expect("Failed to write config file");

    let config = Config::load(&config_path).expect("Failed to load config");

    assert_eq!(
        config.common.database.storage_path,
        config_dir.join("data/graphdb").to_string_lossy()
    );
    assert_eq!(
        config.common.log.dir,
        config_dir.join("logs").to_string_lossy()
    );
    assert_eq!(
        config.common.monitoring.slow_query_log.log_file_path,
        config_dir.join("logs/slow_query.log").to_string_lossy()
    );
    assert_eq!(config.fulltext.index_path, config_dir.join("fulltext"));
}

#[test]
fn load_user_config_prefers_graphdb_config_dir() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_dir = temp_dir.path().join("user-config");
    fs::create_dir_all(&config_dir).expect("Failed to create config directory");

    fs::write(
        config_dir.join("config.toml"),
        r#"
[database]
storage_path = "storage"
"#,
    )
    .expect("Failed to write config file");

    let previous = env::var("GRAPHDB_CONFIG_DIR").ok();
    env::set_var("GRAPHDB_CONFIG_DIR", &config_dir);

    let config = Config::load_user_config().expect("Failed to load user config");
    assert_eq!(
        config.common.database.storage_path,
        config_dir.join("storage").to_string_lossy()
    );

    match previous {
        Some(value) => env::set_var("GRAPHDB_CONFIG_DIR", value),
        None => env::remove_var("GRAPHDB_CONFIG_DIR"),
    }
}
