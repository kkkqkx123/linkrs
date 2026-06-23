//! SQLite Storage Implementation

use chrono::Utc;
use sqlx::{sqlite::SqlitePoolOptions, Pool, Row, Sqlite};

use crate::api::server::web::error::{WebError, WebResult};
use crate::api::server::web::models::metadata::{FavoriteItem, HistoryItem};

use super::MetadataStorage;

/// SQLite storage for metadata
pub struct SqliteStorage {
    pool: Pool<Sqlite>,
}

impl SqliteStorage {
    /// Create a new SQLite storage instance
    pub async fn new(database_path: &str) -> WebResult<Self> {
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(database_path)
            .await
            .map_err(|e| WebError::Storage(format!("Failed to connect to SQLite: {}", e)))?;

        let storage = Self { pool };
        storage.init_tables().await?;

        Ok(storage)
    }

    /// Initialize database tables
    async fn init_tables(&self) -> WebResult<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS query_history (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                query TEXT NOT NULL,
                executed_at TIMESTAMP NOT NULL,
                execution_time_ms INTEGER NOT NULL,
                rows_returned INTEGER NOT NULL,
                success BOOLEAN NOT NULL,
                error_message TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to create history table: {}", e)))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS query_favorites (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                name TEXT NOT NULL,
                query TEXT NOT NULL,
                description TEXT,
                created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to create favorites table: {}", e)))?;

        // Create indexes
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_history_session ON query_history(session_id, executed_at DESC)"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to create history index: {}", e)))?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_favorites_session ON query_favorites(session_id, created_at DESC)"
        )
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to create favorites index: {}", e)))?;

        Ok(())
    }
}

#[async_trait::async_trait]
impl MetadataStorage for SqliteStorage {
    async fn add_history(&self, item: &HistoryItem) -> WebResult<()> {
        sqlx::query(
            r#"
            INSERT INTO query_history (id, session_id, query, executed_at, execution_time_ms, rows_returned, success, error_message)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&item.id)
        .bind(&item.session_id)
        .bind(&item.query)
        .bind(item.executed_at)
        .bind(item.execution_time_ms)
        .bind(item.rows_returned)
        .bind(item.success)
        .bind(&item.error_message)
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to add history: {}", e)))?;

        Ok(())
    }

    async fn get_history(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> WebResult<(Vec<HistoryItem>, i64)> {
        let items: Vec<HistoryItem> = sqlx::query_as(
            r#"
            SELECT id, session_id, query, executed_at, execution_time_ms, rows_returned, success, error_message
            FROM query_history
            WHERE session_id = ?1
            ORDER BY executed_at DESC
            LIMIT ?2 OFFSET ?3
            "#,
        )
        .bind(session_id)
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to get history: {}", e)))?;

        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM query_history WHERE session_id = ?1")
                .bind(session_id)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| WebError::Storage(format!("Failed to count history: {}", e)))?;

        Ok((items, total))
    }

    async fn delete_history(&self, id: &str, session_id: &str) -> WebResult<()> {
        let result = sqlx::query("DELETE FROM query_history WHERE id = ?1 AND session_id = ?2")
            .bind(id)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WebError::Storage(format!("Failed to delete history: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(WebError::NotFound("History item not found".to_string()));
        }

        Ok(())
    }

    async fn clear_history(&self, session_id: &str) -> WebResult<()> {
        sqlx::query("DELETE FROM query_history WHERE session_id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WebError::Storage(format!("Failed to clear history: {}", e)))?;

        Ok(())
    }

    async fn add_favorite(&self, item: &FavoriteItem) -> WebResult<()> {
        sqlx::query(
            r#"
            INSERT INTO query_favorites (id, session_id, name, query, description, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            "#,
        )
        .bind(&item.id)
        .bind(&item.session_id)
        .bind(&item.name)
        .bind(&item.query)
        .bind(&item.description)
        .bind(item.created_at)
        .bind(item.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to add favorite: {}", e)))?;

        Ok(())
    }

    async fn get_favorites(&self, session_id: &str) -> WebResult<Vec<FavoriteItem>> {
        let items: Vec<FavoriteItem> = sqlx::query_as(
            r#"
            SELECT id, session_id, name, query, description, created_at
            FROM query_favorites
            WHERE session_id = ?1
            ORDER BY created_at DESC
            "#,
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to get favorites: {}", e)))?;

        Ok(items)
    }

    async fn get_favorite(&self, id: &str, session_id: &str) -> WebResult<FavoriteItem> {
        let item: FavoriteItem = sqlx::query_as(
            r#"
            SELECT id, session_id, name, query, description, created_at
            FROM query_favorites
            WHERE id = ?1 AND session_id = ?2
            "#,
        )
        .bind(id)
        .bind(session_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => WebError::NotFound("Favorite not found".to_string()),
            _ => WebError::Storage(format!("Failed to get favorite: {}", e)),
        })?;

        Ok(item)
    }

    async fn update_favorite(
        &self,
        id: &str,
        session_id: &str,
        item: &FavoriteItem,
    ) -> WebResult<()> {
        let result = sqlx::query(
            r#"
            UPDATE query_favorites
            SET name = ?1, query = ?2, description = ?3, updated_at = ?4
            WHERE id = ?5 AND session_id = ?6
            "#,
        )
        .bind(&item.name)
        .bind(&item.query)
        .bind(&item.description)
        .bind(Utc::now())
        .bind(id)
        .bind(session_id)
        .execute(&self.pool)
        .await
        .map_err(|e| WebError::Storage(format!("Failed to update favorite: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(WebError::NotFound("Favorite not found".to_string()));
        }

        Ok(())
    }

    async fn delete_favorite(&self, id: &str, session_id: &str) -> WebResult<()> {
        let result = sqlx::query("DELETE FROM query_favorites WHERE id = ?1 AND session_id = ?2")
            .bind(id)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WebError::Storage(format!("Failed to delete favorite: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(WebError::NotFound("Favorite not found".to_string()));
        }

        Ok(())
    }

    async fn delete_all_favorites(&self, session_id: &str) -> WebResult<()> {
        sqlx::query("DELETE FROM query_favorites WHERE session_id = ?1")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .map_err(|e| WebError::Storage(format!("Failed to delete all favorites: {}", e)))?;

        Ok(())
    }
}

// SQLx row mapping for HistoryItem
impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for HistoryItem {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(HistoryItem {
            id: row.try_get("id")?,
            session_id: row.try_get("session_id")?,
            query: row.try_get("query")?,
            executed_at: row.try_get("executed_at")?,
            execution_time_ms: row.try_get("execution_time_ms")?,
            rows_returned: row.try_get("rows_returned")?,
            success: row.try_get("success")?,
            error_message: row.try_get("error_message")?,
        })
    }
}

// SQLx row mapping for FavoriteItem
impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for FavoriteItem {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(FavoriteItem {
            id: row.try_get("id")?,
            session_id: row.try_get("session_id")?,
            name: row.try_get("name")?,
            query: row.try_get("query")?,
            description: row.try_get("description")?,
            created_at: row.try_get("created_at")?,
        })
    }
}
