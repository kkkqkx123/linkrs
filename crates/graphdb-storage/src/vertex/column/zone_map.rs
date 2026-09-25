use graphdb_core::Value;

use super::Column;

/// Rows per zone-map chunk of a [`Column`].
pub const ZONE_MAP_CHUNK_ROWS: usize = 1024;

/// Versioned writes after which a column becomes eligible for an exact zone
/// rebuild. Long update histories leave widened bounds permanently stale;
/// crossing this threshold marks the bounds for a shrink-safe rebuild that
/// still covers retained version chains.
pub const ZONE_STALE_REBUILD_THRESHOLD: u64 = 1024;

/// Conservative min/max bounds over the non-null values of one chunk.
#[derive(Debug, Clone, Default)]
pub struct ZoneBounds {
    pub min: Option<Value>,
    pub max: Option<Value>,
}

/// Prunable length summary for complex and variable-length values.
///
/// Opaque nested values keep whole-value min/max for ordering-based pruning,
/// but ordering alone cannot prune equality queries whose values differ only
/// in content length. This summary tracks the minimum and maximum container
/// or payload length observed in the chunk so equality queries whose probe
/// length falls outside the interval can skip the whole chunk. Bounds only
/// widen, matching the conservative contract of [`ZoneBounds`].
///
/// `leaf_min`/`leaf_max` extend the same idea below the container surface: the
/// minimum and maximum scalar leaf over every nested value in the chunk
/// (list elements, map values, struct fields, vector components, JSON
/// leaves, graph property values). An equality probe whose own leaf interval
/// is disjoint cannot match any value in the chunk even when outer lengths
/// coincide. `key_fp` is a 64-bit bloom over map keys, struct field names,
/// JSON object keys, and graph property names; a probe key fingerprint with
/// bits outside the chunk fingerprint cannot match either. Both widen only
/// (interval expansion, bit OR), so pruning stays sound for any snapshot.
#[derive(Debug, Clone, Default)]
pub struct ComplexZoneSummary {
    pub len_min: Option<usize>,
    pub len_max: Option<usize>,
    pub leaf_min: Option<Value>,
    pub leaf_max: Option<Value>,
    pub key_fp: u64,
    pub count: u64,
}

/// Container or payload length of `value` when it carries a prunable length.
///
/// Scalars without a meaningful length return `None` and leave the summary
/// untouched. Strings, blobs and all nested containers report their element
/// or byte length so equality probes can prune on length first.
pub fn complex_len(value: &Value) -> Option<usize> {
    match value {
        Value::String(s) => Some(s.len()),
        Value::FixedString(s) => Some(s.len()),
        Value::Blob(b) => Some(b.len()),
        Value::List(l) => Some(l.len()),
        Value::Map(m) => Some(m.len()),
        Value::Set(s) => Some(s.len()),
        Value::Vector(v) => Some(v.dimension()),
        Value::Json(j) => Some(j.as_str().len()),
        Value::JsonB(j) => Some(j.to_json_string().len()),
        Value::DataSet(ds) => Some(ds.rows.len()),
        Value::Vertex(v) => Some(v.tag.properties.len()),
        Value::Edge(e) => Some(e.props.len()),
        Value::Path(p) => Some(p.steps.len()),
        _ => None,
    }
}

/// Scalar-leaf interval of a complex value, if it carries prunable leaves.
///
/// Flattens one level of nesting recursively (list elements, map values,
/// struct fields, vector components, JSON leaves, graph property values)
/// and returns the minimum and maximum leaf under [`compare_values`].
/// Values without leaves (empty containers, opaque path steps) return
/// `None` and leave the summary untouched. The same flattening applies to
/// chunk values and equality probes, so an exactly equal pair always shares
/// its interval and pruning on disjoint intervals stays sound.
pub fn complex_leaf_range(value: &Value) -> Option<(Value, Value)> {
    let mut leaves = Vec::new();
    collect_leaves(value, &mut leaves);
    let mut iter = leaves.into_iter();
    let first = iter.next()?;
    let mut lo = first.clone();
    let mut hi = first;
    for leaf in iter {
        if compare_values(&leaf, &lo) == std::cmp::Ordering::Less {
            lo = leaf.clone();
        }
        if compare_values(&leaf, &hi) == std::cmp::Ordering::Greater {
            hi = leaf;
        }
    }
    Some((lo, hi))
}

