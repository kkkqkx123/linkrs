//! Recursive assembly of operators and fragment DAG edges.

use super::super::types::FragmentId;

mod conversion;
mod fragment_ops;

pub(super) struct ArenaPlanAssembler;

pub(super) struct ArenaFragmentAllocator {
    next: usize,
}

impl ArenaFragmentAllocator {
    pub(super) fn new() -> Self {
        Self { next: 0 }
    }

    pub(super) fn allocate(&mut self) -> FragmentId {
        let id = FragmentId(self.next);
        self.next += 1;
        id
    }
}
