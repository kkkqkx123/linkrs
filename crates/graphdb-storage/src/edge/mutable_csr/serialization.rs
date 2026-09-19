use graphdb_core::{StorageError, StorageResult};

use crate::persistence::read_u32_le;

use super::super::{EdgeId, Nbr};
use super::overflow::OverflowChunk;

pub(crate) const MUTABLE_CSR_FORMAT_VERSION: u32 = 6;
/// Direct-dump format version: topology columns stored at native widths
/// with no encoding choice. Selected by configuration for speed-sensitive
/// checkpoints; the loader accepts both versions by marker and rejects any
/// other marker instead of converting.
///
/// Both markers cover payloads with a trailing CRC32 trailer verified on
/// load; versions 4/5 without the trailer are rejected by marker.
pub(crate) const MUTABLE_CSR_FORMAT_RAW_VERSION: u32 = 7;

/// Integer-only column encoding for topology persistence.
///
/// Neighbor, edge-id, offset and length columns carry small integers, so
/// only bit-packing and run-length encodings apply here. String and float
/// encodings stay with attribute columns and are never introduced for
/// topology.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopologyColumnEncoding {
    Plain = 0,
    BitPacked = 1,
    Rle = 2,
}

impl TopologyColumnEncoding {
    fn from_u8(value: u8) -> StorageResult<Self> {
        match value {
            0 => Ok(Self::Plain),
            1 => Ok(Self::BitPacked),
            2 => Ok(Self::Rle),
            other => Err(StorageError::deserialize_error(format!(
                "unknown topology column encoding: {}",
                other
            ))),
        }
    }
}

/// Report for one encoded topology column: which encoding won the size
/// comparison and how many bytes each candidate needed. Correctness comes
/// first: when no encoding beats plain, plain wins and no ineffective
/// encoding branch is retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TopologyEncodingChoice {
    pub encoding: TopologyColumnEncoding,
    pub plain_bytes: usize,
    pub encoded_bytes: usize,
}

fn bit_width_for_range(range: u64) -> u8 {
    if range == 0 {
        1
    } else {
        (64 - range.leading_zeros()) as u8
    }
}

fn rle_runs(values: &[u64]) -> Vec<(u64, u32)> {
    let mut runs: Vec<(u64, u32)> = Vec::new();
    for &value in values {
        match runs.last_mut() {
            Some(last) if last.0 == value => {
                last.1 = last.1.saturating_add(1);
            }
            _ => runs.push((value, 1)),
        }
    }
    runs
}

fn bitpacked_payload(values: &[u64]) -> Option<Vec<u8>> {
    if values.is_empty() {
        return Some(Vec::new());
    }
    let min = *values.iter().min()?;
    let max = *values.iter().max()?;
    let width = bit_width_for_range(max.saturating_sub(min)).max(1) as usize;
    if width >= 64 {
        return None;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&min.to_le_bytes());
    out.push(width as u8);
    let mut current: u64 = 0;
    let mut filled: usize = 0;
    for &value in values {
        let adjusted = value.saturating_sub(min);
        for bit in 0..width {
            if (adjusted >> bit) & 1 == 1 {
                current |= 1u64 << filled;
            }
            filled += 1;
            if filled == 64 {
                out.extend_from_slice(&current.to_le_bytes());
                current = 0;
                filled = 0;
            }
        }
    }
    if filled > 0 {
        out.extend_from_slice(&current.to_le_bytes());
    }
    Some(out)
}

fn decode_bitpacked_payload(payload: &[u8], count: usize) -> StorageResult<Vec<u64>> {
    if count == 0 {
        if !payload.is_empty() {
            return Err(StorageError::deserialize_error(
                "non-empty bitpacked payload for empty topology column",
            ));
        }
        return Ok(Vec::new());
    }
    if payload.len() < 9 {
        return Err(StorageError::deserialize_error(
            "bitpacked topology column too short",
        ));
    }
    let min = u64::from_le_bytes(payload[0..8].try_into().map_err(|_| {
        StorageError::deserialize_error("bitpacked topology column base too short")
    })?);
    let width = payload[8] as usize;
    if width == 0 || width >= 64 {
        return Err(StorageError::deserialize_error(format!(
            "invalid bitpacked topology width: {}",
            width
        )));
    }
    let words: Vec<u64> = payload[9..]
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap_or([0u8; 8])))
        .collect();
    if payload[9..].len() != words.len() * 8 {
        return Err(StorageError::deserialize_error(
            "bitpacked topology payload has trailing bytes",
        ));
    }
    let mut out = Vec::with_capacity(count);
    let mut bit_pos: usize = 0;
    for _ in 0..count {
        let mut adjusted: u64 = 0;
        for bit in 0..width {
            let word = bit_pos / 64;
            let offset = bit_pos % 64;
            let stored = words.get(word).copied().unwrap_or(0);
            if (stored >> offset) & 1 == 1 {
                adjusted |= 1u64 << bit;
            }
            bit_pos += 1;
        }
        out.push(min.saturating_add(adjusted));
    }
    let expected_words = bit_pos.div_ceil(64);
    if words.len() != expected_words {
        return Err(StorageError::deserialize_error(
            "bitpacked topology payload length mismatch",
        ));
    }
    Ok(out)
}

