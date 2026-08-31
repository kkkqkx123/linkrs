use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlanQuery {
    pub from_version: Option<u64>,
    pub to_version: Option<u64>,
    pub is_edge: Option<bool>,
    pub expand_contract: Option<bool>,
}

impl MigrationPlanQuery {
    pub fn require_from_version(&self) -> Result<u64, String> {
        self.from_version.ok_or_else(|| "from_version required".to_string())
    }
    pub fn require_to_version(&self) -> Result<u64, String> {
        self.to_version.ok_or_else(|| "to_version required".to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationExecuteRequest {
    pub plan_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationRollbackRequest {
    pub plan_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationPlanResponse {
    pub plan: serde_json::Value,
    pub plan_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationExecuteResponse {
    pub success: bool,
    pub steps_completed: usize,
    pub rows_migrated: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationStatusResponse {
    pub space: Option<String>,
    pub label: Option<String>,
    pub is_edge: Option<bool>,
    pub latest_applied_version: Option<u64>,
    pub applied_versions: Vec<u64>,
    pub history_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationHistoryEntry {
    pub id: u64,
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub from_version: u64,
    pub to_version: u64,
    pub safety_level: String,
    pub steps_count: usize,
    pub rows_migrated: u64,
    pub status: String,
    pub applied_at: u64,
    pub completed_at: Option<u64>,
    pub error_message: Option<String>,
    pub file_entry_id: Option<String>,
    pub file_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationHistoryResponse {
    pub space: String,
    pub label: String,
    pub is_edge: bool,
    pub applied_versions: Vec<u64>,
    pub history: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dto_serde_roundtrip() {
        let req = MigrationExecuteRequest {
            plan_json: "{}".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: MigrationExecuteRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.plan_json, "{}");

        let query = MigrationPlanQuery {
            from_version: Some(1),
            to_version: Some(2),
            is_edge: Some(false),
            expand_contract: Some(true),
        };
        let json = serde_json::to_string(&query).unwrap();
        let decoded: MigrationPlanQuery = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.from_version, Some(1));
    }

    #[test]
    fn test_status_response() {
        let resp = MigrationStatusResponse {
            space: Some("s".into()),
            label: Some("l".into()),
            is_edge: Some(false),
            latest_applied_version: Some(5),
            applied_versions: vec![1, 2, 5],
            history_count: 3,
        };
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: MigrationStatusResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.latest_applied_version, Some(5));
    }
}
