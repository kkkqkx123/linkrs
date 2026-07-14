use bitvec::prelude::*;
use std::collections::HashSet;

use crate::core::types::storage_ids::VertexId;

const DENSE_THRESHOLD: usize = 64;

enum Inner {
    Sparse(HashSet<VertexId>),
    Dense {
        ids: HashSet<VertexId>,
        bitmap: BitVec,
        offset: i64,
    },
}

pub struct VisitedSet {
    inner: Inner,
    switch_count: usize,
}

impl VisitedSet {
    pub fn new() -> Self {
        Self {
            inner: Inner::Sparse(HashSet::new()),
            switch_count: 0,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            inner: Inner::Sparse(HashSet::with_capacity(cap)),
            switch_count: 0,
        }
    }

    pub fn contains(&self, id: &VertexId) -> bool {
        match &self.inner {
            Inner::Sparse(set) => set.contains(id),
            Inner::Dense { ids, bitmap, offset } => {
                if let Some(idx) = id_to_dense_index(id, *offset) {
                    if idx < bitmap.len() && bitmap[idx] {
                        return true;
                    }
                }
                ids.contains(id)
            }
        }
    }

    pub fn insert(&mut self, id: VertexId) -> bool {
        match &mut self.inner {
            Inner::Sparse(set) => {
                if set.len() >= DENSE_THRESHOLD && self.switch_count < 3 {
                    if let Some((offset, max_id)) = try_switch_to_dense(set, &id) {
                        let mut bitmap = bitvec![0; (max_id - offset + 1) as usize];
                        for existing in set.iter() {
                            if let Some(idx) = id_to_dense_index(existing, offset) {
                                if idx < bitmap.len() {
                                    bitmap.set(idx, true);
                                }
                            }
                        }
                        let new = if let Some(idx) = id_to_dense_index(&id, offset) {
                            if idx < bitmap.len() {
                                let already = bitmap[idx];
                                bitmap.set(idx, true);
                                already
                            } else {
                                false
                            }
                        } else {
                            false
                        };
                        if !new {
                            set.insert(id);
                        }
                        self.inner = Inner::Dense { ids: std::mem::take(set), bitmap, offset };
                        self.switch_count += 1;
                        return !new;
                    }
                }
                set.insert(id)
            }
            Inner::Dense { ids, bitmap, offset } => {
                if let Some(idx) = id_to_dense_index(&id, *offset) {
                    if idx < bitmap.len() {
                        if bitmap[idx] {
                            return false;
                        }
                        bitmap.set(idx, true);
                        return ids.insert(id);
                    }
                }
                ids.insert(id)
            }
        }
    }

    pub fn len(&self) -> usize {
        match &self.inner {
            Inner::Sparse(set) => set.len(),
            Inner::Dense { ids, .. } => ids.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        match &mut self.inner {
            Inner::Sparse(set) => set.clear(),
            Inner::Dense { ids, bitmap, .. } => {
                ids.clear();
                bitmap.clear();
            }
        }
    }

    pub fn iter(&self) -> Box<dyn Iterator<Item = &VertexId> + '_> {
        match &self.inner {
            Inner::Sparse(set) => Box::new(set.iter()),
            Inner::Dense { ids, .. } => Box::new(ids.iter()),
        }
    }
}

fn id_to_dense_index(id: &VertexId, offset: i64) -> Option<usize> {
    let raw = id.as_int64()?;
    let idx = raw.checked_sub(offset)?;
    if idx < 0 {
        return None;
    }
    usize::try_from(idx).ok()
}

fn try_switch_to_dense(
    set: &HashSet<VertexId>,
    candidate: &VertexId,
) -> Option<(i64, i64)> {
    let min_id = set.iter().filter_map(|id| id.as_int64()).min()?;
    let max_id = set.iter().filter_map(|id| id.as_int64()).max()?;
    let candidate_int = candidate.as_int64()?;

    let overall_min = min_id.min(candidate_int);
    let overall_max = max_id.max(candidate_int);

    let range_len = (overall_max - overall_min + 1) as u64;
    if range_len > 1_000_000 {
        return None;
    }

    Some((overall_min, overall_max))
}

impl Default for VisitedSet {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for VisitedSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.inner {
            Inner::Sparse(set) => write!(f, "VisitedSet(Sparse, len={})", set.len()),
            Inner::Dense { ids, bitmap, .. } => {
                write!(
                    f,
                    "VisitedSet(Dense, ids={}, bitmap_ones={})",
                    ids.len(),
                    bitmap.count_ones()
                )
            }
        }
    }
}
