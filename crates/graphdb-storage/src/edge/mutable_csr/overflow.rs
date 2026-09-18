use super::super::Nbr;

/// Single benchmarked per-vertex overflow bound. Past this many chunks a row
/// is repacked on the write path; a row holding only live entries is merged
/// the same way so skewed rows cannot grow unbounded chains. Region and full
/// compactions still handle watermark reclaim separately.
pub(crate) const OVERFLOW_REPACK_CHUNKS_PER_VERTEX: usize = 8;

/// Per-vertex overflow storage with dense row indexing.
///
/// Row addresses inside one CSR are dense vertex ids, so chunks are addressed
/// by direct subscript instead of hashing. Empty rows hold `None` and cost
/// only the slot; sparsity across groups is still handled by the group map
/// above this layer.
#[derive(Debug, Clone, Default)]
pub struct OverflowStorage {
    slots: Vec<Option<Vec<Vec<Nbr>>>>,
    live_entries: usize,
}

impl OverflowStorage {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            live_entries: 0,
        }
    }

    fn slot_index(vid: &u32) -> usize {
        *vid as usize
    }

    /// Grow the slot array so `vid` is addressable. New slots are empty.
    pub fn ensure_capacity(&mut self, vertex_capacity: usize) {
        if self.slots.len() < vertex_capacity {
            self.slots.resize_with(vertex_capacity, || None);
        }
    }

    #[inline]
    pub fn get(&self, vid: &u32) -> Option<&Vec<Vec<Nbr>>> {
        self.slots.get(Self::slot_index(vid))?.as_ref()
    }

    #[inline]
    pub fn get_mut(&mut self, vid: &u32) -> Option<&mut Vec<Vec<Nbr>>> {
        self.slots.get_mut(Self::slot_index(vid))?.as_mut()
    }

    /// Get mutable reference to the chunk list for `vid`, inserting an empty
    /// entry if absent.
    #[inline]
    pub fn get_or_create(&mut self, vid: u32) -> &mut Vec<Vec<Nbr>> {
        let idx = vid as usize;
        if self.slots.len() <= idx {
            self.slots.resize_with(idx + 1, || None);
        }
        let slot = &mut self.slots[idx];
        if slot.is_none() {
            *slot = Some(Vec::new());
            self.live_entries += 1;
        }
        slot.as_mut().expect("slot just created")
    }

    #[inline]
    pub fn insert(&mut self, vid: u32, chunks: Vec<Vec<Nbr>>) {
        let idx = vid as usize;
        if self.slots.len() <= idx {
            self.slots.resize_with(idx + 1, || None);
        }
        if self.slots[idx].is_none() && !chunks.is_empty() {
            self.live_entries += 1;
        } else if self.slots[idx].is_some() && chunks.is_empty() {
            self.live_entries = self.live_entries.saturating_sub(1);
        }
        if chunks.is_empty() {
            self.slots[idx] = None;
        } else {
            self.slots[idx] = Some(chunks);
        }
    }

    #[inline]
    pub fn contains_key(&self, vid: &u32) -> bool {
        self.get(vid).is_some_and(|chunks| !chunks.is_empty())
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.live_entries
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.live_entries == 0
    }

    #[inline]
    pub fn clear(&mut self) {
        for slot in self.slots.iter_mut() {
            *slot = None;
        }
        self.live_entries = 0;
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (u32, &Vec<Vec<Nbr>>)> {
        self.slots
            .iter()
            .enumerate()
            .filter_map(|(idx, slot)| slot.as_ref().map(|chunks| (idx as u32, chunks)))
    }

    #[inline]
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (u32, &mut Vec<Vec<Nbr>>)> {
        self.slots
            .iter_mut()
            .enumerate()
            .filter_map(|(idx, slot)| slot.as_mut().map(|chunks| (idx as u32, chunks)))
    }

    /// Remove entry for `vid` and return its chunks if present.
    #[inline]
    pub fn remove(&mut self, vid: &u32) -> Option<Vec<Vec<Nbr>>> {
        let idx = Self::slot_index(vid);
        if idx >= self.slots.len() {
            return None;
        }
        let taken = self.slots[idx].take();
        if taken.is_some() {
            self.live_entries = self.live_entries.saturating_sub(1);
        }
        taken
    }

    /// Total number of Nbr entries across all overflow chunks.
    pub fn total_nbr_count(&self) -> usize {
        self.slots.iter().flatten().flatten().map(Vec::len).sum()
    }

    /// Estimate of wasted capacity inside overflow chunks (capacity - len).
    pub fn wasted_capacity(&self) -> usize {
        self.slots
            .iter()
            .flatten()
            .flatten()
            .map(|c| c.capacity().saturating_sub(c.len()))
            .sum()
    }

    /// Reserved slot-array memory plus per-row chunk lists.
    pub fn index_bytes(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<Option<Vec<Vec<Nbr>>>>()
            + self.live_entries
                * (std::mem::size_of::<u32>() + std::mem::size_of::<Vec<Vec<Nbr>>>())
    }
}
