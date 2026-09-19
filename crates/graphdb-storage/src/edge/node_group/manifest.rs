//! Per-table group layout manifest: the existing-group record that drives
//! sparse checkpoint and load.

use graphdb_core::{StorageError, StorageResult};

use super::validate_group_bits;

/// Manifest version for the per-table group layout file. Version 5 records
/// existing group ids rather than contiguous counts and admits per-group
/// timestamp, property and segment-statistics shards; older manifests are
/// rejected, never converted.
pub const GROUP_MANIFEST_VERSION: u32 = 5;

/// Per-table group layout shared by both directions.
///
/// Version 4 records existing group ids rather than contiguous counts:
/// sparse endpoints materialize only groups holding rows, missing groups
/// read as empty and never produce files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableShardManifest {
    pub group_bits: u32,
    pub out_groups: Vec<u32>,
    pub in_groups: Vec<u32>,
}

impl TableShardManifest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out_groups = self.out_groups.clone();
        out_groups.sort_unstable();
        out_groups.dedup();
        let mut in_groups = self.in_groups.clone();
        in_groups.sort_unstable();
        in_groups.dedup();
        let mut out = Vec::with_capacity(16 + (out_groups.len() + in_groups.len()) * 4);
        out.extend_from_slice(&GROUP_MANIFEST_VERSION.to_le_bytes());
        out.extend_from_slice(&self.group_bits.to_le_bytes());
        out.extend_from_slice(&(out_groups.len() as u32).to_le_bytes());
        for gid in &out_groups {
            out.extend_from_slice(&gid.to_le_bytes());
        }
        out.extend_from_slice(&(in_groups.len() as u32).to_le_bytes());
        for gid in &in_groups {
            out.extend_from_slice(&gid.to_le_bytes());
        }
        out
    }

    pub fn decode(data: &[u8]) -> StorageResult<Self> {
        let bad_slice = || StorageError::deserialize_error("group manifest slice too short");
        if data.len() < 12 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest too short: got {}",
                data.len()
            )));
        }
        let version = u32::from_le_bytes(data[0..4].try_into().map_err(|_| bad_slice())?);
        if version != GROUP_MANIFEST_VERSION {
            return Err(StorageError::deserialize_error(format!(
                "unsupported group manifest version: {}",
                version
            )));
        }
        let group_bits = u32::from_le_bytes(data[4..8].try_into().map_err(|_| bad_slice())?);
        validate_group_bits(group_bits)?;
        let mut cursor = 8usize;
        let take_u32 = |data: &[u8], cursor: &mut usize| -> StorageResult<u32> {
            if data.len() - *cursor < 4 {
                return Err(StorageError::deserialize_error(
                    "group manifest slice too short",
                ));
            }
            let value = u32::from_le_bytes(
                data[*cursor..*cursor + 4]
                    .try_into()
                    .map_err(|_| bad_slice())?,
            );
            *cursor += 4;
            Ok(value)
        };
        let out_len = take_u32(data, &mut cursor)? as usize;
        if data.len() - cursor < out_len * 4 + 4 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest too short for {} out groups",
                out_len
            )));
        }
        let mut out_groups = Vec::with_capacity(out_len);
        for _ in 0..out_len {
            out_groups.push(take_u32(data, &mut cursor)?);
        }
        let in_len = take_u32(data, &mut cursor)? as usize;
        if data.len() - cursor != in_len * 4 {
            return Err(StorageError::deserialize_error(format!(
                "group manifest trailing bytes: expected {} in groups, {} bytes remain",
                in_len,
                data.len() - cursor
            )));
        }
        let mut in_groups = Vec::with_capacity(in_len);
        for _ in 0..in_len {
            in_groups.push(take_u32(data, &mut cursor)?);
        }
        out_groups.sort_unstable();
        out_groups.dedup();
        in_groups.sort_unstable();
        in_groups.dedup();
        Ok(Self {
            group_bits,
            out_groups,
            in_groups,
        })
    }
}
