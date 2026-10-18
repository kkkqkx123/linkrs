pub mod columnar;
pub mod cost_profile;
pub mod database;
pub mod fulltext;
pub mod log;
pub mod logging;
pub mod migration;
pub mod monitoring;
pub mod optimizer;
pub mod parallel;
pub mod runtime;
pub mod storage;
pub mod transaction;
pub mod vector;

#[cfg(feature = "server")]
pub mod auth;
#[cfg(feature = "server")]
pub mod bootstrap;
#[cfg(feature = "server")]
pub mod grpc;
#[cfg(feature = "server")]
pub mod http;
#[cfg(feature = "server")]
pub mod security;

pub use columnar::*;
pub use cost_profile::*;
pub use database::*;
pub use fulltext::*;
pub use log::*;
pub use logging::*;
pub use migration::*;
pub use monitoring::*;
pub use optimizer::*;
pub use parallel::*;
pub use runtime::*;
pub use storage::*;
pub use transaction::*;
pub use vector::*;

#[cfg(feature = "server")]
pub use auth::*;
#[cfg(feature = "server")]
pub use bootstrap::*;
#[cfg(feature = "server")]
pub use grpc::*;
#[cfg(feature = "server")]
pub use http::*;
#[cfg(feature = "server")]
pub use security::*;

use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(feature = "rerank")]
use llm_rerank::{RerankConfig, RerankFusionStrategy};

/// Common configuration aggregator
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct CommonConfig {
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub transaction: TransactionConfig,
    #[serde(default)]
    pub log: LogConfig,
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub optimizer: OptimizerConfig,
    #[serde(default)]
    pub parallel: ParallelConfig,
    #[serde(default)]
    pub monitoring: MonitoringConfig,
    #[serde(default)]
    pub migration: MigrationConfig,
    #[serde(default)]
    pub query_resource: QueryResourceConfig,
    #[serde(default)]
    pub columnar: ColumnarConfig,
}

impl CommonConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.database.validate()?;
        self.transaction.validate()?;
        self.log.validate()?;
        self.storage.validate()?;
        self.optimizer.validate()?;
        self.parallel.validate()?;
        self.monitoring.validate()?;
        self.migration.validate()?;
        self.query_resource.validate()?;
        Ok(())
    }
}

/// Embedded configuration aggregator
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct EmbeddedConfig {
    #[serde(default)]
    pub runtime: RuntimeConfig,
}

impl EmbeddedConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.runtime.validate()?;
        Ok(())
    }
}

/// Server configuration aggregator
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct ServerConfig {
    #[cfg(feature = "server")]
    #[serde(default)]
    pub grpc: GrpcConfig,
    #[cfg(feature = "server")]
    #[serde(default)]
    pub http: HttpServerConfig,
    #[cfg(feature = "server")]
    #[serde(default)]
    pub auth: AuthConfig,
    #[cfg(feature = "server")]
    #[serde(default)]
    pub bootstrap: BootstrapConfig,
    #[cfg(feature = "server")]
    #[serde(default)]
    pub security: SecurityConfig,
}

#[cfg(feature = "server")]
impl ServerConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.grpc.validate()?;
        self.http.validate()?;
        self.auth.validate()?;
        self.security.validate()?;
        self.validate_listeners()?;
        Ok(())
    }

    /// Cross-section rules that only make sense once every listener is known.
    fn validate_listeners(&self) -> Result<(), String> {
        if !self.grpc.enabled && !self.http.enabled {
            return Err("at least one of grpc.enabled and http.enabled must be true".to_string());
        }
        if self.grpc.enabled && self.http.enabled && self.http.port == self.grpc.port {
            return Err(format!(
                "http.port and grpc.port must differ, both are {}",
                self.http.port
            ));
        }

        let http_loopback = is_loopback_address(&self.http.bind_address);
        if !http_loopback && self.http.cors_enabled && self.http.cors_allowed_origins.is_empty() {
            return Err(
                "http.cors_allowed_origins must list exact origins when http.bind_address is not loopback"
                    .to_string(),
            );
        }

        if !http_loopback && self.bootstrap.single_user_mode {
            return Err(
                "bootstrap.single_user_mode requires http.bind_address to be loopback".to_string(),
            );
        }

        Ok(())
    }
}

/// Global configuration aggregator
#[derive(Debug, Deserialize, Serialize, Clone, Default)]
pub struct Config {
    #[serde(flatten)]
    pub common: CommonConfig,

    #[cfg(feature = "server")]
    #[serde(flatten)]
    pub server: ServerConfig,

    #[cfg(feature = "embedded")]
    #[serde(default)]
    pub embedded: EmbeddedConfig,

    #[cfg(feature = "vector")]
    #[serde(default)]
    pub vector: VectorConfig,

    #[serde(default)]
    pub fulltext: FulltextConfig,
}

impl Config {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, Box<dyn std::error::Error>> {
        let path = path.as_ref();
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        let content = fs::read_to_string(path)?;
        let default_value: toml::Value = toml::from_str(&toml::to_string(&Config::default())?)?;
        let file_value: toml::Value = toml::from_str(&content)?;
        Self::reject_unknown_sections(&default_value, &file_value)?;
        let merged_value = Self::merge_toml_values(default_value, file_value);
        let mut config: Config = toml::from_str(&toml::to_string(&merged_value)?)?;
        config.resolve_relative_paths(base_dir)?;
        config.apply_env_overrides()?;
        Ok(config)
    }