/// Encode one integer topology column, keeping the smallest of plain,
/// bit-packed and run-length forms. Falls back to plain when compression
/// does not pay, so no ineffective encoding branch is persisted.
pub fn encode_topology_u64_column(values: &[u64]) -> (TopologyEncodingChoice, Vec<u8>) {
    let plain_bytes = values.len().saturating_mul(8);
    let mut best_encoding = TopologyColumnEncoding::Plain;
    let mut best_payload = Vec::with_capacity(plain_bytes);
    for &value in values {
        best_payload.extend_from_slice(&value.to_le_bytes());
    }
    let mut best_bytes = plain_bytes;

    if let Some(packed) = bitpacked_payload(values) {
        if packed.len() < best_bytes {
            best_bytes = packed.len();
            best_encoding = TopologyColumnEncoding::BitPacked;
            best_payload = packed;
        }
    }

    if !values.is_empty() {
        let runs = rle_runs(values);
        let rle_bytes = runs.len().saturating_mul(12);
        if rle_bytes < best_bytes {
            let mut payload = Vec::with_capacity(rle_bytes);
            for (value, count) in &runs {
                payload.extend_from_slice(&value.to_le_bytes());
                payload.extend_from_slice(&count.to_le_bytes());
            }
            best_bytes = rle_bytes;
            best_encoding = TopologyColumnEncoding::Rle;
            best_payload = payload;
        }
    }

    let mut out = Vec::with_capacity(5 + best_payload.len());
    out.push(best_encoding as u8);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&best_payload);
    let choice = TopologyEncodingChoice {
        encoding: best_encoding,
        plain_bytes,
        encoded_bytes: best_bytes,
    };
    (choice, out)
}

/// Decode one integer topology column, failing closed on count or trailing
/// mismatches.
pub fn decode_topology_u64_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
    if data.len().saturating_sub(*offset) < 5 {
        return Err(StorageError::deserialize_error(
            "topology column too short for header",
        ));
    }
    let encoding = TopologyColumnEncoding::from_u8(data[*offset])?;
    *offset += 1;
    let count = u32::from_le_bytes(
        data[*offset..*offset + 4]
            .try_into()
            .map_err(|_| StorageError::deserialize_error("topology column count too short"))?,
    ) as usize;
    *offset += 4;
    match encoding {
        TopologyColumnEncoding::Plain => {
            let need = count.saturating_mul(8);
            if data.len().saturating_sub(*offset) < need {
                return Err(StorageError::deserialize_error(
                    "plain topology column too short",
                ));
            }
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                let value = u64::from_le_bytes(
                    data[*offset..*offset + 8]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("plain value too short"))?,
                );
                *offset += 8;
                out.push(value);
            }
            Ok(out)
        }
        TopologyColumnEncoding::BitPacked => {
            let remaining = &data[*offset..];
            let min_width_len = if count == 0 { 0 } else { 9 };
            if remaining.len() < min_width_len {
                return Err(StorageError::deserialize_error(
                    "bitpacked topology column too short",
                ));
            }
            let width = if count == 0 { 1 } else { remaining[8] as usize };
            let payload_words = if count == 0 {
                0
            } else {
                (count.saturating_mul(width)).div_ceil(64)
            };
            let payload_len = min_width_len + payload_words.saturating_mul(8);
            if remaining.len() < payload_len {
                return Err(StorageError::deserialize_error(
                    "bitpacked topology column too short",
                ));
            }
            let values = decode_bitpacked_payload(&remaining[..payload_len], count)?;
            *offset += payload_len;
            Ok(values)
        }
        TopologyColumnEncoding::Rle => {
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                if data.len().saturating_sub(*offset) < 12 {
                    return Err(StorageError::deserialize_error(
                        "run-length topology column too short",
                    ));
                }
                let value = u64::from_le_bytes(
                    data[*offset..*offset + 8]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("run value too short"))?,
                );
                *offset += 8;
                let run = u32::from_le_bytes(
                    data[*offset..*offset + 4]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("run length too short"))?,
                ) as usize;
                *offset += 4;
                if run == 0 {
                    return Err(StorageError::deserialize_error(
                        "run-length topology run has zero length",
                    ));
                }
                if out.len().saturating_add(run) > count {
                    return Err(StorageError::deserialize_error(
                        "run-length topology column overruns declared count",
                    ));
                }
                out.extend(std::iter::repeat_n(value, run));
            }
            Ok(out)
        }
    }
}

