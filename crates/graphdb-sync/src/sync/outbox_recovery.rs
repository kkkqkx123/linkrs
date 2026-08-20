//! Outbox Recovery
//!
//! Implements recovery when the live SQLite outbox is lost, corrupted,
//! or rolled back, rebuild it from the most recent outbox snapshot and replay
//! the remaining committed WAL intents.

use std::path::{Path, PathBuf};

use graphdb_core::core::types::CommitLsn;

use crate::sync::sqlite_outbox::OutboxSnapshot;

/// Check if the live SQLite database file exists and is accessible.
pub fn live_database_exists(path: &Path) -> bool {
    path.is_file()
}

/// Attempt to verify the live SQLite database by opening it read-only.
///
/// Returns true if the database is accessible and structurally valid.
pub async fn verify_live_database(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    let url = format!("sqlite://{}?mode=ro", path.display());
    match sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect_lazy(&url)
    {
        Ok(pool) => match sqlx::query("SELECT 1").execute(&pool).await {
            Ok(_) => {
                pool.close().await;
                true
            }
            Err(_) => false,
        },
        Err(_) => false,
    }
}

/// Find the most recent valid outbox snapshot in the snapshot directory.
///
/// Snapshot files are named `outbox_snapshot_<lsn>.sqlite` and have an
/// accompanying `.checksum` file.
pub fn find_latest_snapshot(snapshot_dir: &Path) -> Option<OutboxSnapshot> {
    find_latest_snapshot_at_or_before(snapshot_dir, u64::MAX)
}

/// Find the most recent valid outbox snapshot whose materialized LSN does not
/// exceed the supplied recovery boundary.
///
/// A snapshot newer than the storage checkpoint cannot be part of the same
/// consistent checkpoint, so callers creating a combined manifest must use
/// this bounded form instead of selecting the directory-wide newest file.
pub fn find_latest_snapshot_at_or_before(
    snapshot_dir: &Path,
    max_lsn: u64,
) -> Option<OutboxSnapshot> {
    if !snapshot_dir.is_dir() {
        return None;
    }

    let mut snapshots: Vec<(u64, PathBuf)> = std::fs::read_dir(snapshot_dir)
        .ok()?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let lsn = name
                .strip_prefix("outbox_snapshot_")?
                .strip_suffix(".sqlite")?
                .parse::<u64>()
                .ok()?;
            if lsn > max_lsn {
                return None;
            }
            Some((lsn, path))
        })
        .collect();

    snapshots.sort_by_key(|(lsn, _)| std::cmp::Reverse(*lsn));

    for (lsn, path) in snapshots {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };

        let checksum_path = path.with_extension("checksum");
        let checksum = std::fs::read_to_string(&checksum_path)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok());

        if checksum.is_none() {
            continue;
        }

        let computed = crc32fast::hash(&bytes);
        if Some(computed) != checksum {
            log::warn!(
                "Outbox snapshot {} checksum mismatch, skipping",
                path.display()
            );
            continue;
        }

        return Some(OutboxSnapshot {
            path,
            size_bytes: bytes.len() as u64,
            checksum: computed,
            materialized_lsn: CommitLsn::new(lsn),
        });
    }

    None
}

/// Attempt to recover the live outbox database from the latest available snapshot.
///
/// This is the top-level recovery entry point. It checks whether the live database
/// is healthy; if not, it finds and restores the latest valid snapshot, then returns
/// the snapshot LSN so the caller can replay remaining committed WAL intents.
///
/// Returns `Ok(Some(snapshot_lsn))` if recovery was performed, `Ok(None)` if the
/// live database was already healthy, or an error if recovery failed.
pub fn recover_outbox(live_path: &Path, snapshot_dir: &Path) -> Result<Option<CommitLsn>, String> {
    if live_database_exists(live_path) {
        return Ok(None);
    }

    log::warn!(
        "Live outbox database {} not found; attempting recovery from snapshots",
        live_path.display()
    );

    let restored_lsn = restore_latest_snapshot(live_path, snapshot_dir)?;
    Ok(Some(restored_lsn))
}