    /// Reject top-level keys that are not configuration sections.
    ///
    /// `Config` flattens its sub-configurations, so serde itself cannot flag
    /// unknown top-level keys. The serialized default names every valid
    /// section, which keeps this list free of duplicated maintenance.
    fn reject_unknown_sections(
        default_value: &toml::Value,
        file_value: &toml::Value,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let known = default_value
            .as_table()
            .map(|table| table.keys().cloned().collect::<Vec<String>>())
            .ok_or("Serialized default configuration is not a table")?;
        let file_table = file_value
            .as_table()
            .ok_or("configuration file must contain a TOML table at the top level")?;
        for key in file_table.keys() {
            if !known.contains(key) {
                return Err(format!(
                    "unknown configuration section '[{}]': expected one of {}",
                    key,
                    known.join(", ")
                )
                .into());
            }
        }
        Ok(())
    }

    pub fn load_user_config() -> Result<Self, Box<dyn std::error::Error>> {
        Self::load_user_config_named("config.toml")
    }

    pub fn load_user_config_named(file_name: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let config_dir = Self::user_config_dir()?;
        Self::load(config_dir.join(file_name))
    }

    /// Canonical user configuration file path (`<user_config_dir>/config.toml`).
    pub fn user_config_path() -> Result<PathBuf, Box<dyn std::error::Error>> {
        Ok(Self::user_config_dir()?.join("config.toml"))
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<(), Box<dyn std::error::Error>> {
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }

    fn user_config_dir() -> Result<PathBuf, Box<dyn std::error::Error>> {
        if let Ok(dir) = env::var("LINKRS_CONFIG_DIR") {
            return Ok(PathBuf::from(dir));
        }
        if let Some(dir) = dirs::config_dir() {
            return Ok(dir.join("linkrs"));
        }
        Err("Failed to determine user configuration directory".into())
    }

    fn apply_env_overrides(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if let Ok(value) = env::var("LINKRS_STORAGE_PATH") {
            if !value.is_empty() {
                self.common.database.storage_path = value;
            }
        }
        if let Ok(value) = env::var("LINKRS_LOG_DIR") {
            if !value.is_empty() {
                self.common.log.dir = value;
            }
        }
        if let Ok(value) = env::var("LINKRS_LOG_LEVEL") {
            if !value.is_empty() {
                self.common.log.level = value;
            }
        }
        #[cfg(feature = "server")]
        {
            if let Ok(value) = env::var("LINKRS_HTTP_PORT") {
                if !value.is_empty() {
                    self.server.http.port = value
                        .parse()
                        .map_err(|_| "LINKRS_HTTP_PORT must be a valid port number")?;
                }
            }
            if let Ok(value) = env::var("LINKRS_GRPC_PORT") {
                if !value.is_empty() {
                    self.server.grpc.port = value
                        .parse()
                        .map_err(|_| "LINKRS_GRPC_PORT must be a valid port number")?;
                }
            }
            if let Ok(value) = env::var("LINKRS_HTTP_BIND") {
                if !value.is_empty() {
                    self.server.http.bind_address = value;
                }
            }
            if let Ok(value) = env::var("LINKRS_GRPC_BIND") {
                if !value.is_empty() {
                    self.server.grpc.bind_address = value;
                }
            }
        }
        Ok(())
    }

    fn resolve_relative_paths(
        &mut self,
        base_dir: &Path,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let storage_path = self.common.database.storage_path.clone();
        self.common.database.storage_path = Self::resolve_string_path(base_dir, &storage_path)?;

        let log_dir = self.common.log.dir.clone();
        self.common.log.dir = Self::resolve_string_path(base_dir, &log_dir)?;

        let slow_query_log_file = self.common.monitoring.slow_query_log.log_file_path.clone();
        self.common.monitoring.slow_query_log.log_file_path =
            Self::resolve_log_file_name(&self.common.log.dir, &slow_query_log_file)?;

        self.fulltext.index_path = Self::resolve_path_buf(base_dir, &self.fulltext.index_path)?;

        self.common.migration.resolve_relative_paths(base_dir)?;

        #[cfg(feature = "vector")]
        {
            let default_dir = PathBuf::from(&self.common.database.storage_path).join("vector");
            let data_dir = self.vector.local.data_dir.clone().unwrap_or(default_dir);
            self.vector.local.data_dir = Some(Self::resolve_path_buf(base_dir, &data_dir)?);
        }

        #[cfg(feature = "server")]
        {
            let static_dir = self.server.http.static_dir.clone();
            self.server.http.static_dir = Self::resolve_optional_string_path(base_dir, static_dir)?;

            let https_cert_file = self.server.http.https_cert_file.clone();
            self.server.http.https_cert_file =
                Self::resolve_optional_string_path(base_dir, https_cert_file)?;

            let https_key_file = self.server.http.https_key_file.clone();
            self.server.http.https_key_file =
                Self::resolve_optional_string_path(base_dir, https_key_file)?;

            let audit_log_file = self.server.security.audit.log_file.clone();
            self.server.security.audit.log_file =
                Self::resolve_log_file_name(&self.common.log.dir, &audit_log_file)?;
        }

        #[cfg(feature = "embedded")]
        {
            let runtime_path = self.embedded.runtime.path.clone();
            self.embedded.runtime.path = Self::resolve_optional_path_buf(base_dir, runtime_path)?;
        }

        Ok(())
    }

    fn resolve_string_path(
        base_dir: &Path,
        path_value: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        Ok(Self::resolve_path_buf(base_dir, Path::new(path_value))?
            .to_string_lossy()
            .into_owned())
    }

    /// Resolve a log file field against the main log directory.
    ///
    /// Accepts a bare file name or an absolute path. Relative directories are
    /// rejected so every log stream lands under one configured root.
    fn resolve_log_file_name(
        log_dir: &str,
        path_value: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        if Path::new(path_value).is_absolute() {
            return Ok(path_value.to_string());
        }
        Ok(Path::new(log_dir)
            .join(path_value)
            .to_string_lossy()
            .into_owned())
    }

    #[cfg(feature = "server")]
    fn resolve_optional_string_path(
        base_dir: &Path,
        path_value: Option<String>,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        path_value
            .map(|path| Self::resolve_string_path(base_dir, &path))
            .transpose()
    }

    fn merge_toml_values(base: toml::Value, overlay: toml::Value) -> toml::Value {
        match (base, overlay) {
            (toml::Value::Table(mut base_table), toml::Value::Table(overlay_table)) => {
                for (key, overlay_value) in overlay_table {
                    let merged_value = match base_table.remove(&key) {
                        Some(base_value) => Self::merge_toml_values(base_value, overlay_value),
                        None => overlay_value,
                    };
                    base_table.insert(key, merged_value);
                }
                toml::Value::Table(base_table)
            }
            (_, overlay_value) => overlay_value,
        }
    }

    fn resolve_path_buf(
        base_dir: &Path,
        path_value: &Path,
    ) -> Result<PathBuf, Box<dyn std::error::Error>> {
        if path_value.is_absolute() {
            return Ok(path_value.to_path_buf());
        }
        let path_text = path_value.to_string_lossy();
        if let Some(relative_path) = path_text.strip_prefix('~') {
            let home_dir = dirs::home_dir().ok_or("Failed to get user home directory")?;
            let relative_path = relative_path
                .strip_prefix('/')
                .or_else(|| relative_path.strip_prefix('\\'))
                .unwrap_or(relative_path);
            return Ok(home_dir.join(relative_path));
        }
        Ok(base_dir.join(path_value))
    }

    #[cfg(feature = "embedded")]
    fn resolve_optional_path_buf(
        base_dir: &Path,
        path_value: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
        path_value
            .map(|path| Self::resolve_path_buf(base_dir, &path))
            .transpose()
    }

    pub fn validate(&self) -> Result<(), String> {
        self.common.validate()?;
        #[cfg(feature = "server")]
        self.server.validate()?;
        #[cfg(feature = "embedded")]
        self.embedded.validate()?;
        self.fulltext.validate()?;
        Ok(())
    }

    pub fn log_level(&self) -> &str {
        &self.common.log.level
    }
    pub fn log_dir(&self) -> &str {
        &self.common.log.dir
    }
    pub fn log_basename(&self) -> &str {
        &self.common.log.basename
    }
    pub fn host(&self) -> &str {
        &self.common.database.host
    }
    pub fn port(&self) -> u16 {
        self.common.database.port
    }

    #[cfg(feature = "server")]
    pub fn grpc_port(&self) -> u16 {
        self.server.grpc.port
    }

    #[cfg(feature = "server")]
    pub fn grpc(&self) -> &GrpcConfig {
        &self.server.grpc
    }

    #[cfg(feature = "server")]
    pub fn grpc_enabled(&self) -> bool {
        self.server.grpc.enabled
    }

    #[cfg(feature = "server")]
    pub fn grpc_bind_address(&self) -> &str {
        &self.server.grpc.bind_address
    }

    #[cfg(feature = "server")]
    pub fn http_bind_address(&self) -> &str {
        &self.server.http.bind_address
    }

    #[cfg(feature = "server")]
    pub fn http_port(&self) -> u16 {
        self.server.http.port
    }

    pub fn storage_path(&self) -> &str {
        &self.common.database.storage_path
    }
    pub fn max_sessions(&self) -> usize {
        self.common.database.max_sessions
    }
    pub fn transaction_timeout(&self) -> u64 {
        self.common.transaction.default_timeout
    }
    pub fn max_concurrent_transactions(&self) -> usize {
        self.common.transaction.max_concurrent_transactions
    }

    pub fn slow_query_log(&self) -> &SlowQueryLogConfig {
        &self.common.monitoring.slow_query_log
    }
    pub fn to_slow_query_config(&self) -> linkrs_metrics::SlowQueryConfig {
        self.common.monitoring.slow_query_log.to_slow_query_config()
    }
    pub fn storage(&self) -> &StorageConfig {
        &self.common.storage
    }
    pub fn query_resource(&self) -> &QueryResourceConfig {
        &self.common.query_resource
    }
    pub fn columnar(&self) -> &ColumnarConfig {
        &self.common.columnar
    }

    pub fn is_vector_enabled(&self) -> bool {
        #[cfg(feature = "vector")]
        {
            match self.vector.engine {
                VectorEngineKind::Local => self.vector.enabled,
                #[cfg(feature = "vector-qdrant")]
                VectorEngineKind::Qdrant => self.vector.enabled && self.vector.qdrant.enabled,
            }
        }
        #[cfg(not(feature = "vector"))]
        {
            false
        }
    }

    pub fn is_local_vector(&self) -> bool {
        #[cfg(feature = "vector")]
        {
            self.vector.enabled && self.vector.engine == VectorEngineKind::Local
        }
        #[cfg(not(feature = "vector"))]
        {
            false
        }
    }

    pub fn vector_engine(&self) -> Option<VectorEngineKind> {
        #[cfg(feature = "vector")]
        {
            self.vector.enabled.then_some(self.vector.engine)
        }
        #[cfg(not(feature = "vector"))]
        {
            None
        }
    }

    #[cfg(feature = "vector")]
    pub fn vector_config(&self) -> &VectorConfig {
        &self.vector
    }

    #[cfg(feature = "vector")]
    pub fn vector_data_dir(&self) -> PathBuf {
        self.vector
            .local
            .data_dir
            .clone()
            .unwrap_or_else(|| PathBuf::from(&self.common.database.storage_path).join("vector"))
    }
}

/// Where the effective bootstrap password came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapPasswordSource {
    /// Injected through the `LINKRS_PASSWORD` environment variable.
    Environment,
    /// Read from the stored owner-only secret file.
    StoredFile,
    /// Generated during this startup and persisted with owner-only access.
    Generated,
}

