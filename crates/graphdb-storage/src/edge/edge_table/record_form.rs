//! Offline record-form migration (single direction, fail-closed).
//!
//! `migrate_record_form` moves every authority-live edge into fresh shard
//! sets of the target form while preserving edge ids, both directions and
//! the secondary index keys. Authority-deleted edges are dropped together
//! with their timestamp records, mirroring compaction. Nothing is swapped
//! until the whole rebuild validates: any bad input (nonzero rank for an
//! inline form, arity or type mismatch, corrupt payload data surfacing as
//! topology errors) aborts with the table untouched. Checkpoint after a
//! successful migration; WAL redo from before the migration must not replay
//! on top of the new form.

use super::core::owner::EdgeOwnerMap;
use super::core::EdgeStore;
use crate::edge::bundled_csr::{decode_scalar, encode_scalar};
use crate::edge::property_schema::PropertySchema;
use crate::edge::{
    is_scalar_encodable, CsrShardSet, CsrWithProperties, MutableCsrTrait, RecordForm,
};
use graphdb_core::types::{EdgeId, Timestamp};
use graphdb_core::{StorageError, StorageResult, Value};

/// Outcome of one offline record-form migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrateStats {
    /// Authority-live edges carried into the new form.
    pub edges_moved: u64,
    /// Groups in the rebuilt directions.
    pub groups_rebuilt: usize,
}

struct LiveEdge {
    row: u32,
    endpoint: u32,
    rank: i64,
    edge_id: EdgeId,
    create_ts: Timestamp,
    props: Vec<(String, Value)>,
}

impl EdgeStore {
    /// Migrate this table to another record form offline.
    ///
    /// Requires exclusive access and a checkpoint afterwards. Migrating to
    /// the current form is a no-op success.
    pub fn migrate_record_form(&mut self, target: RecordForm) -> StorageResult<MigrateStats> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let current = self.schema.record_form;
        if current == target {
            return Ok(MigrateStats {
                edges_moved: 0,
                groups_rebuilt: 0,
            });
        }
        match target {
            RecordForm::Pure if !self.schema.properties.is_empty() => {
                return Err(StorageError::invalid_operation(
                    "pure record form requires zero properties".to_string(),
                ));
            }
            RecordForm::Bundled => {
                if self.schema.properties.len() != 1 {
                    return Err(StorageError::invalid_operation(
                        "bundled record form requires exactly one property".to_string(),
                    ));
                }
                if !is_scalar_encodable(&self.schema.properties[0].data_type) {
                    return Err(StorageError::invalid_operation(format!(
                        "property type {:?} cannot inline into the bundled form",
                        self.schema.properties[0].data_type
                    )));
                }
            }
            RecordForm::Pure | RecordForm::Columnar => {}
        }

        // Extract and validate everything before touching live state.
        let (out_live, mut dropped) = self.extract_live_edges(true, current, target)?;
        let (in_live, mut dropped_in) = self.extract_live_edges(false, current, target)?;
        dropped.append(&mut dropped_in);

