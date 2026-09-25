/// Columns file record-layout version. Development builds keep this at 1;
/// there is no backward compatibility with any other layout.
pub const COLUMNS_FORMAT_VERSION: u8 = 1;

/// Pick one encoding for a column by profiling each chunk independently
/// (streaming, no column-wide value vector) and voting for the most common
/// non-None chunk choice. Hot chunks vote `None` through the profile.
pub(super) fn select_encoding_for_column(
    col: &crate::vertex::column::Column,
    selector: &crate::encoding::EncodingSelector,
) -> crate::encoding::EncodingType {
    use crate::encoding::{profile_chunk, EncodingType};
    use std::collections::HashMap;
    if col.is_empty() {
        return EncodingType::None;
    }
    let capacity = col.chunk_capacity().max(1);
    let total = col.len();
    let n_chunks = total.div_ceil(capacity).max(1);
    let mut votes: HashMap<u8, usize> = HashMap::new();
    for ci in 0..n_chunks {
        let start = ci * capacity;
        let end = (start + capacity).min(total);
        let hot = col.chunk_needs_recode_for_row(start);
        let profile = profile_chunk((start..end).map(|r| col.get(r)), &col.data_type, hot);
        let choice = selector.select_for_chunk_profile(&profile);
        *votes.entry(choice.to_u8()).or_insert(0) += 1;
    }
    let (best_tag, best_count) = votes
        .iter()
        .max_by_key(|(_, count)| **count)
        .map(|(tag, count)| (*tag, *count))
        .unwrap_or((EncodingType::None.to_u8(), 0));
    if best_tag == EncodingType::None.to_u8() || best_count == 0 {
        return EncodingType::None;
    }
    // Require a majority for non-trivial encodings on multi-chunk columns so
    // a single odd chunk cannot force a column-wide encoding.
    if n_chunks > 1 && best_count * 2 <= n_chunks {
        return EncodingType::None;
    }
    EncodingType::from_u8(best_tag)
}