/// Environment variable that injects the bootstrap password directly.
#[cfg(feature = "server")]
const BOOTSTRAP_PASSWORD_ENV: &str = "LINKRS_PASSWORD";

/// Length of a generated bootstrap password.
#[cfg(feature = "server")]
const BOOTSTRAP_PASSWORD_LENGTH: usize = 24;

impl Config {
    /// Path of the stored bootstrap password, relative to the data directory.
    pub fn bootstrap_password_path(&self) -> PathBuf {
        PathBuf::from(&self.common.database.storage_path).join("auth/bootstrap_password")
    }

    /// Fill the bootstrap password used to seed and authenticate the default
    /// administrator account.
    ///
    /// Precedence: `LINKRS_PASSWORD`, the stored secret file, then a freshly
    /// generated secret persisted with owner-only permissions. The value is
    /// held in memory only and never written back to the configuration file.
    #[cfg(feature = "server")]
    pub fn resolve_bootstrap_password(&mut self) -> Result<BootstrapPasswordSource, String> {
        if let Some(password) = env::var(BOOTSTRAP_PASSWORD_ENV)
            .ok()
            .filter(|value| !value.is_empty())
        {
            self.server.auth.bootstrap_password = password;
            return Ok(BootstrapPasswordSource::Environment);
        }

        let path = self.bootstrap_password_path();
        match fs::read_to_string(&path) {
            Ok(stored) => {
                let stored = stored.trim().to_string();
                if stored.is_empty() {
                    return Err(format!(
                        "bootstrap password file {} is empty",
                        path.display()
                    ));
                }
                self.server.auth.bootstrap_password = stored;
                Ok(BootstrapPasswordSource::StoredFile)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let generated = generate_bootstrap_password();
                write_owner_only_file(&path, &generated)?;
                self.server.auth.bootstrap_password = generated;
                Ok(BootstrapPasswordSource::Generated)
            }
            Err(error) => Err(format!(
                "failed to read bootstrap password file {}: {}",
                path.display(),
                error
            )),
        }
    }
}

