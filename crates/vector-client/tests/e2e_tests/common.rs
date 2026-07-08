use std::sync::OnceLock;

use vector_client::api::VectorClient;
use vector_client::config::VectorClientConfig;

const QDRANT_HTTP_PORT: u16 = 6333;
const QDRANT_GRPC_PORT: u16 = 6334;

fn qdrant_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let url = format!("http://localhost:{}/healthz", QDRANT_HTTP_PORT);
        let log_url = url.clone();
        let result = std::thread::spawn(move || {
            let client = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(2))
                .build()
                .ok()?;
            let resp = client.get(&url).send().ok()?;
            Some(resp.status().is_success())
        })
        .join();
        match result {
            Ok(Some(true)) => {
                eprintln!("[E2E] Qdrant available at {}", log_url);
                true
            }
            _ => {
                eprintln!(
                    "[E2E] Qdrant not available at {}, skipping E2E tests",
                    log_url
                );
                false
            }
        }
    })
}

fn grpc_port() -> u16 {
    std::env::var("QDRANT_GRPC_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(QDRANT_GRPC_PORT)
}

fn http_port() -> u16 {
    std::env::var("QDRANT_HTTP_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(QDRANT_HTTP_PORT)
}

pub async fn create_e2e_client() -> Option<VectorClient> {
    if !qdrant_available() {
        return None;
    }
    let config = VectorClientConfig::qdrant_local("localhost", grpc_port(), http_port());
    Some(VectorClient::new(config).await.expect("create e2e client"))
}

pub async fn ensure_deleted(client: &VectorClient, name: &str) {
    if client
        .engine()
        .collection_exists(name)
        .await
        .unwrap_or(false)
    {
        client.engine().delete_collection(name).await.ok();
    }
}

pub fn test_collection(name: &str) -> String {
    format!("e2e_{}", name)
}
