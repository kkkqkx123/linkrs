//! Client state, transport construction, and session helpers.
//!
//! [`GrpcClient`] owns the tonic channel and the stored session id; the
//! sibling modules add the per-service RPC methods on top of the helpers
//! defined here.

use std::sync::RwLock;
use std::time::Duration;

use tonic::transport::{Channel, Endpoint};

use crate::config::ClientConfig;
use crate::utils::error::{ClientError, Result};

use super::proto::linkrs_service_client::LinkrsServiceClient;

/// Request metadata key carrying the session id issued by `Login`.
pub(super) const SESSION_METADATA_KEY: &str = "x-session-id";

/// TLS settings for a gRPC endpoint.
///
/// The defaults verify the server against the system root certificates and use
/// the endpoint host as the expected domain name.
#[derive(Debug, Clone, Default)]
pub struct GrpcTlsConfig {
    /// PEM-encoded CA certificate(s) used to verify the server. When absent,
    /// the platform's trusted roots are used.
    pub ca_certificate_pem: Option<Vec<u8>>,
    /// Domain name used for certificate verification. When absent, the host
    /// component of the endpoint URI is used.
    pub domain_name: Option<String>,
}

impl GrpcTlsConfig {
    /// Settings for the system root store with the endpoint host as domain.
    pub fn new() -> Self {
        Self::default()
    }

    /// Verify the server against the given PEM-encoded CA certificate(s).
    pub fn with_ca_certificate_pem(mut self, pem: impl Into<Vec<u8>>) -> Self {
        self.ca_certificate_pem = Some(pem.into());
        self
    }

    /// Override the domain name used for certificate verification.
    pub fn with_domain_name(mut self, domain: impl Into<String>) -> Self {
        self.domain_name = Some(domain.into());
        self
    }

    fn client_tls_config(&self) -> tonic::transport::ClientTlsConfig {
        install_crypto_provider();
        let mut config = tonic::transport::ClientTlsConfig::new();
        match &self.ca_certificate_pem {
            Some(pem) => {
                config = config.ca_certificate(tonic::transport::Certificate::from_pem(pem));
            }
            None => config = config.with_native_roots(),
        }
        if let Some(domain) = &self.domain_name {
            config = config.domain_name(domain.clone());
        }
        config
    }
}

/// Install a process-wide rustls provider when none has been installed yet.
///
/// `reqwest` enables the `aws-lc-rs` provider while tonic enables `ring`, so
/// rustls cannot pick a default on its own and TLS setup would panic. Selecting
/// `ring` here resolves that ambiguity without changing plaintext connections.
fn install_crypto_provider() {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
}

/// Typed gRPC client for a Linkrs server.
///
/// Cheap to construct; the underlying channel reconnects lazily when built via
/// [`GrpcClient::new`] / [`GrpcClient::with_config`], while
/// [`GrpcClient::connect`] verifies reachability up front.
pub struct GrpcClient {
    inner: LinkrsServiceClient<Channel>,
    endpoint: String,
    session: RwLock<Option<String>>,
    max_message_size: Option<usize>,
}

impl GrpcClient {
    /// Create a client for `host:port` with default settings (lazy channel).
    pub fn new(host: &str, port: u16) -> Result<Self> {
        Self::with_config(ClientConfig::new().with_host(host).with_port(port))
    }

    /// Create a client from a [`ClientConfig`] (lazy channel). The configured
    /// timeout bounds both connection establishment and individual calls.
    ///
    /// [`ClientConfig`] defaults to the HTTP port; pass the gRPC port explicitly
    /// (the server default is `9669`).
    pub fn with_config(config: ClientConfig) -> Result<Self> {
        let endpoint = endpoint_address(&config.host, config.port);
        Self::lazy_from_endpoint(endpoint, config.timeout_seconds, None)
    }

    /// Create a TLS-enabled client from a [`ClientConfig`] (lazy channel).
    pub fn with_config_and_tls(config: ClientConfig, tls: GrpcTlsConfig) -> Result<Self> {
        let endpoint = endpoint_address(&config.host, config.port);
        Self::lazy_from_endpoint(endpoint, config.timeout_seconds, Some(&tls))
    }

