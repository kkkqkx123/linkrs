//! Outbox snapshots: immutable backup creation, verification and restore.
//! One responsibility of [`SqliteOutbox`](super::SqliteOutbox); the method set is unchanged.

use super::*;

impl SqliteOutbox {
    /// Create an immutable SQLite backup at `destination`.
    ///
    /// `VACUUM INTO` runs against a consistent SQLite snapshot and includes
    /// the WAL contents in the resulting file. The temporary file is synced
    /// before an atomic rename so a crash cannot publish a partial backup.
    pub async fn create_snapshot(
        &self,
        destination: impl AsRef<Path>,
    ) -> Result<OutboxSnapshot, String> {
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = destination.with_extension(format!(
            "tmp-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(|error| error.to_string())?
                .as_nanos()
        ));
        if temporary.exists() {
            std::fs::remove_file(&temporary).map_err(|error| error.to_string())?;
        }
        let destination_sql = temporary.to_string_lossy().replace('\'', "''");
        sqlx::query(&format!("VACUUM INTO '{}'", destination_sql))
            .execute(&self.pool)
            .await
            .map_err(|error| error.to_string())?;
        let bytes = std::fs::read(&temporary).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
        let checksum_path = destination.with_extension("checksum");
        let checksum_temporary = checksum_path.with_extension("checksum.tmp");
        std::fs::write(&checksum_temporary, crc32fast::hash(&bytes).to_string())
            .map_err(|error| error.to_string())?;
        linkrs_core::fs::sync_file(&checksum_temporary).map_err(|error| error.to_string())?;
        std::fs::rename(&checksum_temporary, &checksum_path).map_err(|error| error.to_string())?;
        if let Some(parent) = destination.parent() {
            linkrs_core::fs::sync_dir(parent).map_err(|error| error.to_string())?;
        }
        let materialized_lsn = self.materialized_lsn().await?;
        Ok(OutboxSnapshot {
            path: destination.to_path_buf(),
            size_bytes: bytes.len() as u64,
            checksum: crc32fast::hash(&bytes),
            materialized_lsn,
        })
    }

    pub fn verify_snapshot(snapshot: &OutboxSnapshot) -> Result<(), String> {
        let bytes = std::fs::read(&snapshot.path).map_err(|error| error.to_string())?;
        if bytes.len() as u64 != snapshot.size_bytes {
            return Err("outbox snapshot size mismatch".to_string());
        }
        if crc32fast::hash(&bytes) != snapshot.checksum {
            return Err("outbox snapshot checksum mismatch".to_string());
        }
        Ok(())
    }

    /// Restore a verified snapshot before opening the live database.
    pub fn restore_snapshot(
        snapshot: &OutboxSnapshot,
        destination: impl AsRef<Path>,
    ) -> Result<(), String> {
        Self::verify_snapshot(snapshot)?;
        let destination = destination.as_ref();
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let temporary = destination.with_extension("restore.tmp");
        std::fs::copy(&snapshot.path, &temporary).map_err(|error| error.to_string())?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|error| error.to_string())?;
        }
        std::fs::rename(&temporary, destination).map_err(|error| error.to_string())?;
        if let Some(parent) = destination.parent() {
            linkrs_core::fs::sync_dir(parent).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
