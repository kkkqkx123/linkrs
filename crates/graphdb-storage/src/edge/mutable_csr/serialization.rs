use graphdb_core::StorageResult;

use crate::persistence::{read_u32_le, read_u64_le};

use super::super::{EdgeId, Nbr};

pub(crate) const MUTABLE_CSR_FORMAT_VERSION: u32 = 2;

pub(crate) fn write_nbr(out: &mut Vec<u8>, nbr: &Nbr) {
    out.extend_from_slice(&nbr.endpoint.to_le_bytes());
    out.extend_from_slice(&nbr.rank.to_le_bytes());
    out.extend_from_slice(&nbr.edge_id.to_le_bytes());
    out.extend_from_slice(&nbr.delete_ts.to_le_bytes());
    out.extend_from_slice(&nbr.prop_offset.to_le_bytes());
}

pub(crate) fn read_nbr(data: &[u8], offset: &mut usize) -> StorageResult<Nbr> {
    let endpoint = read_u32_le(data, offset)?;
    let rank = read_u64_le(data, offset)? as i64;
    let raw_edge_id = read_u64_le(data, offset)?;
    let delete_ts = read_u64_le(data, offset)?;
    let prop_offset = read_u32_le(data, offset)?;
    Ok(Nbr::with_timestamps_and_prop(
        endpoint,
        rank,
        EdgeId(raw_edge_id),
        delete_ts,
        prop_offset,
    ))
}