fn rle_runs_u32(values: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &value in values {
        match runs.last_mut() {
            Some(last) if last.0 == value => {
                last.1 = last.1.saturating_add(1);
            }
            _ => runs.push((value, 1)),
        }
    }
    runs
}

fn bitpacked_payload_u32(values: &[u32]) -> Option<Vec<u8>> {
    if values.is_empty() {
        return Some(Vec::new());
    }
    let min = *values.iter().min()?;
    let max = *values.iter().max()?;
    let range = (max as u64).saturating_sub(min as u64);
    let width = bit_width_for_range(range).max(1) as usize;
    if width >= 32 {
        return None;
    }
    let mut out = Vec::new();
    out.extend_from_slice(&min.to_le_bytes());
    out.push(width as u8);
    let mut current: u64 = 0;
    let mut filled: usize = 0;
    for &value in values {
        let adjusted = (value as u64).saturating_sub(min as u64);
        for bit in 0..width {
            if (adjusted >> bit) & 1 == 1 {
                current |= 1u64 << filled;
            }
            filled += 1;
            if filled == 64 {
                out.extend_from_slice(&current.to_le_bytes());
                current = 0;
                filled = 0;
            }
        }
    }
    if filled > 0 {
        out.extend_from_slice(&current.to_le_bytes());
    }
    Some(out)
}

fn decode_bitpacked_payload_u32(payload: &[u8], count: usize) -> StorageResult<Vec<u32>> {
    if count == 0 {
        if !payload.is_empty() {
            return Err(StorageError::deserialize_error(
                "non-empty bitpacked payload for empty topology column",
            ));
        }
        return Ok(Vec::new());
    }
    if payload.len() < 5 {
        return Err(StorageError::deserialize_error(
            "bitpacked topology column too short",
        ));
    }
    let min = u32::from_le_bytes(payload[0..4].try_into().map_err(|_| {
        StorageError::deserialize_error("bitpacked topology column base too short")
    })?);
    let width = payload[4] as usize;
    if width == 0 || width >= 32 {
        return Err(StorageError::deserialize_error(format!(
            "invalid bitpacked topology width: {}",
            width
        )));
    }
    let words: Vec<u64> = payload[5..]
        .chunks_exact(8)
        .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap_or([0u8; 8])))
        .collect();
    if payload[5..].len() != words.len() * 8 {
        return Err(StorageError::deserialize_error(
            "bitpacked topology payload has trailing bytes",
        ));
    }
    let mut out = Vec::with_capacity(count);
    let mut bit_pos: usize = 0;
    for _ in 0..count {
        let mut adjusted: u64 = 0;
        for bit in 0..width {
            let word = bit_pos / 64;
            let offset = bit_pos % 64;
            let stored = words.get(word).copied().unwrap_or(0);
            if (stored >> offset) & 1 == 1 {
                adjusted |= 1u64 << bit;
            }
            bit_pos += 1;
        }
        out.push((min as u64).saturating_add(adjusted) as u32);
    }
    let expected_words = bit_pos.div_ceil(64);
    if words.len() != expected_words {
        return Err(StorageError::deserialize_error(
            "bitpacked topology payload length mismatch",
        ));
    }
    Ok(out)
}

