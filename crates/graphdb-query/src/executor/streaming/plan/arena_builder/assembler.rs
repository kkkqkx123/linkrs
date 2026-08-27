//! Recursive assembly of operators and fragment DAG edges.

use super::super::types::FragmentId;

mod conversion;
pub(super) mod fragment_ops;

pub(super) use conversion::build_subquery_runner_specs;
pub(super) use fragment_ops::BinaryOperatorSpec;
pub(super) use fragment_ops::FragmentCtx;
pub(super) use fragment_ops::HashExchangeParams;

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
