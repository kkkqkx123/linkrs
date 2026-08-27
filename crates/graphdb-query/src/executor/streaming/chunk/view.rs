//! Zero-copy view into DataChunk rows

/// A zero-copy view into a slice of rows within a [`DataChunk`].
///
/// Created by [`DataChunk::view`](super::DataChunk::view).  The view borrows the parent chunk
/// and does not own its data.
#[derive(Debug)]
pub struct ChunkView<'a> {
    pub(crate) rows: &'a [Vec<crate::core::Value>],
}

impl ChunkView<'_> {
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn row(&self, idx: usize) -> Option<&[crate::core::Value]> {
        self.rows.get(idx).map(|r| r.as_slice())
    }
}