/// Generate a random alphanumeric bootstrap password.
#[cfg(feature = "server")]
fn generate_bootstrap_password() -> String {
    use rand::Rng;

    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(BOOTSTRAP_PASSWORD_LENGTH)
        .map(char::from)
        .collect()
}

/// Write a secret so only the owning user can read it back.
#[cfg(feature = "server")]
fn write_owner_only_file(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {}", parent.display(), error))?;
    }

    #[cfg(unix)]
    {
        use std::io::Write as _;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|error| format!("failed to create {}: {}", path.display(), error))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| format!("failed to write {}: {}", path.display(), error))?;
        // Creation mode is masked by umask; re-apply it so an existing file
        // cannot keep wider permissions.
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("failed to secure {}: {}", path.display(), error))?;
    }

    #[cfg(not(unix))]
    {
        fs::write(path, contents)
            .map_err(|error| format!("failed to write {}: {}", path.display(), error))?;
    }

    Ok(())
}

/// Validate a log file field shared by the slow-query and audit streams.
///
/// Rotation thresholds are expressed in megabytes to match `[log]`, and a
/// relative directory is rejected so the file always lands under the
/// configured log root.
fn validate_log_file_field(
    field: &str,
    path_value: &str,
    max_file_size_mb: u64,
    max_files: u32,
) -> Result<(), String> {
    if max_file_size_mb == 0 {
        return Err(format!("{field}: max file size must be greater than 0"));
    }
    if max_files == 0 {
        return Err(format!("{field}: max files must be greater than 0"));
    }
    let path = Path::new(path_value);
    if !path.is_absolute() && path.parent().is_some_and(|parent| parent != Path::new("")) {
        return Err(format!(
            "{field} must be a file name or an absolute path, got '{path_value}'"
        ));
    }
    Ok(())
}

/// Whether the address restricts the listener to the local machine.
#[cfg(feature = "server")]
fn is_loopback_address(address: &str) -> bool {
    match address.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => address == "localhost",
    }
}