    /// Create a client from a complete endpoint URI such as `http://host:9669`
    /// or `https://host:9669` (lazy channel).
    pub fn from_endpoint(endpoint: impl Into<String>, timeout_seconds: u64) -> Result<Self> {
        Self::lazy_from_endpoint(endpoint.into(), timeout_seconds, None)
    }

    /// Create a TLS-enabled client from a complete endpoint URI (lazy channel).
    pub fn from_endpoint_with_tls(
        endpoint: impl Into<String>,
        timeout_seconds: u64,
        tls: GrpcTlsConfig,
    ) -> Result<Self> {
        Self::lazy_from_endpoint(endpoint.into(), timeout_seconds, Some(&tls))
    }

    fn lazy_from_endpoint(
        endpoint: String,
        timeout_seconds: u64,
        tls: Option<&GrpcTlsConfig>,
    ) -> Result<Self> {
        let channel = build_endpoint(&endpoint, timeout_seconds, tls)?.connect_lazy();
        Ok(Self::from_channel(channel, endpoint))
    }

    /// Connect to `host:port` eagerly, failing if the server is unreachable.
    pub async fn connect(host: &str, port: u16) -> Result<Self> {
        let endpoint = endpoint_address(host, port);
        Self::eager_connect(endpoint, ClientConfig::default().timeout_seconds, None).await
    }

    /// Connect to a complete endpoint URI eagerly.
    pub async fn connect_endpoint(
        endpoint: impl Into<String>,
        timeout_seconds: u64,
    ) -> Result<Self> {
        Self::eager_connect(endpoint.into(), timeout_seconds, None).await
    }

    /// Connect to a complete endpoint URI eagerly over TLS.
    pub async fn connect_endpoint_with_tls(
        endpoint: impl Into<String>,
        timeout_seconds: u64,
        tls: GrpcTlsConfig,
    ) -> Result<Self> {
        Self::eager_connect(endpoint.into(), timeout_seconds, Some(&tls)).await
    }

    async fn eager_connect(
        endpoint: String,
        timeout_seconds: u64,
        tls: Option<&GrpcTlsConfig>,
    ) -> Result<Self> {
        let channel = build_endpoint(&endpoint, timeout_seconds, tls)?
            .connect()
            .await
            .map_err(|e| {
                ClientError::connection(format!(
                    "Failed to connect to gRPC endpoint {}: {}",
                    endpoint, e
                ))
            })?;
        Ok(Self::from_channel(channel, endpoint))
    }

    /// Configure the maximum decoded/encoded message size for subsequent calls.
    ///
    /// The server defaults to 10 MB while tonic clients default to 4 MB, so
    /// setting this to the server value avoids truncation errors on large
    /// query results.
    pub fn with_message_size_limit(mut self, bytes: usize) -> Self {
        self.max_message_size = Some(bytes);
        self
    }

    fn from_channel(channel: Channel, endpoint: String) -> Self {
        Self {
            inner: LinkrsServiceClient::new(channel),
            endpoint,
            session: RwLock::new(None),
            max_message_size: None,
        }
    }

    pub(super) fn client(&self) -> LinkrsServiceClient<Channel> {
        let mut client = self.inner.clone();
        if let Some(bytes) = self.max_message_size {
            client = client
                .max_decoding_message_size(bytes)
                .max_encoding_message_size(bytes);
        }
        client
    }

    /// Endpoint address the client talks to.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Session id issued by the last successful [`GrpcClient::login`].
    pub fn session_id(&self) -> Option<i64> {
        self.stored_session().and_then(|id| id.parse().ok())
    }

    /// Restore or replace the session used by subsequent protected calls.
    pub fn set_session_id(&self, session_id: i64) {
        self.store_session(session_id.to_string());
    }

