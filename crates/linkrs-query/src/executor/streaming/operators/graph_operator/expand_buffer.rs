use crate::executor::streaming::chunk::DataChunk;
use linkrs_core::Value;

/// Reusable buffer for building expand output rows without repeated allocation.
///
/// For each seed row, the buffer clones the seed row into `row_buf`, pushes
/// the edge and destination vertex values, then takes ownership of the
/// completed row via `std::mem::take`. This avoids per-row `Vec::clone`
/// allocation overhead.
pub(super) struct ExpandOutputBuffer {
    row_buf: Vec<Value>,
    rows: Vec<Vec<Value>>,
}

impl ExpandOutputBuffer {
    pub(super) fn new(seed_width: usize, capacity: usize) -> Self {
        Self {
            row_buf: Vec::with_capacity(seed_width + 2),
            rows: Vec::with_capacity(capacity),
        }
    }

    #[inline]
    pub(super) fn push_row(&mut self, seed_row: &[Value], edge: Value, dst: Value) {
        self.row_buf.clear();
        self.row_buf.extend_from_slice(seed_row);
        self.row_buf.push(edge);
        self.row_buf.push(dst);
        let row = std::mem::take(&mut self.row_buf);
        self.rows.push(row);
    }

    pub(super) fn finish(self) -> Vec<Vec<Value>> {
        self.rows
    }
}

/// Iterator over the visible rows of a chunk.
///
/// When a selection vector is attached, only the selected rows are
/// yielded, preserving the absolute upstream row order. The output carries
/// `(row_index, &row)` so consumers that need the absolute index (e.g. for
/// `get_variable` on a per-row basis) keep working identically.
pub(super) struct VisibleRows<'a> {
    chunk: &'a DataChunk,
    pos: usize,
}

impl<'a> Iterator for VisibleRows<'a> {
    type Item = (usize, &'a Vec<Value>);

    fn next(&mut self) -> Option<Self::Item> {
        match self.chunk.selection() {
            Some(indices) => {
                let i = *indices.get(self.pos)?;
                self.pos += 1;
                Some((i, &self.chunk.rows[i]))
            }
            None => {
                let i = self.pos;
                let row = self.chunk.rows.get(i)?;
                self.pos += 1;
                Some((i, row))
            }
        }
    }
}

/// Yield the visible rows of `chunk` in upstream row order.
pub(super) fn visible_rows(chunk: &DataChunk) -> VisibleRows<'_> {
    VisibleRows { chunk, pos: 0 }
}