        let mut out_new = CsrShardSet::new(
            self.schema.oe_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
            target,
        )?;
        let mut in_new = CsrShardSet::new(
            self.schema.ie_strategy,
            self.config.node_group_bits,
            self.config.overflow_chunk_edges,
            target,
        )?;
        let prop_schemas: Vec<PropertySchema> = self
            .schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
                    .with_default_value(p.default_value.clone())
            })
            .collect();
        let mut properties_new = match target {
            RecordForm::Pure | RecordForm::Bundled => CsrWithProperties::inline_stub(prop_schemas),
            RecordForm::Columnar => CsrWithProperties::new(prop_schemas),
        };
        let mut owner_new = EdgeOwnerMap::new();
        let mut moved = 0u64;

        for edge in &out_live {
            let dst_key = Self::edge_endpoint_key(edge.endpoint, edge.rank);
            if target == RecordForm::Bundled {
                let inline_value = edge.props.first().map(|(_, v)| encode_scalar(v));
                out_new.bundled_insert_with_value(
                    edge.row,
                    dst_key,
                    edge.edge_id,
                    edge.create_ts,
                    inline_value,
                )?;
            } else {
                out_new.insert_edge(edge.row, dst_key, edge.edge_id, edge.create_ts)?;
            }
            if target == RecordForm::Columnar {
                let converted = self.convert_property_values(&edge.props)?;
                properties_new.insert_for_edge_at(edge.edge_id, &converted, edge.create_ts)?;
            }
            owner_new.insert(edge.edge_id, self.owner_gid_for(edge.row, edge.endpoint));
            moved += 1;
        }
        for edge in &in_live {
            let dst_key = Self::edge_endpoint_key(edge.endpoint, edge.rank);
            if target == RecordForm::Bundled {
                let inline_value = edge.props.first().map(|(_, v)| encode_scalar(v));
                in_new.bundled_insert_with_value(
                    edge.row,
                    dst_key,
                    edge.edge_id,
                    edge.create_ts,
                    inline_value,
                )?;
            } else {
                in_new.insert_edge(edge.row, dst_key, edge.edge_id, edge.create_ts)?;
            }
        }

        for edge_id in dropped {
            self.mvcc.remove_edge_timestamps(edge_id);
        }

        let groups_rebuilt = out_new.group_count() + in_new.group_count();
        self.out_csr = out_new;
        self.in_csr = in_new;
        self.schema.record_form = target;
        self.properties = properties_new;
        self.edge_owner = owner_new;
        self.segment_stats.clear();
        self.mark_properties_dirty();
        self.out_csr.mark_all_dirty();
        self.in_csr.mark_all_dirty();
        self.out_csr.clear_all_append_logs();
        self.in_csr.clear_all_append_logs();
        self.out_csr.truncate_trailing_empty_groups();
        self.in_csr.truncate_trailing_empty_groups();
        // The secondary index keys (src, dst, rank) never change across
        // forms, and migrated edges keep their ids, so no rebuild is needed.
        // New-table stats flow through the regular maintenance paths.
        self.debug_assert_migrated(target);
        Ok(MigrateStats {
            edges_moved: moved,
            groups_rebuilt,
        })
    }

    /// Collect every authority-live physical edge of one direction.
    ///
    /// Holes and authority-deleted edges never migrate; the latter are
    /// reported for timestamp cleanup. Validation against the target form
    /// fails the whole migration with no partial state.
    fn extract_live_edges(
        &self,
        outgoing: bool,
        current: RecordForm,
        target: RecordForm,
    ) -> StorageResult<(Vec<LiveEdge>, Vec<EdgeId>)> {
        let shards = if outgoing {
            &self.out_csr
        } else {
            &self.in_csr
        };
        let mut live = Vec::new();
        let mut dropped = Vec::new();
        for gid in shards.existing_group_ids() {
            let base = crate::edge::node_group::group_base(gid, shards.group_bits()) as u32;
            let Some(variant) = shards.group_variant(gid) else {
                continue;
            };
            for (local_vid, nbr) in variant.iter_all() {
                if nbr.edge_id == crate::edge::INVALID_EDGE_ID {
                    continue;
                }
                match self.mvcc.edge_timestamps.get(&nbr.edge_id) {
                    Some(info) if info.delete_ts != Timestamp::MAX => {
                        dropped.push(nbr.edge_id);
                        continue;
                    }
                    _ => {}
                }
                if target != RecordForm::Columnar && nbr.rank != 0 {
                    return Err(StorageError::invalid_operation(format!(
                        "nonzero rank {} cannot migrate to {:?}",
                        nbr.rank, target
                    )));
                }
                let row = base + local_vid.as_int64().unwrap_or(0) as u32;
                let create_ts = self
                    .mvcc
                    .edge_timestamps
                    .get(&nbr.edge_id)
                    .map(|info| info.create_ts)
                    .unwrap_or(nbr.create_ts);
                let props = self.extract_edge_props(shards, row, nbr.edge_id, current, target)?;
                live.push(LiveEdge {
                    row,
                    endpoint: nbr.endpoint,
                    rank: nbr.rank,
                    edge_id: nbr.edge_id,
                    create_ts,
                    props,
                });
            }
        }
        Ok((live, dropped))
    }

    /// Resolve one edge's properties from the current form, validating
    /// against the target so data loss fails loudly instead of silently.
    fn extract_edge_props(
        &self,
        shards: &CsrShardSet,
        row: u32,
        edge_id: EdgeId,
        current: RecordForm,
        target: RecordForm,
    ) -> StorageResult<Vec<(String, Value)>> {
        let props = match current {
            RecordForm::Pure => Vec::new(),
            RecordForm::Columnar => self
                .properties
                .read_properties_by_edge_id(edge_id)
                .unwrap_or_default(),
            RecordForm::Bundled => match shards.bundled_value_at(row, edge_id) {
                Some((raw, true)) => {
                    let def = self.schema.properties.first().ok_or_else(|| {
                        StorageError::data_corruption(format!(
                            "bundled value without schema property for edge {:?}",
                            edge_id
                        ))
                    })?;
                    vec![(def.name.clone(), decode_scalar(raw, &def.data_type))]
                }
                _ => Vec::new(),
            },
        };
        match target {
            RecordForm::Pure if !props.is_empty() => Err(StorageError::invalid_operation(format!(
                "edge {:?} carries properties into the pure form",
                edge_id
            ))),
            RecordForm::Bundled if props.len() > 1 => {
                Err(StorageError::invalid_operation(format!(
                    "edge {:?} carries multiple properties into the bundled form",
                    edge_id
                )))
            }
            RecordForm::Bundled => {
                if let Some((name, _)) = props.first() {
                    let def = self.schema.properties.first().ok_or_else(|| {
                        StorageError::data_corruption(format!(
                            "bundled target without schema property for edge {:?}",
                            edge_id
                        ))
                    })?;
                    if name != &def.name {
                        return Err(StorageError::data_corruption(format!(
                            "property name mismatch into the bundled form: {}",
                            name
                        )));
                    }
                }
                Ok(props)
            }
            RecordForm::Pure | RecordForm::Columnar => Ok(props),
        }
    }

    /// Debug-only post-migration invariant: every moved edge resolves
    /// through the authority and the new topology.
    fn debug_assert_migrated(&self, _target: RecordForm) {
        for gid in self.out_csr.existing_group_ids() {
            if let Some(variant) = self.out_csr.group_variant(gid) {
                for (_, nbr) in variant.iter_all() {
                    if nbr.edge_id == crate::edge::INVALID_EDGE_ID {
                        continue;
                    }
                    debug_assert!(
                        self.mvcc.edge_timestamps.contains_key(&nbr.edge_id),
                        "migrated edge without authority entry"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordFormPreference};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;

    fn weight_schema() -> EdgeSchema {
        EdgeSchema {
            label_id: 0,
            label_name: "rates".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "weight".to_string(),
                data_type: DataType::Double,
                nullable: false,
                default_value: Some(Value::Double(0.0)),
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        }
    }

    fn auto_config() -> EdgeTableConfig {
        let mut config = EdgeTableConfig::default();
        config.record_form = RecordFormPreference::Auto;
        config
    }

    fn make_bundled_table() -> EdgeStore {
        EdgeStore::with_config(weight_schema(), auto_config()).expect("bundled table builds")
    }

    fn make_columnar_table() -> EdgeStore {
        EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds")
    }

    #[test]
    fn auto_selects_bundled_with_inline_stub() {
        let table = make_bundled_table();
        assert_eq!(table.schema.record_form, RecordForm::Bundled);
        assert!(table.out_csr.is_bundled());
        assert!(table.in_csr.is_bundled());
        assert!(table.properties.is_inline_stub());
        assert!(table
            .out_csr
            .group_variant(0)
            .is_some_and(|v| v.is_bundled()));
    }

    #[test]
    fn bundled_insert_read_update_delete() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("bundled insert");
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .expect("bundled insert");
        table
            .insert_edge(0, 3, 0, &[], 100)
            .expect("bundled NULL insert");

        // Point reads bypass the columnar store.
        let edge = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
        let null_edge = table.get_edge(0, 3, 0, 200).expect("edge present");
        assert!(null_edge.properties.is_empty());

        // Row scans decode both directions from the value columns.
        let mut out: Vec<(u32, Vec<(String, Value)>)> = table
            .out_edges(0, 200)
            .into_iter()
            .map(|e| (e.dst_vid.as_int64().unwrap_or(-1) as u32, e.properties))
            .collect();
        out.sort_by_key(|(dst, _)| *dst);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].1, vec![("weight".to_string(), Value::Double(1.5))]);

        // Point writes land in both directions.
        assert!(table
            .update_edge_property(0, 1, 0, "weight", &Value::Double(9.25), 300)
            .expect("update"));
        let updated = table.get_edge(0, 1, 0, 400).expect("edge present");
        assert_eq!(
            updated.properties,
            vec![("weight".to_string(), Value::Double(9.25))]
        );
        let in_edges = table.in_edges(1, 400);
        assert_eq!(in_edges.len(), 1);
        assert_eq!(
            in_edges[0].properties,
            vec![("weight".to_string(), Value::Double(9.25))]
        );

        // Deletes drop rows; the surviving set stays aligned.
        assert!(table.delete_edge(0, 2, 0, 500).expect("delete"));
        assert!(table.get_edge(0, 2, 0, 600).is_none());
        assert_eq!(table.out_edges(0, 600).len(), 2);
        let survivor = table.get_edge(0, 1, 0, 600).expect("survivor");
        assert_eq!(
            survivor.properties,
            vec![("weight".to_string(), Value::Double(9.25))]
        );
    }

    #[test]
    fn bundled_rejects_rank_and_duplicates() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert");
        assert!(table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(2.0))], 100)
            .is_err());
        assert!(table
            .insert_edge(0, 2, 3, &[("weight".to_string(), Value::Double(2.0))], 100)
            .is_err());
    }

    #[test]
    fn bundled_projection_filters_single_property() {
        use crate::mvcc_visibility::PendingGate;
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert");
        let vm = graphdb_transaction::VersionManager::new();
        let gate = PendingGate::new(&vm, None);
        let all = table
            .get_edge_with_gate_projected(0, 1, 0, 200, &gate, None)
            .expect("edge present");
        assert_eq!(all.properties.len(), 1);
        let none = table
            .get_edge_with_gate_projected(0, 1, 0, 200, &gate, Some(&[]))
            .expect("edge present");
        assert!(none.properties.is_empty());
        let missing = table
            .get_edge_with_gate_projected(0, 1, 0, 200, &gate, Some(&["nope".to_string()]))
            .expect("edge present");
        assert!(missing.properties.is_empty());
    }

    #[test]
    fn bundled_schema_changes_require_offline_rebuild() {
        let mut table = make_bundled_table();
        assert!(table
            .prepare_add_property("extra".to_string(), DataType::Double, true, None)
            .is_err());
        assert!(table.prepare_drop_property("weight").is_err());
    }

    #[test]
    fn bundled_freeze_rejects_valued_groups() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert");
        assert!(table
            .freeze_group(true, 0, graphdb_core::types::Timestamp::MAX, 0.0)
            .is_err());

        let mut nulls = make_bundled_table();
        nulls.insert_edge(0, 1, 0, &[], 100).expect("insert");
        nulls
            .freeze_group(true, 0, graphdb_core::types::Timestamp::MAX, 0.0)
            .expect("all-NULL freeze packs topology");
        nulls.unfreeze_group(true, 0).expect("unfreeze restores");
        assert_eq!(nulls.out_edges(0, 200).len(), 1);
    }

    #[test]
    fn bundled_checkpoint_roundtrip_preserves_values() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        table.insert_edge(0, 2, 0, &[], 100).expect("insert");
        table.delete_edge(5, 6, 0, 150).expect("no-op delete");
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush succeeds");
        let mut loaded = make_bundled_table();
        loaded.load(dir.path()).expect("load succeeds");
        assert_eq!(loaded.schema.record_form, RecordForm::Bundled);
        let edge = loaded.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
        assert_eq!(loaded.out_edges(0, 200).len(), 2);
        // Writes continue on the loaded form.
        loaded
            .update_edge_property(0, 1, 0, "weight", &Value::Double(3.25), 300)
            .expect("update after load");
        let updated = loaded.get_edge(0, 1, 0, 400).expect("edge present");
        assert_eq!(
            updated.properties,
            vec![("weight".to_string(), Value::Double(3.25))]
        );
    }

    #[test]
    fn migrate_columnar_bundled_roundtrip() {
        let mut table = make_columnar_table();
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        table
            .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .expect("insert");

        let stats = table
            .migrate_record_form(RecordForm::Bundled)
            .expect("migrate to bundled");
        assert_eq!(stats.edges_moved, 2);
        assert_eq!(table.schema.record_form, RecordForm::Bundled);
        assert!(table.properties.is_inline_stub());
        let edge = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
        // Same-form migration is a no-op success.
        let noop = table
            .migrate_record_form(RecordForm::Bundled)
            .expect("no-op");
        assert_eq!(noop.edges_moved, 0);

        let back = table
            .migrate_record_form(RecordForm::Columnar)
            .expect("migrate back");
        assert_eq!(back.edges_moved, 2);
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        let restored = table.get_edge(2, 3, 0, 200).expect("edge present");
        assert_eq!(
            restored.properties,
            vec![("weight".to_string(), Value::Double(2.5))]
        );
    }

    #[test]
    fn migrate_rejects_illegal_targets() {
        let mut table = make_columnar_table();
        assert!(table.migrate_record_form(RecordForm::Pure).is_err());
        assert_eq!(table.schema.record_form, RecordForm::Columnar);

        let schema = EdgeSchema {
            properties: vec![
                StoragePropertyDef {
                    name: "a".to_string(),
                    data_type: DataType::Double,
                    nullable: true,
                    default_value: None,
                },
                StoragePropertyDef {
                    name: "b".to_string(),
                    data_type: DataType::Double,
                    nullable: true,
                    default_value: None,
                },
            ],
            ..weight_schema()
        };
        let mut two = EdgeStore::with_config(schema, EdgeTableConfig::default())
            .expect("two-column table builds");
        assert!(two.migrate_record_form(RecordForm::Bundled).is_err());
    }

    #[test]
    fn bundled_pushdown_matches_inline_values() {
        use crate::cursor::ScanPredicate;
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .expect("insert");
        let equal = vec![ScanPredicate::ColumnEqual {
            column: "weight".to_string(),
            value: Value::Double(1.5),
        }];
        assert!(table.matches_pushdown(EdgeId(0), 200, &equal));
        assert!(!table.matches_pushdown(EdgeId(1), 200, &equal));
        let hits = table.filter_edge_ids(&equal, 200, None);
        assert_eq!(hits, vec![EdgeId(0)]);
        let missing = vec![ScanPredicate::ColumnEqual {
            column: "nope".to_string(),
            value: Value::Double(1.5),
        }];
        assert!(table.filter_edge_ids(&missing, 200, None).is_empty());
    }
}