    /// Attach the session metadata required by protected RPCs.
    pub fn authed<T>(&self, mut request: tonic::Request<T>) -> tonic::Request<T> {
        if let Some(session) = self.stored_session() {
            if let (Ok(key), Ok(value)) = (
                tonic::metadata::MetadataKey::from_bytes(SESSION_METADATA_KEY.as_bytes()),
                tonic::metadata::MetadataValue::<tonic::metadata::Ascii>::try_from(
                    session.as_str(),
                ),
            ) {
                request.metadata_mut().insert(key, value);
            }
        }
        request
    }

    /// Drop the stored session id so it does not linger after logout.
    pub fn clear_credentials(&self) {
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = None;
    }

    pub(super) fn stored_session(&self) -> Option<String> {
        self.session
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub(super) fn store_session(&self, session_id: String) {
        *self.session.write().unwrap_or_else(|e| e.into_inner()) = Some(session_id);
    }

    pub(super) fn require_session(&self) -> Result<String> {
        self.stored_session()
            .ok_or_else(|| ClientError::auth("no active gRPC session; call login() first"))
    }

    pub(super) fn fill_optional_session(&self, slot: &mut Option<String>) {
        if slot.as_deref().is_none_or(|value| value.is_empty()) {
            *slot = self.stored_session();
        }
    }

    pub(super) fn fill_required_session(&self, slot: &mut String) -> Result<()> {
        if slot.is_empty() {
            *slot = self.require_session()?;
        }
        Ok(())
    }

    pub(super) fn fill_optional_session_i64(&self, slot: &mut i64) -> Result<()> {
        if *slot == 0 {
            *slot = self
                .session_id()
                .ok_or_else(|| ClientError::auth("no active gRPC session; call login() first"))?;
        }
        Ok(())
    }

    pub(super) fn fill_optional_owner(&self, slot: &mut Option<String>) {
        if slot.as_deref().is_none_or(|value| value.is_empty()) {
            *slot = self.stored_session();
        }
    }
}

fn endpoint_address(host: &str, port: u16) -> String {
    format!("http://{}:{}", host, port)
}

fn build_endpoint(
    address: &str,
    timeout_seconds: u64,
    tls: Option<&GrpcTlsConfig>,
) -> Result<Endpoint> {
    let mut endpoint = Endpoint::from_shared(address.to_string())
        .map_err(|e| ClientError::config(format!("Invalid gRPC endpoint {}: {}", address, e)))?;
    if let Some(tls) = tls {
        endpoint = endpoint.tls_config(tls.client_tls_config()).map_err(|e| {
            ClientError::config(format!(
                "Invalid gRPC TLS configuration for {}: {}",
                address, e
            ))
        })?;
    }
    if timeout_seconds == 0 {
        return Ok(endpoint);
    }
    Ok(endpoint
        .timeout(Duration::from_secs(timeout_seconds))
        .connect_timeout(Duration::from_secs(timeout_seconds)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grpc_client::proto::HealthCheckRequest;

    fn test_client() -> GrpcClient {
        GrpcClient::new("127.0.0.1", 7070).expect("client should build")
    }

    #[tokio::test]
    async fn endpoint_address_is_built_from_host_and_port() {
        let client = test_client();
        assert_eq!(client.endpoint(), "http://127.0.0.1:7070");
        assert!(client.session_id().is_none());
    }

    #[tokio::test]
    async fn authed_without_session_omits_session_metadata() {
        let client = test_client();
        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        assert!(request.metadata().get(SESSION_METADATA_KEY).is_none());
    }

    #[tokio::test]
    async fn authed_injects_stored_session_metadata() {
        let client = test_client();
        client.store_session("42".to_string());
        assert_eq!(client.session_id(), Some(42));

        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        let value = request
            .metadata()
            .get(SESSION_METADATA_KEY)
            .map(|value| value.to_str().unwrap_or_default().to_string());
        assert_eq!(value.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn session_id_is_none_for_non_numeric_session() {
        let client = test_client();
        client.store_session("not-a-number".to_string());
        assert!(client.session_id().is_none());
    }

    #[tokio::test]
    async fn clear_credentials_drops_stored_session() {
        let client = test_client();
        client.store_session("42".to_string());
        client.clear_credentials();
        assert!(client.session_id().is_none());

        let request = client.authed(tonic::Request::new(HealthCheckRequest {}));
        assert!(request.metadata().get(SESSION_METADATA_KEY).is_none());
    }

    #[tokio::test]
    async fn optional_session_is_filled_only_when_missing() {
        let client = test_client();
        client.set_session_id(42);

        let mut slot = None;
        client.fill_optional_session(&mut slot);
        assert_eq!(slot.as_deref(), Some("42"));

        let mut explicit = Some("other".to_string());
        client.fill_optional_session(&mut explicit);
        assert_eq!(explicit.as_deref(), Some("other"));
    }

    #[tokio::test]
    async fn required_session_fails_without_login() {
        let client = test_client();
        let mut slot = String::new();
        let error = client
            .fill_required_session(&mut slot)
            .expect_err("expected auth error");
        assert!(matches!(error, ClientError::AuthError(_)));

        client.set_session_id(42);
        let mut slot = String::new();
        client
            .fill_required_session(&mut slot)
            .expect("session should be filled");
        assert_eq!(slot, "42");
    }

    #[tokio::test]
    async fn optional_int_session_and_owner_are_filled() {
        let client = test_client();
        client.set_session_id(42);

        let mut slot = 0;
        client
            .fill_optional_session_i64(&mut slot)
            .expect("session should be filled");
        assert_eq!(slot, 42);

        let mut owner = None;
        client.fill_optional_owner(&mut owner);
        assert_eq!(owner.as_deref(), Some("42"));
    }

    #[tokio::test]
    async fn endpoint_and_message_limit_configuration_is_applied() {
        let client = GrpcClient::from_endpoint("https://127.0.0.1:9669", 0)
            .expect("client should build")
            .with_message_size_limit(10 * 1024 * 1024);
        assert_eq!(client.endpoint(), "https://127.0.0.1:9669");
        assert_eq!(client.max_message_size, Some(10 * 1024 * 1024));
    }

    #[test]
    fn tls_config_builder_records_certificate_and_domain() {
        let tls = GrpcTlsConfig::new()
            .with_ca_certificate_pem(b"-----BEGIN CERTIFICATE-----".to_vec())
            .with_domain_name("linkrs.example");
        assert_eq!(
            tls.ca_certificate_pem.as_deref(),
            Some(b"-----BEGIN CERTIFICATE-----".as_slice())
        );
        assert_eq!(tls.domain_name.as_deref(), Some("linkrs.example"));
    }

    #[test]
    fn tls_endpoint_is_built_from_https_uri() {
        let tls = GrpcTlsConfig::new();
        let endpoint = build_endpoint("https://127.0.0.1:9669", 0, Some(&tls))
            .expect("TLS endpoint should build");
        assert_eq!(endpoint.uri().scheme_str(), Some("https"));
    }

    #[test]
    fn plaintext_endpoint_skips_tls_configuration() {
        let endpoint = build_endpoint("http://127.0.0.1:9669", 0, None)
            .expect("plaintext endpoint should build");
        assert_eq!(endpoint.uri().scheme_str(), Some("http"));
    }

    #[test]
    fn invalid_endpoint_is_a_config_error() {
        let error = build_endpoint("not a uri", 0, None).expect_err("expected config error");
        assert!(matches!(error, ClientError::ConfigError(_)));
    }

    #[tokio::test]
    async fn logout_without_session_is_an_auth_error() {
        let client = test_client();
        let result = client.logout().await;
        assert!(
            matches!(&result, Err(ClientError::AuthError(_))),
            "expected auth error, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn auth_me_without_session_is_an_auth_error() {
        let client = test_client();
        let result = client.auth_me().await;
        assert!(
            matches!(&result, Err(ClientError::AuthError(_))),
            "expected auth error, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn calls_to_unreachable_endpoint_return_error() {
        let config = ClientConfig::new()
            .with_host("127.0.0.1")
            .with_port(1)
            .with_timeout(2);
        let client = GrpcClient::with_config(config).expect("client should build");
        let result = client.health_check().await;
        assert!(result.is_err(), "expected transport error");
    }
}