impl std::ops::Deref for Config {
    type Target = CommonConfig;
    fn deref(&self) -> &Self::Target {
        &self.common
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.common
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.common.database.host, "127.0.0.1");
        assert_eq!(config.common.database.port, 9758);
        assert_eq!(config.common.log.level, "info");
        assert_eq!(config.common.optimizer.max_iteration_rounds, 5);
        #[cfg(feature = "server")]
        assert_eq!(config.server.grpc.port, 9669);
        #[cfg(feature = "server")]
        assert!(config.server.grpc.enabled);
    }

    #[test]
    fn test_config_load_save() {
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let config = Config::default();
        let toml_content =
            toml::to_string_pretty(&config).expect("Failed to serialize config to TOML");
        temp_file
            .write_all(toml_content.as_bytes())
            .expect("Failed to write TOML content to temporary file");
        let loaded_config =
            Config::load(temp_file.path()).expect("Failed to load config from temporary file");
        assert_eq!(
            config.common.database.host,
            loaded_config.common.database.host
        );
        assert_eq!(
            config.common.database.port,
            loaded_config.common.database.port
        );
        assert_eq!(config.common.log.level, loaded_config.common.log.level);
    }

    #[test]
    fn test_nested_config_load() {
        let config_content = r#"
[database]
host = "0.0.0.0"
port = 8080
storage_path = "/tmp/linkrs"
max_sessions = 100

[transaction]
default_timeout = 60
max_concurrent_transactions = 500

[log]
level = "debug"
dir = "/var/log/linkrs"
basename = "linkrs"
max_file_size_mb = 100
max_files = 10

[storage]
engine = "propertygraph"
compression = "zstd"
compression_level = 5

[query_resource]
max_concurrent_queries = 50
max_memory_per_query = 1073741824
"#;
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");
        let config = Config::load(temp_file.path()).expect("Failed to load config");
        assert_eq!(config.common.database.host, "0.0.0.0");
        assert_eq!(config.common.database.port, 8080);
        assert_eq!(config.common.transaction.default_timeout, 60);
        assert_eq!(config.common.transaction.max_concurrent_transactions, 500);
        assert_eq!(config.common.log.level, "debug");
        assert_eq!(
            config.common.storage.compression,
            CompressionAlgorithm::Zstd
        );
        assert_eq!(config.common.storage.compression_level, 5);
        assert_eq!(config.common.query_resource.max_concurrent_queries, 50);
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_flat_server_sections_load() {
        let config_content = r#"
[auth]
enable_authorize = false
session_idle_timeout_secs = 120
bcrypt_cost = 6

[http]
enabled = true
port = 8081
cors_enabled = true
cors_allowed_origins = ["https://a.example"]

[security.password_policy]
min_length = 10
max_age_days = 30
history_size = 5

[bootstrap]
single_user_mode = true

[grpc]
enabled = false
"#;
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");
        let config = Config::load(temp_file.path()).expect("Failed to load config");
        assert!(!config.server.auth.enable_authorize);
        assert_eq!(config.server.auth.session_idle_timeout_secs, 120);
        assert_eq!(config.server.auth.bcrypt_cost, 6);
        assert_eq!(config.server.http.port, 8081);
        assert!(config.server.http.cors_enabled);
        assert_eq!(
            config.server.http.cors_allowed_origins,
            vec!["https://a.example".to_string()]
        );
        assert_eq!(config.server.security.password_policy.min_length, 10);
        assert_eq!(config.server.security.password_policy.max_age_days, 30);
        assert_eq!(config.server.security.password_policy.history_size, 5);
        assert!(config.server.bootstrap.single_user_mode);
        assert!(!config.server.grpc.enabled);

        let serialized =
            toml::to_string_pretty(&config).expect("Failed to serialize config to TOML");
        assert!(
            !serialized.contains("[server."),
            "server config must persist in flat form, got:\n{serialized}"
        );
        let reloaded: Config =
            toml::from_str(&serialized).expect("Serialized config must round-trip");
        assert_eq!(reloaded.server.http.port, 8081);
        assert_eq!(reloaded.server.auth.bcrypt_cost, 6);
    }

    #[test]
    fn test_parallel_config_load() {
        let config_content = r#"
[parallel]
enabled = true
workers = 4
min_rows_per_partition = 20000
max_partitions = 4
max_buffered_chunks = 8
vertex_id_start = 0
vertex_id_end = 100000
"#;
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");
        let config = Config::load(temp_file.path()).expect("Failed to load config");
        assert!(config.common.parallel.enabled);
        assert_eq!(config.common.parallel.workers, 4);
        assert_eq!(config.common.parallel.min_rows_per_partition, 20_000);
        assert_eq!(config.common.parallel.max_partitions, 4);
        assert_eq!(config.common.parallel.max_buffered_chunks, 8);
        assert_eq!(config.common.parallel.vertex_id_range(), Some(0..100_000));
    }

    #[test]
    fn test_parallel_config_defaults_when_absent() {
        let config_content = r#"
[database]
host = "0.0.0.0"
"#;
        let mut temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        temp_file
            .write_all(config_content.as_bytes())
            .expect("Failed to write config file");
        let config = Config::load(temp_file.path()).expect("Failed to load config");
        assert!(!config.common.parallel.enabled);
        assert_eq!(config.common.parallel.workers, 1);
        assert!(config.common.parallel.vertex_id_range().is_none());
    }

    #[test]
    fn test_config_load_resolves_relative_paths_from_file_directory() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_dir = temp_dir.path().join("config");
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        let config_content = r#"
[database]
storage_path = "data/linkrs"
"#;
        let config_path = config_dir.join("config.toml");
        std::fs::write(&config_path, config_content).expect("Failed to write config");
        let config = Config::load(&config_path).expect("Failed to load config");
        assert_eq!(
            config.common.database.storage_path,
            config_dir.join("data/linkrs").to_string_lossy()
        );
        assert_eq!(
            config.common.log.dir,
            config_dir.join("logs").to_string_lossy()
        );
        assert_eq!(
            config.common.monitoring.slow_query_log.log_file_path,
            config_dir.join("logs/slow_query.log").to_string_lossy()
        );
        assert_eq!(config.fulltext.index_path, config_dir.join("data/fulltext"));
    }

    #[test]
    fn test_config_load_rejects_unknown_top_level_section() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, "[databse]\nhost = \"127.0.0.1\"\n")
            .expect("Failed to write config");
        let error = Config::load(&config_path)
            .expect_err("a misspelled section must be rejected")
            .to_string();
        assert!(
            error.contains("unknown configuration section '[databse]'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_config_load_rejects_unknown_key_in_section() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(&config_path, "[database]\nhostname = \"127.0.0.1\"\n")
            .expect("Failed to write config");
        let error = Config::load(&config_path)
            .expect_err("a misspelled key must be rejected")
            .to_string();
        assert!(
            error.contains("hostname"),
            "unknown key should be named in the error: {error}"
        );
    }

    #[test]
    fn test_slow_query_log_resolves_under_log_dir() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[log]\ndir = \"main_logs\"\n\n[monitoring.slow_query_log]\nlog_file_path = \"slow.log\"\n",
        )
        .expect("Failed to write config");
        let config = Config::load(&config_path).expect("Failed to load config");
        assert_eq!(
            config.common.monitoring.slow_query_log.log_file_path,
            temp_dir.path().join("main_logs/slow.log").to_string_lossy()
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_audit_log_resolves_under_log_dir() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[log]\ndir = \"main_logs\"\n\n[security.audit]\nlog_file = \"audit.log\"\n",
        )
        .expect("Failed to write config");
        let config = Config::load(&config_path).expect("Failed to load config");
        assert_eq!(
            config.server.security.audit.log_file,
            temp_dir
                .path()
                .join("main_logs/audit.log")
                .to_string_lossy()
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_absolute_audit_log_bypasses_log_dir() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_path = temp_dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "[log]\ndir = \"main_logs\"\n\n[security.audit]\nlog_file = \"/var/audit/audit.log\"\n",
        )
        .expect("Failed to write config");
        let config = Config::load(&config_path).expect("Failed to load config");
        assert_eq!(
            config.server.security.audit.log_file,
            "/var/audit/audit.log"
        );
    }

    #[test]
    fn test_load_user_config_named_uses_linkrs_config_dir() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let config_dir = temp_dir.path().join("user-config");
        std::fs::create_dir_all(&config_dir).expect("Failed to create config directory");
        let config_content = r#"