/// Encode one u32 topology column at its native width without widening.
///
/// Plain fallback stores four bytes per value, never eight, so narrow
/// columns never inflate on the fallback path. Bit-packing and run-length
/// encodings operate directly on the u32 slice without an intermediate
/// widened copy, keeping peak memory to one column plus its winning payload.
pub fn encode_topology_u32_column(values: &[u32]) -> (TopologyEncodingChoice, Vec<u8>) {
    let plain_bytes = values.len().saturating_mul(4);
    let mut best_encoding = TopologyColumnEncoding::Plain;
    let mut best_payload = Vec::with_capacity(plain_bytes);
    for &value in values {
        best_payload.extend_from_slice(&value.to_le_bytes());
    }
    let mut best_bytes = plain_bytes;

    if let Some(packed) = bitpacked_payload_u32(values) {
        if packed.len() < best_bytes {
            best_bytes = packed.len();
            best_encoding = TopologyColumnEncoding::BitPacked;
            best_payload = packed;
        }
    }

    if !values.is_empty() {
        let runs = rle_runs_u32(values);
        let rle_bytes = runs.len().saturating_mul(8);
        if rle_bytes < best_bytes {
            let mut payload = Vec::with_capacity(rle_bytes);
            for (value, count) in &runs {
                payload.extend_from_slice(&value.to_le_bytes());
                payload.extend_from_slice(&count.to_le_bytes());
            }
            best_bytes = rle_bytes;
            best_encoding = TopologyColumnEncoding::Rle;
            best_payload = payload;
        }
    }

    let mut out = Vec::with_capacity(5 + best_payload.len());
    out.push(best_encoding as u8);
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    out.extend_from_slice(&best_payload);
    let choice = TopologyEncodingChoice {
        encoding: best_encoding,
        plain_bytes,
        encoded_bytes: best_bytes,
    };
    (choice, out)
}

/// Decode one u32 topology column written by `encode_topology_u32_column`.
/// Version 3 payloads widened through the u64 path and are rejected by the
/// outer format version, never converted here.
pub fn decode_topology_u32_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u32>> {
    if data.len().saturating_sub(*offset) < 5 {
        return Err(StorageError::deserialize_error(
            "topology column too short for header",
        ));
    }
    let encoding = TopologyColumnEncoding::from_u8(data[*offset])?;
    *offset += 1;
    let count = u32::from_le_bytes(
        data[*offset..*offset + 4]
            .try_into()
            .map_err(|_| StorageError::deserialize_error("topology column count too short"))?,
    ) as usize;
    *offset += 4;
    match encoding {
        TopologyColumnEncoding::Plain => {
            let need = count.saturating_mul(4);
            if data.len().saturating_sub(*offset) < need {
                return Err(StorageError::deserialize_error(
                    "plain topology column too short",
                ));
            }
            let mut out = Vec::with_capacity(count);
            for _ in 0..count {
                let value = u32::from_le_bytes(
                    data[*offset..*offset + 4]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("plain value too short"))?,
                );
                *offset += 4;
                out.push(value);
            }
            Ok(out)
        }
        TopologyColumnEncoding::BitPacked => {
            let remaining = &data[*offset..];
            let min_width_len = if count == 0 { 0 } else { 5 };
            if remaining.len() < min_width_len {
                return Err(StorageError::deserialize_error(
                    "bitpacked topology column too short",
                ));
            }
            let width = if count == 0 { 1 } else { remaining[4] as usize };
            let payload_words = if count == 0 {
                0
            } else {
                (count.saturating_mul(width)).div_ceil(64)
            };
            let payload_len = min_width_len + payload_words.saturating_mul(8);
            if remaining.len() < payload_len {
                return Err(StorageError::deserialize_error(
                    "bitpacked topology column too short",
                ));
            }
            let values = decode_bitpacked_payload_u32(&remaining[..payload_len], count)?;
            *offset += payload_len;
            Ok(values)
        }
        TopologyColumnEncoding::Rle => {
            let mut out = Vec::with_capacity(count);
            while out.len() < count {
                if data.len().saturating_sub(*offset) < 8 {
                    return Err(StorageError::deserialize_error(
                        "run-length topology column too short",
                    ));
                }
                let value = u32::from_le_bytes(
                    data[*offset..*offset + 4]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("run value too short"))?,
                );
                *offset += 4;
                let run = u32::from_le_bytes(
                    data[*offset..*offset + 4]
                        .try_into()
                        .map_err(|_| StorageError::deserialize_error("run length too short"))?,
                ) as usize;
                *offset += 4;
                if run == 0 {
                    return Err(StorageError::deserialize_error(
                        "run-length topology run has zero length",
                    ));
                }
                if out.len().saturating_add(run) > count {
                    return Err(StorageError::deserialize_error(
                        "run-length topology column overruns declared count",
                    ));
                }
                out.extend(std::iter::repeat_n(value, run));
            }
            Ok(out)
        }
    }
}

