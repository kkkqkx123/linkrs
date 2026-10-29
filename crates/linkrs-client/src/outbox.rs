//! HTTP client extensions for outbox/sync operations.

use crate::http_client::HttpClient;
use crate::utils::error::{ClientError, Result};

impl HttpClient {
    pub async fn outbox_diagnostics(&self) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/diagnostics", self.base_url());
        let resp = self
            .authed(self.inner().get(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to get outbox diagnostics").await
    }

    pub async fn outbox_dead_letters(
        &self,
        target: Option<&str>,
        index_id: Option<u64>,
        generation: Option<u64>,
        limit: usize,
        offset: usize,
    ) -> Result<serde_json::Value> {
        let mut url = format!(
            "{}/sync/outbox/dead_letters?limit={}&offset={}",
            self.base_url(),
            limit,
            offset
        );
        if let Some(t) = target {
            url.push_str(&format!("&target={}", t));
        }
        if let Some(id) = index_id {
            url.push_str(&format!("&index_id={}", id));
        }
        if let Some(gen) = generation {
            url.push_str(&format!("&generation={}", gen));
        }
        let resp = self
            .authed(self.inner().get(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to get outbox dead letters").await
    }

    pub async fn outbox_requeue(&self, req: &impl serde::Serialize) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/requeue", self.base_url());
        let resp = self
            .authed(self.inner().post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to requeue outbox entries").await
    }

    pub async fn outbox_degraded_ranges(
        &self,
        target: Option<&str>,
        index_id: Option<u64>,
        generation: Option<u64>,
    ) -> Result<serde_json::Value> {
        let mut url = format!("{}/sync/outbox/degraded_ranges", self.base_url());
        let mut first = true;
        if let Some(t) = target {
            url.push_str(&format!("{}target={}", if first { "?" } else { "&" }, t));
            first = false;
        }
        if let Some(id) = index_id {
            url.push_str(&format!("{}index_id={}", if first { "?" } else { "&" }, id));
            first = false;
        }
        if let Some(gen) = generation {
            url.push_str(&format!(
                "{}generation={}",
                if first { "?" } else { "&" },
                gen
            ));
        }
        let resp = self
            .authed(self.inner().get(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to get degraded ranges").await
    }

    pub async fn outbox_degraded_clear(
        &self,
        req: &impl serde::Serialize,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/degraded/clear", self.base_url());
        let resp = self
            .authed(self.inner().post(&url))
            .json(req)
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to clear degraded ranges").await
    }

    pub async fn outbox_retry(&self) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/retry", self.base_url());
        let resp = self
            .authed(self.inner().post(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to retry outbox").await
    }

    pub async fn sync_status(&self) -> Result<serde_json::Value> {
        let url = format!("{}/sync/status", self.base_url());
        let resp = self
            .authed(self.inner().get(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to get sync status").await
    }

    pub async fn retention_status(&self) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/retention/status", self.base_url());
        let resp = self
            .authed(self.inner().get(&url))
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to get retention status").await
    }

    pub async fn retention_run(
        &self,
        grace_lsn_distance: Option<u64>,
        max_age_ms: Option<u64>,
    ) -> Result<serde_json::Value> {
        let url = format!("{}/sync/outbox/retention/run", self.base_url());
        let body = serde_json::json!({
            "grace_lsn_distance": grace_lsn_distance,
            "max_age_ms": max_age_ms,
        });
        let resp = self
            .authed(self.inner().post(&url))
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))?;
        Self::check_json_response(resp, "Failed to run retention").await
    }

    async fn check_json_response(
        resp: reqwest::Response,
        context: &str,
    ) -> Result<serde_json::Value> {
        let resp = Self::check_response(resp, context).await?;
        resp.json()
            .await
            .map_err(|e| ClientError::connection(e.to_string()))
    }
}
