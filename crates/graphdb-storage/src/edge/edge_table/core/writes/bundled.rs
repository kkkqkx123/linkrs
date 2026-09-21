use super::super::EdgeStore;
use crate::edge::bundled_csr::{decode_scalar, encode_scalar};
use crate::edge::MutableCsrTrait;
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

impl EdgeStore {
    /// Validate one staged insert against the bundled single-property shape
    /// and encode it to its storage word (`None` for NULL/absent).
    fn convert_bundled_value(
        &self,
        property_values: &[(String, Value)],
    ) -> StorageResult<Option<u64>> {
        if property_values.is_empty() {
            return Ok(None);
        }
        if property_values.len() > 1 {
            return Err(StorageError::invalid_operation(
                "bundled record form stores exactly one property".to_string(),
            ));
        }
        let Some(def) = self.schema.properties.first() else {
            return Err(StorageError::column_not_found(property_values[0].0.clone()));
        };
        let (name, value) = &property_values[0];
        if name != &def.name {
            return Err(StorageError::column_not_found(name.clone()));
        }
        let cast = if value.data_type() != def.data_type {
            value.try_cast_to(&def.data_type)?
        } else {
            value.clone()
        };
        match cast {
            Value::Null(_) | Value::Empty => Ok(None),
            _ => Ok(Some(encode_scalar(&cast))),
        }
    }

    /// Decode one stored inline word back to its indexed pair.
    fn bundled_index_pair(&self, inline_value: Option<u64>) -> Option<(String, Value)> {
        let raw = inline_value?;
        let prop = self.schema.properties.first()?;
        Some((prop.name.clone(), decode_scalar(raw, &prop.data_type)))
    }

    /// Index pairs sourced from the inline column for erase paths.
    ///
    /// Read before the physical removal: the columnar `read_properties_*`
    /// helpers see no rows on inline tables, while the value column still
    /// holds the last written word.
    pub(super) fn bundled_index_pairs_for_erase(
        &self,
        src: u32,
        edge_id: EdgeId,
    ) -> Vec<(String, Value)> {
        let Some(prop) = self.schema.properties.first() else {
            return Vec::new();
        };
        match self.out_csr.bundled_value_at(src, edge_id) {
            Some((raw, true)) => vec![(prop.name.clone(), decode_scalar(raw, &prop.data_type))],
            _ => Vec::new(),
        }
    }

    /// Bundled counterpart of [`Self::apply_staged_insert`]: the single
    /// scalar rides the CSR value column in both directions and the
    /// columnar store is never touched.
    ///
    /// The value is stored twice on purpose: each direction must answer
    /// value reads from its own row walk without a cross-direction hop, so
    /// halving the storage would trade one cheap duplicate write for a hop
    /// on every reverse traversal. The duplication is kept deliberately.
    pub(super) fn apply_staged_insert_bundled(
        &mut self,
        src: u32,
        dst: u32,
        rank: i64,
        property_values: &[(String, Value)],
        ts: Timestamp,
    ) -> StorageResult<EdgeId> {
        let inline_value = self.convert_bundled_value(property_values)?;
        let edge_id = self.next_edge_id.fetch_add();

        self.mvcc.record_creation(edge_id, ts);

        let dst_key = Self::edge_endpoint_key(dst, rank);
        let src_key = Self::edge_endpoint_key(src, rank);
        if let Err(e) =
            self.out_csr
                .bundled_insert_with_value(src, dst_key, edge_id, ts, inline_value)
        {
            self.mvcc.remove_edge_timestamps(edge_id);
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if let Err(e) =
            self.in_csr
                .bundled_insert_with_value(dst, src_key, edge_id, ts, inline_value)
        {
            if !self.out_csr.rollback_insert(src, edge_id) {
                let _ = self.out_csr.delete_edge(src, edge_id, ts);
            }
            self.mvcc.remove_edge_timestamps(edge_id);
            self.debug_assert_copies_consistent(edge_id);
            return Err(e);
        }

        if self.property_index.is_some() {
            let label = self.label;
            let pair = self.bundled_index_pair(inline_value);
            let outcomes: Vec<(String, StorageResult<()>, u64)> =
                if let Some(ref mut index) = self.property_index {
                    pair.into_iter()
                        .map(|(prop_name, prop_value)| {
                            let started = std::time::Instant::now();
                            let result =
                                index.insert(&prop_name, &prop_value, src, dst, rank, label, ts);
                            let latency = started.elapsed().as_millis() as u64;
                            (prop_name, result, latency)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
            for (prop_name, result, latency) in outcomes {
                self.note_index_result(&prop_name, result, latency);
            }
        }

        self.mark_properties_dirty();
        self.edge_owner
            .insert(edge_id, self.owner_gid_for(src, dst));
        self.debug_assert_copies_consistent(edge_id);
        Ok(edge_id)
    }

    /// Bundled point write: encode the scalar and store it in both
    /// directions' value columns. The property name must be the table's
    /// single inline property.
    pub(super) fn write_bundled_property(
        &mut self,
        src: u32,
        dst: u32,
        prop_name: &str,
        value: &Value,
    ) -> StorageResult<()> {
        let Some(def) = self.schema.properties.first() else {
            return Err(StorageError::column_not_found(prop_name.to_string()));
        };
        if prop_name != def.name {
            return Err(StorageError::column_not_found(prop_name.to_string()));
        }
        let cast = if value.data_type() != def.data_type {
            value.try_cast_to(&def.data_type)?
        } else {
            value.clone()
        };
        let inline_value = match cast {
            Value::Null(_) | Value::Empty => None,
            _ => Some(encode_scalar(&cast)),
        };
        // Row endpoints, not key halves: bundled rows are keyed by the raw
        // vertex id with rank pinned to zero.
        if !self
            .out_csr
            .bundled_set_value_by_endpoint(src, dst, inline_value)
        {
            return Err(StorageError::column_not_found(prop_name.to_string()));
        }
        if !self
            .in_csr
            .bundled_set_value_by_endpoint(dst, src, inline_value)
        {
            return Err(StorageError::data_corruption(format!(
                "bundled in-direction value missing for edge ({}, {})",
                src, dst
            )));
        }
        Ok(())
    }
}
