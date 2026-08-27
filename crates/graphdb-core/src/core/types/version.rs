use crate::error::StorageError;
use crate::StorageResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StorageVersion {
    V1 = 1,
}

impl StorageVersion {
    pub const CURRENT: StorageVersion = StorageVersion::V1;
    pub const MIN_SUPPORTED: StorageVersion = StorageVersion::V1;

    pub fn from_u32(v: u32) -> StorageResult<Self> {
        match v {
            1 => Ok(StorageVersion::V1),
            _ => Err(StorageError::unsupported_version(
                v,
                StorageVersion::CURRENT as u32,
            )),
        }
    }

    pub fn as_u32(self) -> u32 {
        self as u32
    }
}
