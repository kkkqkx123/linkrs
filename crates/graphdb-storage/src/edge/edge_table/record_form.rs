//! Record-form migration: offline checklist plus in-table online switch.
//!
//! `migrate_record_form` (offline) and `switch_record_form_online` share one
//! pure rebuild: every authority-live edge moves into fresh shard sets of the
//! target form while preserving edge ids, both directions and the secondary
//! index keys. Authority-deleted edges are dropped together with their
//! timestamp records, mirroring compaction. Nothing is swapped until the
//! whole rebuild validates: any bad input (nonzero rank for an inline form,
//! arity or type mismatch, corrupt payload data surfacing as topology errors)
//! aborts with the table untouched. Checkpoint after a successful switch;
//! WAL redo from before the switch is fenced at switch time and must never
//! replay on top of the new form.
//!
//! Inline value semantics, pinned here so the query layer never misreads:
//! a bundled delete clears the validity bit while retaining the stale word
//! for the slot that held it. A positional revert holding the pre-delete
//! slot revives the retained word; an id-keyed lookup after an erase-form
//! delete cannot locate the sentinel slot and reports not found. Validity
//! bits persist across checkpoints, so a reload never defaults a blind slot
//! to valid.
//!
//! Online contract: the switch holds `&mut self`, which already serializes
//! every writer in this crate (single-writer discipline), so no concurrent
//! read or write can interleave mid-switch. Reads before the call see the old
//! form, reads after see the new form; there is no close/reopen window. A
//! failed switch returns with the original shards, authority and properties
//! untouched.

use super::core::owner::EdgeOwnerMap;
use super::core::EdgeStore;
use crate::edge::bundled_csr::{decode_scalar, encode_scalar};
use crate::edge::property_schema::PropertySchema;
use crate::edge::{CsrShardSet, CsrWithProperties, MutableCsrTrait, RecordForm};
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

/// Executable pre-switch checklist for one record-form migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MigrationPlan {
    /// Form the table holds now.
    pub current: RecordForm,
    /// Requested form.
    pub target: RecordForm,
    /// Authority-live edges the rebuild will carry over.
    pub live_edges: u64,
    /// Authority-deleted edges the rebuild will drop with cleanup.
    pub dropped_tombstones: u64,
    /// Groups the rebuilt directions will hold.
    pub groups_to_rebuild: usize,
}

struct LiveEdge {
    row: u32,
    endpoint: u32,
    rank: i64,
    edge_id: EdgeId,
    create_ts: Timestamp,
    props: Vec<(String, Value)>,
}

/// Fresh shards, properties and owner map built without touching live state.
///
/// The rebuild is pure until [`EdgeStore::publish_rebuilt_form`]: every
/// fallible step runs here, so any failure aborts with the original table
/// untouched and no rollback compensation is needed.
struct RebuiltForm {
    out_new: CsrShardSet,
    in_new: CsrShardSet,
    properties_new: CsrWithProperties,
    owner_new: EdgeOwnerMap,
    dropped: Vec<EdgeId>,
    stats: MigrateStats,
}

impl EdgeStore {
    /// Migrate this table to another record form offline.
    ///
    /// Requires exclusive access and a checkpoint afterwards. Migrating to
    /// the current form is a no-op success.
    pub fn migrate_record_form(&mut self, target: RecordForm) -> StorageResult<MigrateStats> {
        self.ensure_no_pending_migration()?;
        if self.pending_add_column.is_some()
            || self.pending_drop_column.is_some()
            || self.pending_rename_column.is_some()
        {
            return Err(StorageError::invalid_operation(
                "record-form migration rejects a pending schema change".to_string(),
            ));
        }
        let rebuilt = self.rebuild_record_form(target)?;
        self.publish_rebuilt_form(rebuilt, target)
    }