pub fn encode_topology_i64_column(values: &[i64]) -> (TopologyEncodingChoice, Vec<u8>) {
    let wide: Vec<u64> = values.iter().map(|&v| v as u64).collect();
    encode_topology_u64_column(&wide)
}

pub fn decode_topology_i64_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<i64>> {
    let wide = decode_topology_u64_column(data, offset)?;
    Ok(wide.into_iter().map(|v| v as i64).collect())
}

/// Raw column framing: value count as u32 followed by native-width
/// little-endian values with no encoding choice. The direct-dump mode
/// writes and reads every topology column in this framing so checkpoints
/// skip the per-column encode passes entirely.
pub fn write_raw_u32_column(values: &[u32], out: &mut Vec<u8>) {
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for &value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Decode one raw u32 column, failing closed on truncation.
pub fn decode_raw_u32_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u32>> {
    let count = read_u32_le(data, offset)? as usize;
    let need = count.saturating_mul(4);
    if data.len().saturating_sub(*offset) < need {
        return Err(StorageError::deserialize_error(
            "raw u32 topology column too short",
        ));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut buf = [0u8; 4];
        buf.copy_from_slice(&data[*offset..*offset + 4]);
        *offset += 4;
        out.push(u32::from_le_bytes(buf));
    }
    Ok(out)
}

/// Write one i64 column at native width, bits preserved exactly.
pub fn write_raw_i64_column(values: &[i64], out: &mut Vec<u8>) {
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for &value in values {
        out.extend_from_slice(&(value as u64).to_le_bytes());
    }
}

/// Decode one raw i64 column, failing closed on truncation.
pub fn decode_raw_i64_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<i64>> {
    let wide = decode_raw_u64_column(data, offset)?;
    Ok(wide.into_iter().map(|v| v as i64).collect())
}

/// Write one u64 column at native width.
pub fn write_raw_u64_column(values: &[u64], out: &mut Vec<u8>) {
    out.extend_from_slice(&(values.len() as u32).to_le_bytes());
    for &value in values {
        out.extend_from_slice(&value.to_le_bytes());
    }
}

/// Decode one raw u64 column, failing closed on truncation.
pub fn decode_raw_u64_column(data: &[u8], offset: &mut usize) -> StorageResult<Vec<u64>> {
    let count = read_u32_le(data, offset)? as usize;
    let need = count.saturating_mul(8);
    if data.len().saturating_sub(*offset) < need {
        return Err(StorageError::deserialize_error(
            "raw u64 topology column too short",
        ));
    }
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&data[*offset..*offset + 8]);
        *offset += 8;
        out.push(u64::from_le_bytes(buf));
    }
    Ok(out)
}

/// Encode one overflow chunk through the narrow integer column path.
///
/// Each of the five neighbor fields goes through its native-width column
/// encoder, so overflow storage benefits from the same bit-packing and
/// run-length encodings as the primary area instead of staying plain.
/// The chunk halves are gathered into the existing column buffers, so the
/// on-disk bytes match the pre-split layout exactly.
pub fn encode_overflow_chunk(chunk: &OverflowChunk, out: &mut Vec<u8>) {
    out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    if chunk.is_empty() {
        return;
    }
    let endpoints: Vec<u32> = chunk.hot_slice().iter().map(|hot| hot.endpoint).collect();
    let ranks: Vec<i64> = chunk.hot_slice().iter().map(|hot| hot.rank).collect();
    let edge_ids: Vec<u64> = chunk.hot_slice().iter().map(|hot| hot.edge_id.0).collect();
    let creates: Vec<u64> = chunk
        .cold_slice()
        .iter()
        .map(|cold| cold.create_ts)
        .collect();
    let deletes: Vec<u64> = chunk
        .cold_slice()
        .iter()
        .map(|cold| cold.delete_ts)
        .collect();
    let (_, payload) = encode_topology_u32_column(&endpoints);
    out.extend_from_slice(&payload);
    let (_, payload) = encode_topology_i64_column(&ranks);
    out.extend_from_slice(&payload);
    let (_, payload) = encode_topology_u64_column(&edge_ids);
    out.extend_from_slice(&payload);
    let (_, payload) = encode_topology_u64_column(&creates);
    out.extend_from_slice(&payload);
    let (_, payload) = encode_topology_u64_column(&deletes);
    out.extend_from_slice(&payload);
}

