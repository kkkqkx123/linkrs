//! Metadata Service

use chrono::Utc;
use std::sync::Arc;
use uuid::Uuid;

use crate::api::server::web::error::{WebError, WebResult};
use crate::api::server::web::models::metadata::{
    AddFavoriteRequest, AddHistoryRequest, FavoriteItem, HistoryItem, UpdateFavoriteRequest,
};
use crate::api::server::web::storage::{MetadataStorage, SqliteStorage};

/// Metadata service for query history and favorites
pub struct MetadataService {
    storage: Arc<SqliteStorage>,
}

impl MetadataService {
    /// Create a new metadata service
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    /// Add a query history item
    pub async fn add_history(
        &self,
        session_id: &str,
        request: AddHistoryRequest,
    ) -> WebResult<HistoryItem> {
        let item = HistoryItem {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            query: request.query,
            executed_at: Utc::now(),
            execution_time_ms: request.execution_time_ms,
            rows_returned: request.rows_returned,
            success: request.success,
            error_message: request.error_message,
        };

        self.storage.add_history(&item).await?;
        Ok(item)
    }

    /// Get query history for a session
    pub async fn get_history(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> WebResult<(Vec<HistoryItem>, i64)> {
        self.storage.get_history(session_id, limit, offset).await
    }

    /// Delete a history item
    pub async fn delete_history(&self, id: &str, session_id: &str) -> WebResult<()> {
        self.storage.delete_history(id, session_id).await
    }

    /// Clear all history for a session
    pub async fn clear_history(&self, session_id: &str) -> WebResult<()> {
        self.storage.clear_history(session_id).await
    }

    /// Add a favorite
    pub async fn add_favorite(
        &self,
        session_id: &str,
        request: AddFavoriteRequest,
    ) -> WebResult<FavoriteItem> {
        // Validate name is not empty
        if request.name.trim().is_empty() {
            return Err(WebError::BadRequest(
                "Favorite name cannot be empty".to_string(),
            ));
        }

        // Validate query is not empty
        if request.query.trim().is_empty() {
            return Err(WebError::BadRequest("Query cannot be empty".to_string()));
        }

        let item = FavoriteItem {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            name: request.name,
            query: request.query,
            description: request.description,
            created_at: Utc::now(),
        };

        self.storage.add_favorite(&item).await?;
        Ok(item)
    }

    /// Get all favorites for a session
    pub async fn get_favorites(&self, session_id: &str) -> WebResult<Vec<FavoriteItem>> {
        self.storage.get_favorites(session_id).await
    }

    /// Get a favorite by ID
    pub async fn get_favorite(&self, id: &str, session_id: &str) -> WebResult<FavoriteItem> {
        self.storage.get_favorite(id, session_id).await
    }

    /// Update a favorite
    pub async fn update_favorite(
        &self,
        id: &str,
        session_id: &str,
        request: UpdateFavoriteRequest,
    ) -> WebResult<FavoriteItem> {
        // Get existing favorite
        let mut item = self.storage.get_favorite(id, session_id).await?;

        // Update fields if provided
        if let Some(name) = request.name {
            if name.trim().is_empty() {
                return Err(WebError::BadRequest(
                    "Favorite name cannot be empty".to_string(),
                ));
            }
            item.name = name;
        }

        if let Some(query) = request.query {
            if query.trim().is_empty() {
                return Err(WebError::BadRequest("Query cannot be empty".to_string()));
            }
            item.query = query;
        }

        if request.description.is_some() {
            item.description = request.description;
        }

        self.storage.update_favorite(id, session_id, &item).await?;
        Ok(item)
    }

    /// Delete a favorite
    pub async fn delete_favorite(&self, id: &str, session_id: &str) -> WebResult<()> {
        self.storage.delete_favorite(id, session_id).await
    }

    /// Delete all favorites for a session
    pub async fn delete_all_favorites(&self, session_id: &str) -> WebResult<()> {
        self.storage.delete_all_favorites(session_id).await
    }
}
