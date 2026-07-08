//! Metadata Models (Query History & Favorites)

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Query history item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: String,
    pub session_id: String,
    pub query: String,
    pub executed_at: DateTime<Utc>,
    pub execution_time_ms: i64,
    pub rows_returned: i64,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Request to add history item
#[derive(Debug, Deserialize)]
pub struct AddHistoryRequest {
    pub query: String,
    pub execution_time_ms: i64,
    pub rows_returned: i64,
    pub success: bool,
    pub error_message: Option<String>,
}

/// Query favorite item
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FavoriteItem {
    pub id: String,
    pub session_id: String,
    pub name: String,
    pub query: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Request to add favorite
#[derive(Debug, Deserialize)]
pub struct AddFavoriteRequest {
    pub name: String,
    pub query: String,
    pub description: Option<String>,
}

/// Request to update favorite
#[derive(Debug, Deserialize)]
pub struct UpdateFavoriteRequest {
    pub name: Option<String>,
    pub query: Option<String>,
    pub description: Option<String>,
}

/// Favorite list response
#[derive(Debug, Serialize)]
pub struct FavoriteListResponse {
    pub items: Vec<FavoriteItem>,
}

/// History list response
#[derive(Debug, Serialize)]
pub struct HistoryListResponse {
    pub items: Vec<HistoryItem>,
    pub total: i64,
}