/// Decode one overflow chunk written by `encode_overflow_chunk`.
/// Fails closed when column lengths disagree or bytes trail.
pub fn decode_overflow_chunk(data: &[u8], offset: &mut usize) -> StorageResult<OverflowChunk> {
    let chunk_len = read_u32_le(data, offset)? as usize;
    if chunk_len == 0 {
        return Ok(OverflowChunk::default());
    }
    let endpoints = decode_topology_u32_column(data, offset)?;
    let ranks = decode_topology_i64_column(data, offset)?;
    let edge_ids = decode_topology_u64_column(data, offset)?;
    let creates = decode_topology_u64_column(data, offset)?;
    let deletes = decode_topology_u64_column(data, offset)?;
    if endpoints.len() != chunk_len
        || ranks.len() != chunk_len
        || edge_ids.len() != chunk_len
        || creates.len() != chunk_len
        || deletes.len() != chunk_len
    {
        return Err(StorageError::deserialize_error(
            "overflow chunk column length mismatch",
        ));
    }
    let mut out = OverflowChunk::with_capacity(chunk_len);
    for index in 0..chunk_len {
        let mut nbr = Nbr::with_timestamps(
            endpoints[index],
            ranks[index],
            EdgeId(edge_ids[index]),
            deletes[index],
        );
        nbr.create_ts = creates[index];
        out.push(nbr);
    }
    Ok(out)
}

/// Write one overflow chunk with every neighbor field at native width.
///
/// Same field order as the encoded form (endpoints, ranks, edge ids,
/// create stamps, delete stamps), each column in raw framing. Written
/// straight from the chunk slices with no gather buffers.
pub fn write_raw_overflow_chunk(chunk: &OverflowChunk, out: &mut Vec<u8>) {
    let hot = chunk.hot_slice();
    let cold = chunk.cold_slice();
    out.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
    out.extend_from_slice(&(hot.len() as u32).to_le_bytes());
    for h in hot {
        out.extend_from_slice(&h.endpoint.to_le_bytes());
    }
    out.extend_from_slice(&(hot.len() as u32).to_le_bytes());
    for h in hot {
        out.extend_from_slice(&(h.rank as u64).to_le_bytes());
    }
    out.extend_from_slice(&(hot.len() as u32).to_le_bytes());
    for h in hot {
        out.extend_from_slice(&h.edge_id.0.to_le_bytes());
    }
    out.extend_from_slice(&(cold.len() as u32).to_le_bytes());
    for c in cold {
        out.extend_from_slice(&c.create_ts.to_le_bytes());
    }
    out.extend_from_slice(&(cold.len() as u32).to_le_bytes());
    for c in cold {
        out.extend_from_slice(&c.delete_ts.to_le_bytes());
    }
}

