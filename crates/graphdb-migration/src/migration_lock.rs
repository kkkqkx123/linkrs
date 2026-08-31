use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::error::MigrationError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationLockRecord {
    pub lock_id: String,
    pub acquired_at: u64,
    pub expires_at: u64,
    pub holder_id: String,
}

/// In-memory migration lock for process-level coordination.
pub struct MigrationStorageLock {
    lock_key: String,
    store: Arc<RwLock<HashMap<String, MigrationLockRecord>>>,
}

impl std::fmt::Debug for MigrationStorageLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MigrationStorageLock")
            .field("lock_key", &self.lock_key)
            .finish()
    }
}

impl MigrationStorageLock {
    pub fn new(space: &str, label: &str) -> Self {
        Self {
            lock_key: format!("migration_lock:{space}:{label}"),
            store: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn lock_key(&self) -> &str {
        &self.lock_key
    }

    /// Try to acquire lock. Returns None if already held and not expired.
    pub fn try_acquire(
        &self,
        holder_id: &str,
        ttl_secs: u64,
    ) -> Result<Option<MigrationLockRecord>, MigrationError> {
        let now = now_millis();
        let expires = now + ttl_secs * 1000;

        let mut map = self.store.write().map_err(|e| {
            MigrationError::Plan(format!("failed to acquire lock store write guard: {e}"))
        })?;

        if let Some(existing) = map.get(&self.lock_key) {
            if existing.expires_at > now {
                return Ok(None);
            }
        }

        let record = MigrationLockRecord {
            lock_id: generate_id(),
            acquired_at: now,
            expires_at: expires,
            holder_id: holder_id.to_string(),
        };
        map.insert(self.lock_key.clone(), record.clone());
        Ok(Some(record))
    }

    /// Release lock (only if we hold it).
    pub fn release(&self, lock_id: &str) -> Result<(), MigrationError> {
        let mut map = self.store.write().map_err(|e| {
            MigrationError::Plan(format!("failed to acquire lock store write guard: {e}"))
        })?;

        if let Some(existing) = map.get(&self.lock_key) {
            if existing.lock_id == lock_id {
                map.remove(&self.lock_key);
            }
        }
        Ok(())
    }

    /// Renew lock TTL. Returns true if renewed.
    pub fn renew(&self, lock_id: &str, ttl_secs: u64) -> Result<bool, MigrationError> {
        let now = now_millis();
        let mut map = self.store.write().map_err(|e| {
            MigrationError::Plan(format!("failed to acquire lock store write guard: {e}"))
        })?;

        if let Some(existing) = map.get_mut(&self.lock_key) {
            if existing.lock_id == lock_id {
                existing.expires_at = now + ttl_secs * 1000;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Check if lock is currently held and not expired.
    pub fn is_locked(&self) -> Result<bool, MigrationError> {
        let now = now_millis();
        let map = self.store.read().map_err(|e| {
            MigrationError::Plan(format!("failed to acquire lock store read guard: {e}"))
        })?;

        if let Some(rec) = map.get(&self.lock_key) {
            return Ok(rec.expires_at > now);
        }
        Ok(false)
    }
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn generate_id() -> String {
    use std::hash::{Hash, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};

    let t = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    t.as_nanos().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    format!("{:x}-{:x}-{:x}", t.as_secs(), t.subsec_nanos(), hasher.finish() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_and_release() {
        let lock = MigrationStorageLock::new("s", "l");
        let rec = lock.try_acquire("holder1", 30).unwrap().unwrap();
        assert!(lock.try_acquire("holder2", 30).unwrap().is_none());
        lock.release(&rec.lock_id).unwrap();
        assert!(lock.try_acquire("holder2", 30).unwrap().is_some());
    }

    #[test]
    fn test_renew() {
        let lock = MigrationStorageLock::new("s", "l2");
        let rec = lock.try_acquire("holder1", 10).unwrap().unwrap();
        assert!(lock.renew(&rec.lock_id, 100).unwrap());
        assert!(!lock.renew("bad-id", 100).unwrap());
    }

    #[test]
    fn test_expired_lock_can_be_overwritten() {
        let lock = MigrationStorageLock::new("s", "l3");
        let rec = lock.try_acquire("holder1", 0).unwrap().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let rec2 = lock.try_acquire("holder2", 30).unwrap();
        assert!(rec2.is_some());
        assert_ne!(rec.lock_id, rec2.unwrap().lock_id);
    }

    #[test]
    fn test_is_locked() {
        let lock = MigrationStorageLock::new("s", "l4");
        assert!(!lock.is_locked().unwrap());
        let _rec = lock.try_acquire("holder1", 30).unwrap().unwrap();
        assert!(lock.is_locked().unwrap());
    }
}