[database]
storage_path = "storage"
"#;
        std::fs::write(config_dir.join("config.toml"), config_content)
            .expect("Failed to write config");
        let previous_dir = env::var("LINKRS_CONFIG_DIR").ok();
        env::set_var("LINKRS_CONFIG_DIR", &config_dir);
        let config =
            Config::load_user_config_named("config.toml").expect("Failed to load user config");
        assert_eq!(
            config.common.database.storage_path,
            config_dir.join("storage").to_string_lossy()
        );
        if let Some(value) = previous_dir {
            env::set_var("LINKRS_CONFIG_DIR", value);
        } else {
            env::remove_var("LINKRS_CONFIG_DIR");
        }
    }

    #[test]
    fn test_config_validate() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_backward_compatibility() {
        let config = Config::default();
        assert_eq!(config.database.host, "127.0.0.1");
        assert_eq!(config.port(), 9758);
        assert_eq!(config.storage_path(), "data/linkrs");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_server_config() {
        let config = Config::default();
        assert!(config.server.grpc.enabled);
        assert!(config.server.http.enabled);
        assert!(config.server.auth.enable_authorize);
        assert_eq!(config.server.grpc.port, 9669);
        assert_eq!(config.server.http.port, 9758);
    }

    #[cfg(feature = "embedded")]
    #[test]
    fn test_embedded_config() {
        let config = Config::default();
        assert!(config.embedded.runtime.is_memory());
        assert_eq!(config.embedded.runtime.cache_size_mb, 64);
    }

    #[cfg(feature = "vector-qdrant")]
    #[test]
    fn test_vector_section_deserializes_qdrant() {
        let toml = r#"
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/linkrs"
max_sessions = 10

[vector]
enabled = true
engine = "qdrant"

[vector.qdrant]
enabled = true

[vector.qdrant.connection]
host = "localhost"
port = 6334
http_port = 6333
use_tls = false
connect_timeout_secs = 5

[vector.qdrant.timeout]
request_timeout_secs = 30
search_timeout_secs = 10
upsert_timeout_secs = 30
"#;
        let config: Config = toml::from_str(toml).expect("vector section should deserialize");
        assert!(config.is_vector_enabled());
        assert_eq!(config.vector_engine(), Some(VectorEngineKind::Qdrant));
        assert_eq!(
            config.vector_data_dir(),
            std::path::PathBuf::from("data/linkrs/vector")
        );
        assert!(config.vector.qdrant.enabled);
        assert_eq!(config.vector.qdrant.connection.host, "localhost");
        assert_eq!(config.vector.qdrant.connection.port, 6334);
    }

    #[test]
    fn parse_vector_client_config_standalone() {
        let toml = r#"
enabled = true

[connection]
host = "localhost"
port = 6334
http_port = 6333
use_tls = false
connect_timeout_secs = 5

[timeout]
request_timeout_secs = 30
search_timeout_secs = 10
upsert_timeout_secs = 30
"#;
        let c: QdrantConfig = toml::from_str(toml).expect("vc parse");
        assert_eq!(c.connection.host, "localhost");
    }

    #[cfg(feature = "rerank")]
    #[test]
    fn parse_and_validate_rerank_config() {
        let toml = r#"
base_url = "https://api.example.com/v1/rerank"
model = "reranker"
max_candidates = 20
fusion = "linear_weighted"
"#;
        let c: VectorRerankConfig = toml::from_str(toml).expect("rerank parse");
        assert_eq!(c.max_candidates, 20);
        assert!(c.text_field.is_none());
        c.validate().expect("valid rerank config");

        let missing_model = VectorRerankConfig {
            endpoint: RerankConfig::new("https://api.example.com/v1/rerank", ""),
            max_candidates: 20,
            fusion: RerankFusionStrategy::default(),
            text_field: None,
        };
        assert!(missing_model.validate().is_err());

        let bad_window = VectorRerankConfig {
            endpoint: RerankConfig::new("https://api.example.com/v1/rerank", "reranker"),
            max_candidates: 0,
            fusion: RerankFusionStrategy::default(),
            text_field: None,
        };
        assert!(bad_window.validate().is_err());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_generate_bootstrap_password_has_expected_length() {
        let first = generate_bootstrap_password();
        let second = generate_bootstrap_password();
        assert_eq!(first.chars().count(), BOOTSTRAP_PASSWORD_LENGTH);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(first, second, "generated passwords must not repeat");
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_resolve_bootstrap_password_prefers_environment() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let mut config = Config::default();
        config.common.database.storage_path = temp_dir.path().to_string_lossy().into_owned();
        let previous = env::var(BOOTSTRAP_PASSWORD_ENV).ok();
        env::set_var(BOOTSTRAP_PASSWORD_ENV, "injected-password");

        let source = config
            .resolve_bootstrap_password()
            .expect("environment password should resolve");

        if let Some(value) = previous {
            env::set_var(BOOTSTRAP_PASSWORD_ENV, value);
        } else {
            env::remove_var(BOOTSTRAP_PASSWORD_ENV);
        }

        assert_eq!(source, BootstrapPasswordSource::Environment);
        assert_eq!(config.server.auth.bootstrap_password, "injected-password");
        assert!(
            !config.bootstrap_password_path().exists(),
            "the environment password must not touch the secret file"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_resolve_bootstrap_password_generates_and_reuses_secret() {
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");
        let mut config = Config::default();
        config.common.database.storage_path = temp_dir.path().to_string_lossy().into_owned();
        let previous = env::var(BOOTSTRAP_PASSWORD_ENV).ok();
        env::remove_var(BOOTSTRAP_PASSWORD_ENV);

        let first = config
            .resolve_bootstrap_password()
            .expect("generation should succeed");
        let generated = config.server.auth.bootstrap_password.clone();
        let path = config.bootstrap_password_path();
        assert_eq!(first, BootstrapPasswordSource::Generated);
        assert_eq!(generated.chars().count(), BOOTSTRAP_PASSWORD_LENGTH);
        assert_eq!(fs::read_to_string(&path).expect("read secret"), generated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path)
                .expect("stat secret")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "secret file must be owner-only");
        }

        config.server.auth.bootstrap_password.clear();
        let second = config
            .resolve_bootstrap_password()
            .expect("stored password should resolve");
        assert_eq!(second, BootstrapPasswordSource::StoredFile);
        assert_eq!(config.server.auth.bootstrap_password, generated);

        if let Some(value) = previous {
            env::set_var(BOOTSTRAP_PASSWORD_ENV, value);
        }
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_config_debug_never_prints_bootstrap_password() {
        let mut config = Config::default();
        config.server.auth.bootstrap_password = "super-secret-value".to_string();
        let rendered = format!("{:?}", config);
        assert!(
            !rendered.contains("super-secret-value"),
            "config debug leaked the bootstrap password: {rendered}"
        );
    }

    #[cfg(all(feature = "server", feature = "vector-qdrant"))]
    #[test]
    fn test_vector_config_debug_hides_api_keys() {
        let mut config = Config::default();
        config.vector.enabled = true;
        config.vector.engine = VectorEngineKind::Qdrant;
        config.vector.qdrant.enabled = true;
        config.vector.qdrant.connection.api_key = Some("qdrant-secret".to_string());
        let rendered = format!("{:?}", config.vector);
        assert!(
            !rendered.contains("qdrant-secret"),
            "vector debug leaked the API key: {rendered}"
        );
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validate_rejects_duplicate_listener_ports() {
        let mut config = Config::default();
        config.server.grpc.port = config.server.http.port;
        assert!(config.validate().is_err());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validate_rejects_permissive_cors_on_public_bind() {
        let mut config = Config::default();
        config.server.http.bind_address = "0.0.0.0".to_string();
        assert!(
            config.validate().is_err(),
            "permissive CORS on a public bind must be rejected"
        );

        config.server.http.cors_allowed_origins = vec!["https://console.example".to_string()];
        assert!(config.validate().is_ok());
    }

    #[cfg(feature = "server")]
    #[test]
    fn test_validate_rejects_single_user_mode_on_public_bind() {
        let mut config = Config::default();
        config.server.http.bind_address = "::".to_string();
        config.server.bootstrap.single_user_mode = true;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_rejects_zero_slow_query_threshold() {
        let mut config = Config::default();
        config.common.monitoring.slow_query_threshold_ms = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validate_accepts_default_config() {
        assert!(Config::default().validate().is_ok());
    }

    /// The development example from `docs/release/07_configuration_reference.md`
    /// must load and validate as-is: it is the contract of the reference doc.
    #[cfg(all(feature = "server", feature = "vector"))]
    #[test]
    fn test_doc_dev_example_config_loads_and_validates() {
        let file = NamedTempFile::new().expect("temp config file");
        let content = r#"
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/linkrs"
max_sessions = 100

[transaction]
default_timeout = 60
max_concurrent_transactions = 100
auto_commit = false

[log]
level = "debug"
dir = "logs"
basename = "linkrs"
max_file_size_mb = 50
max_files = 3

[auth]
enable_authorize = false
failed_login_attempts = 0
session_idle_timeout_secs = 7200
default_username = "root"
force_change_default_password = false
bcrypt_cost = 6

[bootstrap]
auto_create_default_space = true
default_space_name = "default"
single_user_mode = true

[grpc]
enabled = true
port = 9669

[parallel]
enabled = false

[vector]
enabled = true
engine = "local"

[fulltext]
enabled = false

[optimizer]
max_iteration_rounds = 3
max_exploration_rounds = 64
enable_cost_model = true
enable_multi_plan = true
enable_property_pruning = true
enable_adaptive_iteration = true
stable_threshold = 2
min_iteration_rounds = 1

[monitoring]
enabled = true
memory_cache_size = 500
slow_query_threshold_ms = 500
"#;
        std::fs::write(file.path(), content).expect("write config");
        let config = Config::load(file.path()).expect("dev example must load");
        config.validate().expect("dev example must validate");
        assert!(config.is_local_vector());
    }

    /// The production example (Qdrant vector backend) must load and validate
    /// as-is under the matching feature set.
    #[cfg(all(feature = "server", feature = "vector-qdrant"))]
    #[test]
    fn test_doc_prod_example_config_loads_and_validates() {
        let file = NamedTempFile::new().expect("temp config file");
        let content = r#"
[database]
host = "0.0.0.0"
port = 9758
storage_path = "/var/lib/linkrs/data"
max_sessions = 100

[transaction]
default_timeout = 30
max_concurrent_transactions = 2000
auto_commit = false

[log]
level = "info"
dir = "/var/log/linkrs"
basename = "linkrs"
max_file_size_mb = 500
max_files = 20

[auth]
enable_authorize = true
failed_login_attempts = 5
session_idle_timeout_secs = 3600
default_username = "admin"
force_change_default_password = true
bcrypt_cost = 12

[bootstrap]
auto_create_default_space = true
default_space_name = "production"
single_user_mode = false

[http]
cors_enabled = true
cors_allowed_origins = ["https://console.example.com"]

[security.password_policy]
min_length = 8
require_uppercase = true
require_lowercase = true
require_digit = true
require_special = false
max_age_days = 90
history_size = 5

[query_resource]
max_concurrent_queries = 100
query_timeout_secs = 60

[storage]
max_memory_bytes = 1073741824
index_memory_bytes = 268435456

[vector]
enabled = true
engine = "qdrant"

[vector.qdrant.connection]
host = "qdrant.example.com"
port = 6334
http_port = 6333
use_tls = true

[vector.qdrant.timeout]
request_timeout_secs = 30
search_timeout_secs = 60
upsert_timeout_secs = 30

[vector.qdrant.embedding]
base_url = "https://api.openai.com/v1/embeddings"
api_key = "sk-xxx"
model = "text-embedding-3-small"
dimension = 1536
timeout_secs = 60

[fulltext]
enabled = true
index_path = "/var/lib/linkrs/fulltext"

[fulltext.tantivy]
writer_memory_budget = 100000000
tokenizer = "jieba"

[optimizer]
max_iteration_rounds = 10
max_exploration_rounds = 256
enable_cost_model = true
enable_multi_plan = true
enable_property_pruning = true
enable_adaptive_iteration = true
stable_threshold = 2
min_iteration_rounds = 1

[monitoring]
enabled = true
memory_cache_size = 5000
slow_query_threshold_ms = 1000

[monitoring.slow_query_log]
enabled = true
threshold_ms = 1000
log_file_path = "/var/log/linkrs/slow_query.log"
max_file_size_mb = 500
max_files = 10
"#;
        std::fs::write(file.path(), content).expect("write config");
        let config = Config::load(file.path()).expect("prod example must load");
        config.validate().expect("prod example must validate");
        assert_eq!(config.vector_engine(), Some(VectorEngineKind::Qdrant));
        assert_eq!(config.common.monitoring.slow_query_log.max_files, 10);
    }
}