/// Key fingerprint of a complex value for bloom pre-pruning.
///
/// ORs two-tap 64-bit bloom bits over map keys, struct field names, JSON
/// object keys, dataset column names, and graph property names. Values
/// without keys contribute zero. An exactly equal pair shares its key set,
/// so a probe fingerprint with bits outside the chunk fingerprint cannot
/// match any value in the chunk.
pub fn complex_key_fp(value: &Value) -> u64 {
    let mut fp = 0u64;
    collect_key_fp(value, &mut fp);
    fp
}

fn bloom_bits_for_str(name: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    name.hash(&mut hasher);
    let hash = hasher.finish();
    (1u64 << (hash & 63)) | (1u64 << ((hash >> 7) & 63))
}

fn bloom_bits_for_key(key: &Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    key.hash(&mut hasher);
    let hash = hasher.finish();
    (1u64 << (hash & 63)) | (1u64 << ((hash >> 7) & 63))
}

fn collect_leaves(value: &Value, out: &mut Vec<Value>) {
    match value {
        Value::List(l) => {
            for element in &l.values {
                collect_leaves(element, out);
            }
        }
        Value::Array(a) => {
            for element in &a.values {
                collect_leaves(element, out);
            }
        }
        Value::Map(m) => {
            for element in m.values() {
                collect_leaves(element, out);
            }
        }
        Value::Set(s) => {
            for element in s.iter() {
                collect_leaves(element, out);
            }
        }
        Value::Struct(s) => {
            for (_, element) in &s.fields {
                collect_leaves(element, out);
            }
        }
        Value::Vector(v) => {
            for component in v.to_dense() {
                out.push(Value::Double(component as f64));
            }
        }
        Value::DataSet(ds) => {
            for row in &ds.rows {
                for element in row {
                    collect_leaves(element, out);
                }
            }
        }
        Value::Json(j) => {
            if let Ok(parsed) = j.to_value() {
                collect_json_leaves(&parsed, out);
            }
        }
        Value::JsonB(j) => {
            collect_json_leaves(j.as_value(), out);
        }
        Value::Vertex(v) => {
            for element in v.tag.properties.values() {
                collect_leaves(element, out);
            }
        }
        Value::Edge(e) => {
            for element in e.props.values() {
                collect_leaves(element, out);
            }
        }
        Value::String(_) | Value::FixedString(_) | Value::Blob(_) => {
            out.push(value.clone());
        }
        _ => {
            out.push(value.clone());
        }
    }
}

fn collect_json_leaves(value: &serde_json::Value, out: &mut Vec<Value>) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(number) = n.as_f64() {
                out.push(Value::Double(number));
            }
        }
        serde_json::Value::String(s) => {
            out.push(Value::string(s));
        }
        serde_json::Value::Bool(b) => {
            out.push(Value::Bool(*b));
        }
        serde_json::Value::Array(elements) => {
            for element in elements {
                collect_json_leaves(element, out);
            }
        }
        serde_json::Value::Object(fields) => {
            for element in fields.values() {
                collect_json_leaves(element, out);
            }
        }
        serde_json::Value::Null => {}
    }
}

fn collect_key_fp(value: &Value, fp: &mut u64) {
    match value {
        Value::Map(m) => {
            for key in m.keys() {
                *fp |= bloom_bits_for_key(key);
            }
            for element in m.values() {
                collect_key_fp(element, fp);
            }
        }
        Value::Struct(s) => {
            for (name, element) in &s.fields {
                *fp |= bloom_bits_for_str(name);
                collect_key_fp(element, fp);
            }
        }
        Value::Json(j) => {
            if let Ok(parsed) = j.to_value() {
                collect_json_key_fp(&parsed, fp);
            }
        }
        Value::JsonB(j) => {
            collect_json_key_fp(j.as_value(), fp);
        }
        Value::DataSet(ds) => {
            for name in &ds.col_names {
                *fp |= bloom_bits_for_str(name);
            }
        }
        Value::Vertex(v) => {
            for (name, element) in &v.tag.properties {
                *fp |= bloom_bits_for_str(name);
                collect_key_fp(element, fp);
            }
        }
        Value::Edge(e) => {
            for (name, element) in &e.props {
                *fp |= bloom_bits_for_str(name);
                collect_key_fp(element, fp);
            }
        }
        Value::List(l) => {
            for element in &l.values {
                collect_key_fp(element, fp);
            }
        }
        Value::Array(a) => {
            for element in &a.values {
                collect_key_fp(element, fp);
            }
        }
        Value::Set(s) => {
            for element in s.iter() {
                collect_key_fp(element, fp);
            }
        }
        _ => {}
    }
}