/// Restore the most recent valid snapshot regardless of whether a live
/// database currently exists. Callers use this after detecting corruption in
/// an existing SQLite file.
pub fn restore_latest_snapshot(live_path: &Path, snapshot_dir: &Path) -> Result<CommitLsn, String> {
    let snapshot = find_latest_snapshot(snapshot_dir).ok_or_else(|| {
        format!(
            "No valid outbox snapshot found in {}",
            snapshot_dir.display()
        )
    })?;

    let restored_lsn = snapshot.materialized_lsn;
    log::info!(
        "Restoring outbox snapshot at LSN {} from {}",
        restored_lsn.get(),
        snapshot.path.display()
    );

    restore_snapshot_sync(&snapshot, live_path)?;

    log::info!("Outbox recovery completed at LSN {}", restored_lsn.get());
    Ok(restored_lsn)
}

/// Restore an outbox snapshot to the live database path synchronously.
///
/// This is a sync version of `SqliteOutbox::restore_snapshot` for use
/// from contexts that cannot use async (e.g., storage recovery).
pub fn restore_snapshot_sync(snapshot: &OutboxSnapshot, destination: &Path) -> Result<(), String> {
    // Verify snapshot first
    let bytes = std::fs::read(&snapshot.path).map_err(|error| error.to_string())?;
    if bytes.len() as u64 != snapshot.size_bytes {
        return Err("outbox snapshot size mismatch".to_string());
    }
    if crc32fast::hash(&bytes) != snapshot.checksum {
        return Err("outbox snapshot checksum mismatch".to_string());
    }

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
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_find_latest_snapshot_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        assert!(find_latest_snapshot(temp_dir.path()).is_none());
    }

    #[test]
    fn test_find_latest_snapshot_finds_valid() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot_dir = temp_dir.path().join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).unwrap();

        // Create a valid snapshot
        let snapshot_path = snapshot_dir.join("outbox_snapshot_100.sqlite");
        let data = b"test snapshot data";
        std::fs::write(&snapshot_path, data).unwrap();
        let checksum = crc32fast::hash(data);
        std::fs::write(
            snapshot_path.with_extension("checksum"),
            checksum.to_string(),
        )
        .unwrap();

        let snapshot = find_latest_snapshot(&snapshot_dir);
        assert!(snapshot.is_some());
        let snapshot = snapshot.unwrap();
        assert_eq!(snapshot.materialized_lsn, CommitLsn::new(100));
        assert_eq!(snapshot.checksum, checksum);
    }

    #[test]
    fn test_find_latest_snapshot_skips_corrupted() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot_dir = temp_dir.path().join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).unwrap();

        // Create a corrupted snapshot (checksum mismatch)
        let snapshot_path = snapshot_dir.join("outbox_snapshot_100.sqlite");
        std::fs::write(&snapshot_path, b"corrupted data").unwrap();
        std::fs::write(snapshot_path.with_extension("checksum"), "99999").unwrap();

        assert!(find_latest_snapshot(&snapshot_dir).is_none());
    }

    #[test]
    fn test_find_latest_snapshot_returns_highest_lsn() {
        let temp_dir = TempDir::new().unwrap();
        let snapshot_dir = temp_dir.path().join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).unwrap();

        // Create multiple snapshots
        for lsn in [100, 200, 50] {
            let path = snapshot_dir.join(format!("outbox_snapshot_{}.sqlite", lsn));
            let data = format!("snapshot-{}", lsn);
            std::fs::write(&path, data.as_bytes()).unwrap();
            let checksum = crc32fast::hash(data.as_bytes());
            std::fs::write(path.with_extension("checksum"), checksum.to_string()).unwrap();
        }

        let snapshot = find_latest_snapshot(&snapshot_dir).unwrap();
        assert_eq!(snapshot.materialized_lsn, CommitLsn::new(200));
        let bounded = find_latest_snapshot_at_or_before(&snapshot_dir, 100).unwrap();
        assert_eq!(bounded.materialized_lsn, CommitLsn::new(100));
    }

    #[test]
    fn test_restore_snapshot_sync() {
        use tempfile::TempDir;
        let temp_dir = TempDir::new().unwrap();
        let snapshot_dir = temp_dir.path().join("snapshots");
        std::fs::create_dir_all(&snapshot_dir).unwrap();

        let snapshot_path = snapshot_dir.join("outbox_snapshot_100.sqlite");
        let data = b"test snapshot data";
        std::fs::write(&snapshot_path, data).unwrap();
        let checksum = crc32fast::hash(data);
        std::fs::write(
            snapshot_path.with_extension("checksum"),
            checksum.to_string(),
        )
        .unwrap();

        let snapshot = find_latest_snapshot(&snapshot_dir).unwrap();
        let dest = temp_dir.path().join("restored.sqlite");
        restore_snapshot_sync(&snapshot, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }
}