    /// Switch record forms in-table without closing the table.
    ///
    /// Same pure rebuild as [`Self::migrate_record_form`], run on the live
    /// open table: `&mut self` already serializes every writer, so the switch
    /// is atomic with respect to reads and writes without a close/reopen
    /// window. Reads before the call observe the old form, reads after
    /// observe the new form with identical logical content. A failed switch
    /// leaves the original shards, authority and properties untouched. A
    /// checkpoint afterwards is mandatory
    /// ([`Self::is_migration_checkpoint_required`]); pre-switch WAL redo is
    /// fenced at switch time and never replays onto the new form.
    pub fn switch_record_form_online(&mut self, target: RecordForm) -> StorageResult<MigrateStats> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        // A second switch before the mandatory checkpoint is rejected: the
        // pre-switch base is still the old checkpoint, so another rebuild
        // would fence new-form redo that has no durable base yet. Pending
        // schema changes are rejected because the rebuild reads the published
        // schema and would silently drop staged state.
        self.ensure_no_pending_migration()?;
        if self.pending_add_column.is_some()
            || self.pending_drop_column.is_some()
            || self.pending_rename_column.is_some()
        {
            return Err(StorageError::invalid_operation(
                "record-form switch rejects a pending schema change".to_string(),
            ));
        }
        let rebuilt = self.rebuild_record_form(target)?;
        self.publish_rebuilt_form(rebuilt, target)
    }

    /// Whether a record-form switch completed since the last checkpoint.
    ///
    /// Memory-only fence flag: while set, the table must be checkpointed
    /// before any WAL replay or further switch is trusted. Cleared by flush
    /// and load, which re-establish the checkpoint base.
    pub fn is_migration_checkpoint_required(&self) -> bool {
        self.migration_pending_checkpoint
    }

    fn ensure_no_pending_migration(&self) -> StorageResult<()> {
        if self.migration_pending_checkpoint {
            return Err(StorageError::invalid_operation(
                "record-form switch requires a checkpoint before further switches or writes"
                    .to_string(),
            ));
        }
        Ok(())
    }

    /// Executable pre-switch checklist: prechecks, cost estimate and the
    /// mandatory-checkpoint requirement, computed without touching live
    /// state. A failing plan returns the same error the switch would, so
    /// callers can quote cost and abort before paying the rebuild.
    pub fn migration_plan(&self, target: RecordForm) -> StorageResult<MigrationPlan> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let current = self.schema.record_form;
        self.check_record_form_target(target)?;
        if current == target {
            return Ok(MigrationPlan {
                current,
                target,
                live_edges: 0,
                dropped_tombstones: 0,
                groups_to_rebuild: 0,
            });
        }
        let (out_live, mut dropped) = self.extract_live_edges(true, current, target)?;
        let (in_live, mut dropped_in) = self.extract_live_edges(false, current, target)?;
        dropped.append(&mut dropped_in);
        // Both directions carry the same logical edges; the out side quotes
        // the move cost, and dropped tombstones dedupe by edge id so one
        // deleted edge counts once, not once per direction.
        let live_edges = out_live.len() as u64;
        let _ = in_live;
        dropped.sort_unstable();
        dropped.dedup();
        let groups_to_rebuild = self.out_csr.group_count() + self.in_csr.group_count();
        Ok(MigrationPlan {
            current,
            target,
            live_edges,
            dropped_tombstones: dropped.len() as u64,
            groups_to_rebuild,
        })
    }

    /// Target prechecks shared by the plan and the rebuild: arity, scalar
    /// encodability and rank rules fail before any state is read or written.
    /// Each rejection names the migration entry so callers quote cost with
    /// `migration_plan` instead of treating the error as generic.
    fn check_record_form_target(&self, target: RecordForm) -> StorageResult<()> {
        crate::edge::validate_record_form_target(
            &self.schema.properties,
            self.schema.oe_strategy,
            self.schema.ie_strategy,
            target,
        )
    }

    /// Pure rebuild shared by the offline and online entries: extract,
    /// validate and build the replacement shards without mutating live
    /// state. Every `?` below runs before the publish step, so any failure
    /// aborts with the original table untouched.
    fn rebuild_record_form(&mut self, target: RecordForm) -> StorageResult<RebuiltForm> {
        if !self.is_open {
            return Err(StorageError::storage_not_open());
        }
        let current = self.schema.record_form;
        if current == target {
            return Ok(RebuiltForm {
                out_new: self.out_csr.clone(),
                in_new: self.in_csr.clone(),
                properties_new: self.properties.clone(),
                owner_new: EdgeOwnerMap::new(),
                dropped: Vec::new(),
                stats: MigrateStats {
                    edges_moved: 0,
                    groups_rebuilt: 0,
                },
            });
        }
        self.check_record_form_target(target)?;

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

        let groups_rebuilt = out_new.group_count() + in_new.group_count();
        Ok(RebuiltForm {
            out_new,
            in_new,
            properties_new,
            owner_new,
            dropped,
            stats: MigrateStats {
                edges_moved: moved,
                groups_rebuilt,
            },
        })
    }

    /// Publish a validated rebuild: fence pre-switch WAL redo, then swap shards,
    /// drop cleaned-up authority records and arm the mandatory checkpoint.
    ///
    /// The WAL fence runs before the swap and fails closed: a fence failure
    /// returns with the original shards, authority and properties untouched.
    /// A crash before the mandatory checkpoint recovers to the pre-switch
    /// checkpoint (old form), never to a mixed form.
    fn publish_rebuilt_form(
        &mut self,
        rebuilt: RebuiltForm,
        target: RecordForm,
    ) -> StorageResult<MigrateStats> {
        let RebuiltForm {
            mut out_new,
            mut in_new,
            properties_new,
            owner_new,
            dropped,
            stats,
        } = rebuilt;
        if self.schema.record_form == target {
            return Ok(stats);
        }
        // Fence first so a fence failure leaves live state untouched.
        self.fence_wal_after_migration()?;
        // Preserve the reuse-hint freshness contract across the swap: fresh
        // groups seed from the live cutoffs instead of resetting to disabled.
        out_new.set_tombstone_reuse_cutoff(self.out_csr.tombstone_reuse_cutoff());
        in_new.set_tombstone_reuse_cutoff(self.in_csr.tombstone_reuse_cutoff());
        self.out_csr = out_new;
        self.in_csr = in_new;
        self.schema.record_form = target;
        self.properties = properties_new;
        self.edge_owner = owner_new;
        for edge_id in dropped {
            self.mvcc.remove_edge_timestamps(edge_id);
        }
        self.segment_stats.clear();
        self.property_column_dirt.clear();
        self.mark_properties_dirty();
        self.out_csr.mark_all_dirty();
        self.in_csr.mark_all_dirty();
        self.out_csr.clear_all_append_logs();
        self.in_csr.clear_all_append_logs();
        self.out_csr.truncate_trailing_empty_groups();
        self.in_csr.truncate_trailing_empty_groups();
        self.migration_pending_checkpoint = true;
        // The secondary index keys (src, dst, rank) never change across
        // forms, and migrated edges keep their ids, so no rebuild is needed.
        // New-table stats flow through the regular maintenance paths.
        self.debug_assert_migrated(target);
        Ok(stats)
    }

    /// Fence pre-switch WAL redo so it can never replay onto the new form.
    ///
    /// The switched in-memory state already carries every live edge, while
    /// old redo logged under the previous form could resurrect dropped
    /// tombstones or misread inline encodings. Truncating is fail-safe: a
    /// crash before the mandatory checkpoint recovers to the pre-switch
    /// checkpoint (old form), never to a mixed form. Tables without a WAL
    /// home have nothing to fence.
    fn fence_wal_after_migration(&mut self) -> StorageResult<()> {
        if let Some(dir) = self.wal_dir.clone() {
            super::wal::truncate(&dir)?;
            log::debug!(
                "record-form switch fenced pre-switch WAL redo in {}",
                dir.display()
            );
        }
        Ok(())
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
            let base = crate::edge::node_group::group_base(gid, shards.group_bits());
            let Some(variant) = shards.group_variant(gid) else {
                continue;
            };
            for (local_vid, nbr) in variant.iter_all() {
                if nbr.edge_id == crate::edge::INVALID_EDGE_ID {
                    continue;
                }
                // Authority-total: live edges always hold an authority
                // record (inserts register, reclaim only removes fully
                // collected tombstones). A missing record means queries
                // treat the edge as invisible, so migration fails closed
                // the same way instead of resurrecting it from the row
                // replica. Tombstoned edges are dropped for cleanup.
                let create_ts = match self.mvcc.edge_timestamps.get(&nbr.edge_id) {
                    Some(info) if info.delete_ts != Timestamp::MAX => {
                        dropped.push(nbr.edge_id);
                        continue;
                    }
                    Some(info) => info.create_ts,
                    None => {
                        dropped.push(nbr.edge_id);
                        continue;
                    }
                };
                if target != RecordForm::Columnar && nbr.rank != 0 {
                    return Err(StorageError::invalid_operation(format!(
                        "nonzero rank {} cannot migrate to {:?}; {}",
                        nbr.rank,
                        target,
                        crate::edge::BUNDLED_RANK_REQUIRES_COLUMNAR_MSG
                    )));
                }
                let row = base + local_vid.as_int64().unwrap_or(0) as u32;
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
            RecordForm::Bundled if props.len() > 1 => {
                Err(StorageError::invalid_operation(format!(
                    "edge {:?} carries multiple properties into the bundled form; keep the columnar form",
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

    fn bundled_config() -> EdgeTableConfig {
        EdgeTableConfig {
            record_form: RecordFormPreference::Bundled,
            ..Default::default()
        }
    }

    fn make_bundled_table() -> EdgeStore {
        EdgeStore::with_config(weight_schema(), bundled_config()).expect("bundled table builds")
    }

    fn make_columnar_table() -> EdgeStore {
        EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds")
    }

    #[test]
    fn bundled_eligibility_covers_all_schema_shapes() {
        use crate::edge::{bundled_ineligibility_reason, is_bundled_eligible};
        let prop = |data_type| StoragePropertyDef {
            name: "p".to_string(),
            data_type,
            nullable: false,
            default_value: None,
        };
        // Single encodable scalar on multi-edge legs: eligible.
        assert!(is_bundled_eligible(
            &[prop(DataType::Double)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        ));
        // Zero or multiple properties: arity refusal.
        assert!(!is_bundled_eligible(
            &[],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        ));
        assert!(!is_bundled_eligible(
            &[prop(DataType::Double), prop(DataType::Int)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        ));
        // Non-encodable scalar: type refusal with the migration wording.
        assert!(!is_bundled_eligible(
            &[prop(DataType::String)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        ));
        let reason = bundled_ineligibility_reason(
            &[prop(DataType::String)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
        )
        .expect("reason present");
        assert!(reason.contains("cannot inline into the bundled form"));
        // Single-edge directions: strategy refusal with the shared wording.
        let reason = bundled_ineligibility_reason(
            &[prop(DataType::Double)],
            EdgeStrategy::Single,
            EdgeStrategy::Multiple,
        )
        .expect("reason present");
        assert_eq!(reason, crate::edge::SINGLE_REQUIRES_COLUMNAR_MSG);
        assert!(!is_bundled_eligible(
            &[prop(DataType::Double)],
            EdgeStrategy::Multiple,
            EdgeStrategy::Single,
        ));
        // Creation and migration agree: the plan path rejects what the
        // selector would never pick.
        let mut single = weight_schema();
        single.oe_strategy = EdgeStrategy::Single;
        assert!(bundled_ineligibility_reason(
            &single.properties,
            single.oe_strategy,
            single.ie_strategy,
        )
        .is_some());
    }

    #[test]
    fn explicit_bundled_selects_inline_stub() {
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
    fn auto_never_selects_bundled() {
        // Auto only derives the safe defaults (pure for empty schemas,
        // columnar otherwise): even a bundled-eligible single scalar stays
        // columnar unless the operator opts into the inline form by name.
        let table = EdgeStore::with_config(
            weight_schema(),
            EdgeTableConfig {
                record_form: RecordFormPreference::Auto,
                ..Default::default()
            },
        )
        .expect("auto table builds");
        assert_eq!(table.schema.record_form, RecordForm::Columnar);

        // The explicit opt-in still works for the same schema.
        let bundled = EdgeStore::with_config(weight_schema(), bundled_config())
            .expect("bundled table builds");
        assert_eq!(bundled.schema.record_form, RecordForm::Bundled);

        // The explicit opt-in fails loudly on ineligible schemas instead of
        // falling back: a single-edge direction reports the shared refusal.
        let mut single = weight_schema();
        single.oe_strategy = EdgeStrategy::Single;
        single.ie_strategy = EdgeStrategy::Single;
        assert!(EdgeStore::with_config(single, bundled_config()).is_err());
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
    fn bundled_freeze_preserves_valued_groups() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.0))], 100)
            .expect("insert");
        table
            .freeze_group(true, 0, graphdb_core::types::Timestamp::MAX, 0.0)
            .expect("valued freeze packs topology plus values");
        let edge = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.0))]
        );
        table.unfreeze_group(true, 0).expect("unfreeze restores");
        let live = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(live.properties, edge.properties);

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
        let dir = tempfile::tempdir().expect("temporary edge table directory");
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
        // The migration arms the mandatory-checkpoint fence; flushing the
        // new form lifts it so further switches are legal again.
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("mandatory checkpoint");
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

    #[test]
    fn migration_plan_quotes_cost_without_touching_state() {
        let mut table = make_columnar_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        table
            .insert_edge(2, 3, 0, &[("weight".to_string(), Value::Double(2.5))], 100)
            .expect("insert");
        table.delete_edge(0, 1, 0, 150).expect("delete");

        let plan = table
            .migration_plan(RecordForm::Bundled)
            .expect("plan succeeds");
        assert_eq!(plan.current, RecordForm::Columnar);
        assert_eq!(plan.target, RecordForm::Bundled);
        assert_eq!(plan.live_edges, 1);
        assert_eq!(plan.dropped_tombstones, 1);
        assert!(plan.groups_to_rebuild >= 1);
        // Planning is read-only: reads still see the old form.
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        assert!(table.has_edge(2, 3, 0, 200));
        assert!(!table.has_edge(0, 1, 0, 200));

        // A failing plan reports the same error the switch would, worded by
        // the shared pure-form constant.
        let err = table
            .migration_plan(RecordForm::Pure)
            .expect_err("pure plan must fail");
        assert!(
            err.to_string()
                .contains(crate::edge::PURE_REQUIRES_ZERO_PROPERTIES_MSG),
            "unexpected wording: {}",
            err
        );
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
    }

    #[test]
    fn online_switch_roundtrip_keeps_reads_and_writes() {
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        let mut table = make_columnar_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        assert!(!table.is_migration_checkpoint_required());

        let forward = table
            .switch_record_form_online(RecordForm::Bundled)
            .expect("online switch");
        assert_eq!(forward.edges_moved, 1);
        assert_eq!(table.schema.record_form, RecordForm::Bundled);
        assert!(table.is_migration_checkpoint_required());
        // Reads see the new form with identical content. Writes stay fenced
        // until the mandatory checkpoint persists the switched form.
        let edge = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
        assert!(table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 200)
            .is_err());
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("mandatory checkpoint");
        assert!(!table.is_migration_checkpoint_required());
        table
            .insert_edge(0, 2, 0, &[("weight".to_string(), Value::Double(2.5))], 200)
            .expect("write after checkpoint");
        assert!(table.audit_copy_drift().is_empty());

        let back = table
            .switch_record_form_online(RecordForm::Columnar)
            .expect("switch back");
        assert_eq!(back.edges_moved, 2);
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        let restored = table.get_edge(0, 2, 0, 300).expect("edge present");
        assert_eq!(
            restored.properties,
            vec![("weight".to_string(), Value::Double(2.5))]
        );
        assert!(table.audit_copy_drift().is_empty());
    }

    #[test]
    fn failed_switch_rolls_back_to_original_form() {
        let mut table = make_columnar_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        // Inline forms reject nonzero ranks: the switch must fail with the
        // original form, data and audit untouched.
        table
            .insert_edge(0, 2, 3, &[("weight".to_string(), Value::Double(2.5))], 100)
            .expect("ranked insert");
        assert!(table
            .switch_record_form_online(RecordForm::Bundled)
            .is_err());
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        assert!(!table.is_migration_checkpoint_required());
        assert!(table.has_edge(0, 1, 0, 200));
        assert!(table.has_edge(0, 2, 3, 200));
        let edge = table.get_edge(0, 2, 3, 200).expect("ranked edge intact");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(2.5))]
        );
        assert!(table.audit_copy_drift().is_empty());
    }

    #[test]
    fn switch_fences_old_wal_and_requires_checkpoint() {
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        let mut table = make_columnar_table();
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("checkpoint gives the WAL a home");
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("post-checkpoint write appends WAL");
        assert!(super::super::wal::wal_path(dir.path()).exists());

        table
            .switch_record_form_online(RecordForm::Bundled)
            .expect("switch");
        assert!(table.is_migration_checkpoint_required());
        // Pre-switch redo is fenced: it can never replay onto the new form.
        assert!(!super::super::wal::wal_path(dir.path()).exists());

        // The mandatory checkpoint clears the fence and persists the form.
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("mandatory checkpoint");
        assert!(!table.is_migration_checkpoint_required());
        let mut loaded = make_columnar_table();
        loaded.load(dir.path()).expect("load succeeds");
        assert_eq!(loaded.schema.record_form, RecordForm::Bundled);
        let edge = loaded.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
        assert!(loaded.audit_copy_drift().is_empty());
    }

    #[test]
    fn bundled_delete_drops_row_and_keyed_revert_reports_false() {
        let mut table = make_bundled_table();
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");

        // Erase-on-delete: the row is dropped from both directions, so no
        // id-keyed or endpoint read observes a tombstone slot. The stale
        // word dies with the row; only the authority tombstone remains.
        assert!(table.delete_edge(0, 1, 0, 150).expect("delete"));
        assert!(table.get_edge(0, 1, 0, 200).is_none());
        assert!(table.out_csr.bundled_value_at(0, EdgeId(0)).is_none());
        assert!(table.out_csr.bundled_value_by_endpoint(0, 1).is_none());
        assert!(table.out_edges(0, 200).is_empty());
        assert!(table.in_edges(1, 200).is_empty());

        // Keyed revert cannot revive an erased row: without the edge id no
        // slot is addressable, so undo reports false instead of reviving a
        // wrong generation. Transaction abort of a bundled delete must not
        // rely on keyed revert until deletes preserve ids as tombstones.
        assert!(!table
            .revert_delete_edge(0, 1, 0, 150)
            .expect("revert reports"));
        assert!(table.get_edge(0, 1, 0, 200).is_none());
    }

    #[test]
    fn single_strategy_auto_selects_columnar() {
        // Single strategies need fixed single slots, which only the columnar
        // form provides. Auto selection never picks an inline form: the
        // bundled shape stays behind its explicit opt-in even when the
        // property count would otherwise allow it.
        let mut schema = weight_schema();
        schema.properties.clear();
        schema.oe_strategy = EdgeStrategy::Single;
        schema.ie_strategy = EdgeStrategy::Single;
        let auto = || EdgeTableConfig {
            record_form: RecordFormPreference::Auto,
            ..Default::default()
        };
        let table = EdgeStore::with_config(schema, auto()).expect("single table builds");
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        assert!(table
            .out_csr
            .group_variant(0)
            .is_some_and(|v| matches!(v, crate::edge::CsrVariant::Single(_))));

        // One single direction is enough to lock the whole table: the other
        // leg keeps its own strategy while sharing the columnar form.
        let mut one_sided = weight_schema();
        one_sided.properties.clear();
        one_sided.oe_strategy = EdgeStrategy::Single;
        one_sided.ie_strategy = EdgeStrategy::Multiple;
        let table = EdgeStore::with_config(one_sided, auto()).expect("one-sided table builds");
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        assert!(table
            .out_csr
            .group_variant(0)
            .is_some_and(|v| matches!(v, crate::edge::CsrVariant::Single(_))));
        assert!(table
            .in_csr
            .group_variant(0)
            .is_some_and(|v| matches!(v, crate::edge::CsrVariant::Multiple(_))));

        // A single encodable scalar is bundled-eligible but stays columnar
        // under Auto: the inline form needs the explicit opt-in.
        let mut scalar = weight_schema();
        scalar.oe_strategy = EdgeStrategy::Single;
        scalar.ie_strategy = EdgeStrategy::Single;
        let table = EdgeStore::with_config(scalar, auto()).expect("single scalar builds");
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
        assert!(table
            .out_csr
            .group_variant(0)
            .is_some_and(|v| matches!(v, crate::edge::CsrVariant::Single(_))));
    }

    #[test]
    fn single_strategy_rejects_inline_migration() {
        let mut schema = weight_schema();
        schema.oe_strategy = EdgeStrategy::Single;
        schema.ie_strategy = EdgeStrategy::Single;
        let mut table =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("columnar builds");
        assert!(table.migration_plan(RecordForm::Pure).is_err());
        assert!(table.migrate_record_form(RecordForm::Pure).is_err());
        assert!(table.migration_plan(RecordForm::Bundled).is_err());
        assert!(table.migrate_record_form(RecordForm::Bundled).is_err());
        assert!(table.switch_record_form_online(RecordForm::Pure).is_err());
        assert!(table
            .switch_record_form_online(RecordForm::Bundled)
            .is_err());
        assert_eq!(table.schema.record_form, RecordForm::Columnar);

        // Multiple strategies stay migratable: the guard only fires on a
        // single direction, never on multi-edge tables.
        let multi = make_columnar_table();
        assert!(multi.migration_plan(RecordForm::Bundled).is_ok());
    }
}
