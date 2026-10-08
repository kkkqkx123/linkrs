use super::{StorageSchemaContextOps, StorageSchemaOps};

/// Catalog and schema access used by query planning and DDL execution.
pub trait CatalogStore:
    StorageSchemaOps + StorageSchemaContextOps + Send + Sync + std::fmt::Debug
{
}

impl<T> CatalogStore for T where
    T: StorageSchemaOps + StorageSchemaContextOps + Send + Sync + std::fmt::Debug
{
}