/// Decode one overflow chunk written by `write_raw_overflow_chunk`.
/// Fails closed when column lengths disagree with the chunk length.
pub fn decode_raw_overflow_chunk(data: &[u8], offset: &mut usize) -> StorageResult<OverflowChunk> {
    let chunk_len = read_u32_le(data, offset)? as usize;
    if chunk_len == 0 {
        return Ok(OverflowChunk::default());
    }
    let endpoints = decode_raw_u32_column(data, offset)?;
    let ranks = decode_raw_i64_column(data, offset)?;
    let edge_ids = decode_raw_u64_column(data, offset)?;
    let creates = decode_raw_u64_column(data, offset)?;
    let deletes = decode_raw_u64_column(data, offset)?;
    if endpoints.len() != chunk_len
        || ranks.len() != chunk_len
        || edge_ids.len() != chunk_len
        || creates.len() != chunk_len
        || deletes.len() != chunk_len
    {
        return Err(StorageError::deserialize_error(
            "raw overflow chunk column length mismatch",
        ));
    }
    let mut out = OverflowChunk::with_capacity(chunk_len);
    for index in 0..chunk_len {
        let mut nbr = Nbr::with_timestamps(
            endpoints[index],
            ranks[index],
            EdgeId(edge_ids[index]),
            deletes[index],
        );
        nbr.create_ts = creates[index];
        out.push(nbr);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip_u64(values: &[u64]) -> Vec<u64> {
        let (_, payload) = encode_topology_u64_column(values);
        let mut offset = 0usize;
        decode_topology_u64_column(&payload, &mut offset).expect("decode must succeed");
        assert_eq!(offset, payload.len());
        decode_topology_u64_column(&payload, &mut 0usize).expect("decode must succeed")
    }

    #[test]
    fn topology_u64_roundtrip_small_range_uses_bitpacking() {
        let values: Vec<u64> = (1000..1100).collect();
        let (choice, _) = encode_topology_u64_column(&values);
        assert_eq!(choice.encoding, TopologyColumnEncoding::BitPacked);
        assert!(choice.encoded_bytes < choice.plain_bytes);
        assert_eq!(roundtrip_u64(&values), values);
    }

    #[test]
    fn topology_u64_roundtrip_runs_use_rle() {
        let values = vec![7u64; 64];
        let (choice, _) = encode_topology_u64_column(values.as_slice());
        assert_eq!(choice.encoding, TopologyColumnEncoding::Rle);
        assert_eq!(roundtrip_u64(&values), values);
    }

    #[test]
    fn topology_u64_falls_back_to_plain_without_benefit() {
        let values = vec![u64::MIN, u64::MAX, 1, u64::MAX - 1];
        let (choice, _) = encode_topology_u64_column(values.as_slice());
        assert_eq!(choice.encoding, TopologyColumnEncoding::Plain);
        assert_eq!(roundtrip_u64(&values), values);
    }

    #[test]
    fn topology_u32_and_i64_roundtrip() {
        let narrow = vec![0u32, 1, 2, 3, 100];
        let (_, payload) = encode_topology_u32_column(&narrow);
        let mut offset = 0usize;
        assert_eq!(
            decode_topology_u32_column(&payload, &mut offset).unwrap(),
            narrow
        );
        assert_eq!(offset, payload.len());

        let ranks = vec![0i64, 0, 1, 1, 2];
        let (_, payload) = encode_topology_i64_column(&ranks);
        let mut offset = 0usize;
        assert_eq!(
            decode_topology_i64_column(&payload, &mut offset).unwrap(),
            ranks
        );
        assert_eq!(offset, payload.len());
    }

    #[test]
    fn raw_columns_roundtrip_at_native_width() {
        let u32_values = vec![0u32, 1, u32::MAX, 4096];
        let mut payload = Vec::new();
        write_raw_u32_column(&u32_values, &mut payload);
        let mut offset = 0usize;
        assert_eq!(
            decode_raw_u32_column(&payload, &mut offset).unwrap(),
            u32_values
        );
        assert_eq!(offset, payload.len());

        let i64_values = vec![0i64, -1, 1, i64::MIN, i64::MAX];
        let mut payload = Vec::new();
        write_raw_i64_column(&i64_values, &mut payload);
        let mut offset = 0usize;
        assert_eq!(
            decode_raw_i64_column(&payload, &mut offset).unwrap(),
            i64_values
        );
        assert_eq!(offset, payload.len());

        let u64_values = vec![0u64, 1, u64::MAX];
        let mut payload = Vec::new();
        write_raw_u64_column(&u64_values, &mut payload);
        let mut offset = 0usize;
        assert_eq!(
            decode_raw_u64_column(&payload, &mut offset).unwrap(),
            u64_values
        );
        assert_eq!(offset, payload.len());

        let empty: Vec<u32> = Vec::new();
        let mut payload = Vec::new();
        write_raw_u32_column(&empty, &mut payload);
        let mut offset = 0usize;
        assert!(decode_raw_u32_column(&payload, &mut offset)
            .unwrap()
            .is_empty());

        let mut short = Vec::new();
        write_raw_u64_column(&[1u64, 2], &mut short);
        short.pop();
        assert!(decode_raw_u64_column(&short, &mut 0usize).is_err());
    }

    #[test]
    fn topology_column_rejects_bad_encoding_and_trailing() {
        let (_, mut payload) = encode_topology_u64_column(&[1u64, 2, 3]);
        payload[0] = 99;
        assert!(decode_topology_u64_column(&payload, &mut 0usize).is_err());

        let (_, mut payload) = encode_topology_u64_column(&[1u64, 2, 3]);
        payload.push(0);
        let mut offset = 0usize;
        decode_topology_u64_column(&payload, &mut offset).unwrap();
        assert_eq!(offset, payload.len() - 1);

        let short = vec![0u8; 3];
        assert!(decode_topology_u64_column(&short, &mut 0usize).is_err());
    }
}
