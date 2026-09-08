use super::{StorageAdmin, StorageGcOps, StoragePersistenceOps};

/// Maintenance-only capabilities used by server initialization and administration.
pub trait StorageMaintenance: StorageAdmin + StoragePersistenceOps + StorageGcOps {}
impl<T> StorageMaintenance for T where T: StorageAdmin + StoragePersistenceOps + StorageGcOps {}
