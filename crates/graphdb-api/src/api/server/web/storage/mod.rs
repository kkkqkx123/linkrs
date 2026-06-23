//! Metadata Storage Module
//!
//! Provides SQLite-based storage for:
//! - Query history
//! - Query favorites

use async_trait::async_trait;

use crate::api::server::web::error::WebResult;
use crate::api::server::web::models::metadata::{FavoriteItem, HistoryItem};

mod sqlite;

pub use sqlite::SqliteStorage;

/// Metadata storage trait
#[async_trait]
pub trait MetadataStorage: Send + Sync {
    /// Add a history item
    async fn add_history(&self, item: &HistoryItem) -> WebResult<()>;

    /// Get history items for a session
    async fn get_history(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> WebResult<(Vec<HistoryItem>, i64)>;

    /// Delete a history item
    async fn delete_history(&self, id: &str, session_id: &str) -> WebResult<()>;

    /// Clear all history for a session
    async fn clear_history(&self, session_id: &str) -> WebResult<()>;

    /// Add a favorite item
    async fn add_favorite(&self, item: &FavoriteItem) -> WebResult<()>;

    /// Get all favorites for a session
    async fn get_favorites(&self, session_id: &str) -> WebResult<Vec<FavoriteItem>>;

    /// Get a favorite by ID
    async fn get_favorite(&self, id: &str, session_id: &str) -> WebResult<FavoriteItem>;

    /// Update a favorite
    async fn update_favorite(
        &self,
        id: &str,
        session_id: &str,
        item: &FavoriteItem,
    ) -> WebResult<()>;

    /// Delete a favorite
    async fn delete_favorite(&self, id: &str, session_id: &str) -> WebResult<()>;

    /// Delete all favorites for a session
    async fn delete_all_favorites(&self, session_id: &str) -> WebResult<()>;
}