fn collect_json_key_fp(value: &serde_json::Value, fp: &mut u64) {
    match value {
        serde_json::Value::Object(fields) => {
            for (name, element) in fields {
                *fp |= bloom_bits_for_str(name);
                collect_json_key_fp(element, fp);
            }
        }
        serde_json::Value::Array(elements) => {
            for element in elements {
                collect_json_key_fp(element, fp);
            }
        }
        _ => {}
    }
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
        if chunk >= self.zone_complex.len() {
            self.zone_complex
                .resize_with(chunk + 1, ComplexZoneSummary::default);
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
        if let Some(len) = complex_len(v) {
            let summary = &mut self.zone_complex[chunk];
            summary.count += 1;
            match summary.len_min {
                Some(cur) if cur <= len => {}
                _ => summary.len_min = Some(len),
            }
            match summary.len_max {
                Some(cur) if cur >= len => {}
                _ => summary.len_max = Some(len),
            }
            if let Some((leaf_lo, leaf_hi)) = complex_leaf_range(v) {
                match &summary.leaf_min {
                    Some(cur) if compare_values(cur, &leaf_lo) != std::cmp::Ordering::Greater => {}
                    _ => summary.leaf_min = Some(leaf_lo),
                }
                match &summary.leaf_max {
                    Some(cur) if compare_values(cur, &leaf_hi) != std::cmp::Ordering::Less => {}
                    _ => summary.leaf_max = Some(leaf_hi),
                }
            }
            summary.key_fp |= complex_key_fp(v);
        }
    }

    /// Recompute chunk bounds from the current column contents without
    /// shrinking them.
    ///
    /// Recomputation only widens: historical extrema stay, so the bounds
    /// contain every non-null value any snapshot can still observe through
    /// a version chain, not just the current contents. Pruning against
    /// these bounds is therefore sound for any snapshot timestamp; the
    /// price is the documented stale-but-conservative tradeoff.
    pub fn rebuild_zone_maps(&mut self) {
        for row_idx in 0..self.len() {
            // Chunk-aware base read: overlay first, then chunk encodings.
            let value = self.get(row_idx);
            self.update_zone_maps(row_idx, value.as_ref());
        }
    }

    /// Exact rebuild covering current values plus retained version chains.
    ///
    /// Unlike [`Self::rebuild_zone_maps`], this clears the bounds first and
    /// recomputes from every value a retained snapshot can still observe
    /// (current plus all before-images). Extremes whose versions have been
    /// garbage-collected shrink away, while live history stays covered, so
    /// pruning regains precision without breaking historical reads. The
    /// stale-write counter resets on success.
    pub fn rebuild_zone_maps_exact(&mut self) {
        self.zone_maps.clear();
        self.zone_complex.clear();
        self.zone_stale_writes = 0;
        let total = self.len();
        for row_idx in 0..total {
            let value = self.get(row_idx);
            self.update_zone_maps(row_idx, value.as_ref());
        }
        let chained: Vec<(usize, Option<Value>)> = self.with_version_chains_read(|chains| {
            chains
                .map(|entries| {
                    entries
                        .iter()
                        .enumerate()
                        .flat_map(|(row, chain)| {
                            chain.iter().map(move |entry| (row, entry.value.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default()
        });
        for (row_idx, value) in chained {
            self.update_zone_maps(row_idx, value.as_ref());
        }
    }

    /// Whether accumulated versioned writes make an exact rebuild worthwhile.
    pub fn zone_needs_exact_rebuild(&self) -> bool {
        self.zone_stale_writes >= ZONE_STALE_REBUILD_THRESHOLD
    }

    /// Rebuild exactly when the stale-write threshold is crossed. Returns
    /// whether a rebuild happened.
    pub fn maybe_rebuild_zone_maps_exact(&mut self) -> bool {
        if !self.zone_needs_exact_rebuild() {
            return false;
        }
        self.rebuild_zone_maps_exact();
        true
    }

    /// Per-chunk min/max bounds (one entry per [`ZONE_MAP_CHUNK_ROWS`] rows).
    pub fn zone_maps(&self) -> &[ZoneBounds] {
        &self.zone_maps
    }

    /// Per-chunk length summaries parallel to [`Self::zone_maps`].
    pub fn zone_complex(&self) -> &[ComplexZoneSummary] {
        &self.zone_complex
    }

    /// Length interval of one zone chunk, if any measured value exists.
    pub fn complex_len_bounds(&self, chunk: usize) -> Option<(usize, usize)> {
        self.zone_complex
            .get(chunk)
            .and_then(|s| match (s.len_min, s.len_max) {
                (Some(lo), Some(hi)) => Some((lo, hi)),
                _ => None,
            })
    }
}
