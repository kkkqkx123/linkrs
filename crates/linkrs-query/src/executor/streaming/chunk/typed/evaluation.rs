use super::super::columnar_common::{gather_bitmap, gather_column};
use super::TypedColumn;

/// Gather a typed column's entries at `indices`.
pub(crate) fn gather_typed_column(column: &TypedColumn, indices: &[usize]) -> TypedColumn {
    gather_column!(TypedColumn, column, indices)
}

/// Repeat every row of `column` `multiplicity` times (row duplication).
///
/// Mirrors `expand_rows_reusing_buffers` on the rows path so a chunk that
/// expands `multiplicity` keeps its typed columnar layout instead of dropping
/// it. The order must match that helper exactly: block replication
/// `[all rows] * multiplicity` (NOT interleaved), otherwise the typed column
/// and the rows would disagree. Implemented via an index vector fed to
/// [`gather_typed_column`] so the validity bitmap of the `Nullable*` variants
/// stays in lockstep with the values.
pub(crate) fn repeat_typed_column(column: &TypedColumn, multiplicity: usize) -> TypedColumn {
    if multiplicity <= 1 {
        return gather_typed_column(column, &(0..column.len()).collect::<Vec<_>>());
    }
    let round = column.len();
    let mut indices = Vec::with_capacity(round.saturating_mul(multiplicity));
    for _ in 0..multiplicity {
        indices.extend(0..round);
    }
    gather_typed_column(column, &indices)
}
