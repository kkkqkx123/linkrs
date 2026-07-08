use graphdb::config::Config;
use std::env;
use std::fs;
use tempfile::TempDir;

#[test]
fn config_load_uses_config_file_directory_for_relative_paths() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_dir = temp_dir.path().join("config");
    fs::create_dir_all(&config_dir).expect("Failed to create config directory");

    let config_path = config_dir.join("graphdb.toml");
    fs::write(
        &config_path,
        r#"
[database]
storage_path = "data/graphdb"
"#,
    )
    .expect("Failed to write config file");

    let config = Config::load(&config_path).expect("Failed to load config");
    assert_eq!(
        config.common.database.storage_path,
        config_dir.join("data/graphdb").to_string_lossy()
    );
}

#[test]
fn user_config_loading_uses_graphdb_config_dir() {
    let temp_dir = TempDir::new().expect("Failed to create temporary directory");
    let config_dir = temp_dir.path().join("graphdb-user-config");
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
