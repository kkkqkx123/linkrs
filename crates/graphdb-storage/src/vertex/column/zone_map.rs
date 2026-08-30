use graphdb_core::Value;

use super::Column;

/// Rows per zone-map chunk of a [`Column`].
pub const ZONE_MAP_CHUNK_ROWS: usize = 1024;

/// Conservative min/max bounds over the non-null values of one chunk.
#[derive(Debug, Clone, Default)]
pub struct ZoneBounds {
    pub min: Option<Value>,
    pub max: Option<Value>,
}

/// Compare two scalar values with the same semantics as pushed-predicate
/// evaluation: exact `i64` for integer kinds, `f64` when a float is
/// involved, otherwise `Value` ordering.
pub fn compare_values(a: &Value, b: &Value) -> std::cmp::Ordering {
    fn as_i64(value: &Value) -> Option<i64> {
        match value {
            Value::SmallInt(v) => Some(*v as i64),
            Value::Int(v) => Some(*v as i64),
            Value::BigInt(v) => Some(*v),
            _ => None,
        }
    }
    fn as_f64(value: &Value) -> Option<f64> {
        match value {
            Value::SmallInt(v) => Some(*v as f64),
            Value::Int(v) => Some(*v as f64),
            Value::BigInt(v) => Some(*v as f64),
            Value::Float(v) => Some(*v as f64),
            Value::Double(v) => Some(*v),
            _ => None,
        }
    }
    match (as_i64(a), as_i64(b)) {
        (Some(x), Some(y)) => x.cmp(&y),
        _ => match (as_f64(a), as_f64(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            _ => Value::cmp(a, b),
        },
    }
}

// ---------------------------------------------------------------------------
// Column zone-map methods
// ---------------------------------------------------------------------------

impl Column {
    /// Widen the chunk bounds covering `row_idx` with `value`.
    ///
    /// Bounds never shrink: a later update that removes a chunk's extreme
    /// leaves stale-but-conservative bounds, which keeps pruning sound
    /// for any MVCC snapshot.
    pub(super) fn update_zone_maps(&mut self, row_idx: usize, value: Option<&Value>) {
        let Some(v) = value else {
            return;
        };
        if v.is_null() {
            return;
        }
        let chunk = row_idx / ZONE_MAP_CHUNK_ROWS;
        if chunk >= self.zone_maps.len() {
            self.zone_maps.resize_with(chunk + 1, ZoneBounds::default);
        }
        let bounds = &mut self.zone_maps[chunk];
        match &bounds.min {
            Some(min) if compare_values(min, v) != std::cmp::Ordering::Greater => {}
            _ => bounds.min = Some(v.clone()),
        }
        match &bounds.max {
            Some(max) if compare_values(max, v) != std::cmp::Ordering::Less => {}
            _ => bounds.max = Some(v.clone()),
        }
    }

    /// Recompute all chunk bounds from the current column contents.
    pub fn rebuild_zone_maps(&mut self) {
        self.zone_maps.clear();
        for row_idx in 0..self.len() {
            let value = if self.encoding.is_encoded() {
                self.encoding.get(row_idx)
            } else {
                self.inner().get(row_idx)
            };
            self.update_zone_maps(row_idx, value.as_ref());
        }
    }

    /// Per-chunk min/max bounds (one entry per [`ZONE_MAP_CHUNK_ROWS`] rows).
    pub fn zone_maps(&self) -> &[ZoneBounds] {
        &self.zone_maps
    }
}
