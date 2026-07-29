use crate::core::types::Timestamp;
use crate::storage::index::key_codec::key_types::SecondaryIndexKey;
use crate::storage::index::types::IndexRecord;

pub(crate) type ChunkId = u32;

pub(crate) const CHUNK_TARGET_SIZE: usize = 65536;

#[derive(Clone)]
pub(crate) struct Chunk {
    pub(crate) id: ChunkId,
    pub(crate) min_key: SecondaryIndexKey,
    pub(crate) max_key: SecondaryIndexKey,
    pub(crate) entries: Vec<(SecondaryIndexKey, IndexRecord)>,
    pub(crate) estimated_size: usize,
    pub(crate) live_count: usize,
}

impl Chunk {
    pub(crate) fn new(
        id: ChunkId,
        entries: Vec<(SecondaryIndexKey, IndexRecord)>,
    ) -> Self {
        let min_key = entries
            .first()
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        let max_key = entries
            .last()
            .map(|(k, _)| k.clone())
            .unwrap_or_default();
        let estimated_size = estimate_entries_size(&entries);
        let live_count = entries.iter().filter(|(_, e)| e.deleted_ts.is_none()).count();
        Self {
            id,
            min_key,
            max_key,
            entries,
            estimated_size,
            live_count,
        }
    }

    pub(crate) fn range(
        &self,
        lower: &[u8],
        upper: &[u8],
    ) -> Vec<(SecondaryIndexKey, IndexRecord)> {
        let start = self
            .entries
            .partition_point(|(k, _)| k.as_slice() < lower);
        let end = if upper.is_empty() {
            self.entries.len()
        } else {
            self
                .entries
                .partition_point(|(k, _)| k.as_slice() < upper)
        };
        if start >= end {
            return Vec::new();
        }
        self.entries[start..end].to_vec()
    }

    pub(crate) fn visible_range_iter(
        &self,
        lower: &[u8],
        upper: &[u8],
        read_ts: Timestamp,
    ) -> impl Iterator<Item = &(SecondaryIndexKey, IndexRecord)> {
        let start = self
            .entries
            .partition_point(|(k, _)| k.as_slice() < lower);
        let end = if upper.is_empty() {
            self.entries.len()
        } else {
            self
                .entries
                .partition_point(|(k, _)| k.as_slice() < upper)
        };
        self.entries[start..end]
            .iter()
            .filter(move |(_, entry)| entry.is_visible_at(read_ts))
    }

}

pub(crate) fn build_chunks(
    entries: Vec<(SecondaryIndexKey, IndexRecord)>,
    chunk_target_size: usize,
) -> Vec<Chunk> {
    if entries.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut chunk_start = 0usize;
    let mut chunk_id = 0u32;

    while chunk_start < entries.len() {
        let mut chunk_end = chunk_start + 1;
        let mut current_size = estimate_entry_size(&entries[chunk_start]);

        while chunk_end < entries.len() {
            let next_size = estimate_entry_size(&entries[chunk_end]);
            if current_size + next_size > chunk_target_size {
                break;
            }
            current_size += next_size;
            chunk_end += 1;
        }

        let chunk_entries: Vec<_> = entries[chunk_start..chunk_end].to_vec();
        chunks.push(Chunk::new(chunk_id, chunk_entries));
        chunk_id += 1;
        chunk_start = chunk_end;
    }

    chunks
}

fn estimate_entry_size(entry: &(SecondaryIndexKey, IndexRecord)) -> usize {
    let (_key, record) = entry;
    let mut size = entry.0.len() + std::mem::size_of::<IndexRecord>();
    if let Some(ref cols) = record.included_columns {
        for (name, val) in cols {
            size += name.len() + val.estimated_size();
        }
    }
    size
}

fn estimate_entries_size(entries: &[(SecondaryIndexKey, IndexRecord)]) -> usize {
    entries.iter().map(estimate_entry_size).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::Timestamp;

    fn make_entry(key_suffix: u8) -> (SecondaryIndexKey, IndexRecord) {
        (vec![key_suffix], IndexRecord::new(key_suffix as Timestamp))
    }

    #[test]
    fn chunk_new_sets_min_max_key() {
        let entries = vec![make_entry(10), make_entry(20), make_entry(30)];
        let chunk = Chunk::new(0, entries);
        assert_eq!(chunk.min_key, vec![10]);
        assert_eq!(chunk.max_key, vec![30]);
    }

    #[test]
    fn chunk_range_returns_correct_slice() {
        let entries = vec![
            make_entry(1),
            make_entry(2),
            make_entry(3),
            make_entry(4),
            make_entry(5),
        ];
        let chunk = Chunk::new(0, entries);
        let range = chunk.range(&[2], &[5]);
        assert_eq!(range.len(), 3);
        assert_eq!(range[0].0, vec![2]);
        assert_eq!(range[1].0, vec![3]);
        assert_eq!(range[2].0, vec![4]);
    }

    #[test]
    fn chunk_range_empty_when_outside() {
        let entries = vec![make_entry(10), make_entry(20)];
        let chunk = Chunk::new(0, entries);
        assert!(chunk.range(&[1], &[9]).is_empty());
        assert!(chunk.range(&[30], &[40]).is_empty());
    }

    #[test]
    fn build_chunks_respects_target_size() {
        let mut entries = Vec::new();
        for i in 0u8..100 {
            entries.push(make_entry(i));
        }
        // Each entry is very small (~56 bytes), so all 100 should fit in one chunk
        let chunks = build_chunks(entries, CHUNK_TARGET_SIZE);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].entries.len(), 100);
    }

    #[test]
    fn build_chunks_splits_large_entries() {
        let mut entries = Vec::new();
        for i in 0u8..20 {
            let key = vec![i; 4096];
            let value = IndexRecord::new(i as Timestamp);
            entries.push((key, value));
        }
        // Each entry is ~4KB, with 64KB target we expect ~16 entries per chunk
        let chunks = build_chunks(entries, 65536);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].entries.len() <= 17);
    }

    #[test]
    fn empty_entries_produces_no_chunks() {
        let chunks = build_chunks(vec![], 65536);
        assert!(chunks.is_empty());
    }

}
