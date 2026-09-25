//! Automatic record-form selection for edge tables.
//!
//! Creation-time `Auto` only derives the safe defaults (pure for empty
//! schemas, columnar otherwise) and never picks the inline form, so narrow
//! single-scalar tables stay columnar until an operator acts. This module
//! closes that loop: [`EdgeStore::recommended_record_form`] folds shape
//! admission plus the observed width and access profile into the
//! recommendation, and [`EdgeStore::auto_migrate_record_form_if_beneficial`]
//! performs the online switch when the recommendation differs.
//!
//! Automatic invocation from background maintenance is opt-in only
//! (`EdgeTableConfig::auto_migrate_record_form`, default off): every switch
//! fences pre-switch WAL redo and arms the mandatory checkpoint, fencing
//! writes until the checkpoint lands. The background caller runs the returned
//! switch and the regular flush then checkpoints; silent always-on switches
//! would stall the write path, so operators enable this knowingly and manual
//! migration remains the primary path.

use super::core::EdgeStore;
use super::record_form::MigrateStats;
use crate::edge::{is_bundled_eligible, RecordForm};
use graphdb_core::StorageResult;

impl EdgeStore {
    /// Recommend the record form for the current schema, strategies and
    /// observed profile.
    ///
    /// Shape stays the veto: empty property sets on multi-edge legs stay
    /// pure, ineligible shapes stay columnar, and single-edge directions
    /// always resolve to columnar because only that form provides fixed
    /// single slots. Eligible single scalars then consult the profile: width
    /// over the slot limit or write-hot tables stay columnar, only narrow
    /// read-heavy tables recommend bundled. Unknown profiles stay columnar
    /// so old checkpoints without a snapshot never mis-trigger a migration.
    pub fn recommended_record_form(&self) -> RecordForm {
        if self.schema.properties.is_empty()
            && !matches!(self.schema.oe_strategy, crate::edge::EdgeStrategy::Single)
            && !matches!(self.schema.ie_strategy, crate::edge::EdgeStrategy::Single)
        {
            return RecordForm::Pure;
        }
        if !is_bundled_eligible(
            &self.schema.properties,
            self.schema.oe_strategy,
            self.schema.ie_strategy,
        ) {
            return RecordForm::Columnar;
        }
        let snapshot = self.form_profile_snapshot();
        if snapshot.is_unknown() {
            return RecordForm::Columnar;
        }
        let thresholds = self.config.record_form_profile;
        if let Some(avg) = snapshot.avg_width_bytes() {
            if avg > thresholds.max_inline_avg_bytes as f64 {
                return RecordForm::Columnar;
            }
        }
        if let Some(share) = snapshot.write_share() {
            if share > thresholds.max_write_share {
                return RecordForm::Columnar;
            }
        }
        if snapshot.writes > 0 {
            let ratio = snapshot.reads as f64 / snapshot.writes.max(1) as f64;
            if ratio < thresholds.min_read_to_write_ratio {
                return RecordForm::Columnar;
            }
        } else if snapshot.reads == 0 {
            return RecordForm::Columnar;
        }
        RecordForm::Bundled
    }

    /// Switch to the recommended form when it differs from the current one.
    ///
    /// Explicit opt-in entry: returns `None` when the table already holds the
    /// recommended form, otherwise runs the same online switch as the manual
    /// path (pure rebuild, WAL fence, mandatory checkpoint afterwards).
    /// Guard failures (pending schema change, pending checkpoint, illegal
    /// target for the live data) propagate as errors with the table
    /// untouched. The caller must checkpoint after a successful switch.
    /// Hysteresis: automatic migration only moves columnar to bundled; a
    /// bundled table never auto-reverses on profile noise and needs a manual
    /// migration back to columnar.
    pub fn auto_migrate_record_form_if_beneficial(
        &mut self,
    ) -> StorageResult<Option<MigrateStats>> {
        let target = self.recommended_record_form();
        if target == self.schema.record_form {
            return Ok(None);
        }
        if self.schema.record_form == RecordForm::Bundled {
            return Ok(None);
        }
        if target != RecordForm::Bundled {
            return Ok(None);
        }
        // Quote cost first so illegal targets fail with the plan wording
        // before paying the rebuild.
        let _ = self.migration_plan(target)?;
        let stats = self.switch_record_form_online(target)?;
        self.last_auto_migrated_to_bundled = true;
        Ok(Some(stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::edge_table::config::EdgeTableConfig;
    use crate::edge::{EdgeSchema, EdgeStrategy, RecordFormPreference};
    use crate::types::StoragePropertyDef;
    use graphdb_core::types::DataType;
    use graphdb_core::Value;

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

    #[test]
    fn recommender_covers_all_schema_shapes() {
        let columnar = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        assert_eq!(columnar.schema.record_form, RecordForm::Columnar);
        // Unknown profile stays columnar instead of mis-triggering inline.
        assert_eq!(columnar.recommended_record_form(), RecordForm::Columnar);

        let mut empty = weight_schema();
        empty.properties.clear();
        let pure =
            EdgeStore::with_config(empty, EdgeTableConfig::default()).expect("empty table builds");
        assert_eq!(pure.recommended_record_form(), RecordForm::Pure);

        let mut two = weight_schema();
        two.properties.push(StoragePropertyDef {
            name: "extra".to_string(),
            data_type: DataType::Double,
            nullable: true,
            default_value: None,
        });
        let wide = EdgeStore::with_config(two, EdgeTableConfig::default())
            .expect("two-column table builds");
        assert_eq!(wide.recommended_record_form(), RecordForm::Columnar);

        let mut single = weight_schema();
        single.oe_strategy = EdgeStrategy::Single;
        single.ie_strategy = EdgeStrategy::Single;
        let single_table = EdgeStore::with_config(single, EdgeTableConfig::default())
            .expect("single table builds");
        assert_eq!(single_table.recommended_record_form(), RecordForm::Columnar);
    }

    fn narrow_read_heavy(table: &EdgeStore) {
        // Narrow Double writes plus read-heavy access drive the bundled
        // recommendation through the real observation entry.
        let _ = table;
    }

    #[test]
    fn recommender_needs_narrow_read_heavy_profile() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        assert_eq!(table.recommended_record_form(), RecordForm::Columnar);
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        // One write without reads is write-hot, so it stays columnar.
        assert_eq!(table.recommended_record_form(), RecordForm::Columnar);
        for _ in 0..4 {
            table.observe_form_read(1);
        }
        // Narrow plus read-heavy now recommends bundled.
        assert_eq!(table.recommended_record_form(), RecordForm::Bundled);
        narrow_read_heavy(&table);
    }

    #[test]
    fn recommender_holds_columnar_on_wide_profile() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "wide".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![StoragePropertyDef {
                name: "note".to_string(),
                data_type: DataType::String,
                nullable: false,
                default_value: None,
            }],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };
        // String is not bundled-eligible, so shape alone keeps columnar.
        let table =
            EdgeStore::with_config(schema, EdgeTableConfig::default()).expect("table builds");
        assert_eq!(table.recommended_record_form(), RecordForm::Columnar);
    }

    #[test]
    fn auto_migrate_switches_narrow_columnar_to_bundled() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        // Drive the profile read-heavy through the observation entry.
        for _ in 0..4 {
            table.observe_form_read(1);
        }
        let stats = table
            .auto_migrate_record_form_if_beneficial()
            .expect("auto migrate")
            .expect("migration ran");
        assert_eq!(stats.edges_moved, 1);
        assert_eq!(table.schema.record_form, RecordForm::Bundled);
        assert!(table.is_migration_checkpoint_required());
        let edge = table.get_edge(0, 1, 0, 200).expect("edge present");
        assert_eq!(
            edge.properties,
            vec![("weight".to_string(), Value::Double(1.5))]
        );
    }

    #[test]
    fn auto_migrate_is_noop_on_recommended_form() {
        let mut table = EdgeStore::with_config(
            weight_schema(),
            EdgeTableConfig {
                record_form: RecordFormPreference::Bundled,
                ..Default::default()
            },
        )
        .expect("bundled table builds");
        assert!(table
            .auto_migrate_record_form_if_beneficial()
            .expect("noop")
            .is_none());
        assert!(!table.is_migration_checkpoint_required());
    }

    #[test]
    fn auto_migrate_refuses_with_pending_schema_change() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        for _ in 0..4 {
            table.observe_form_read(1);
        }
        assert_eq!(table.recommended_record_form(), RecordForm::Bundled);
        table
            .prepare_add_property("extra".to_string(), DataType::Double, true, None)
            .expect("prepare add");
        assert!(table.auto_migrate_record_form_if_beneficial().is_err());
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
    }

    #[test]
    fn auto_migrate_holds_on_unknown_and_write_hot() {
        let mut unknown = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        unknown
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        // Unknown read profile is write-hot, so no migration runs.
        assert!(unknown
            .auto_migrate_record_form_if_beneficial()
            .expect("noop")
            .is_none());
        assert_eq!(unknown.schema.record_form, RecordForm::Columnar);
    }

    #[test]
    fn auto_migrate_never_reverses_bundled() {
        let mut table = EdgeStore::with_config(
            weight_schema(),
            EdgeTableConfig {
                record_form: RecordFormPreference::Bundled,
                ..Default::default()
            },
        )
        .expect("bundled table builds");
        // Even a write-heavy profile never auto-reverses; manual migration
        // stays the only way back to columnar.
        for _ in 0..10 {
            table.observe_form_write(&[("weight".to_string(), Value::Double(1.0))]);
        }
        assert!(table
            .auto_migrate_record_form_if_beneficial()
            .expect("noop")
            .is_none());
        assert_eq!(table.schema.record_form, RecordForm::Bundled);
    }

    #[test]
    fn migration_plan_reports_profile_basis() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        let unknown_plan = table
            .migration_plan(RecordForm::Bundled)
            .expect("plan succeeds");
        assert!(unknown_plan.basis.contains("unknown"));
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        for _ in 0..4 {
            table.observe_form_read(1);
        }
        let ready_plan = table
            .migration_plan(RecordForm::Bundled)
            .expect("plan succeeds");
        assert!(ready_plan.basis.contains("avg_width"));
    }

    #[test]
    fn form_profile_survives_checkpoint_as_known() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
        for _ in 0..4 {
            table.observe_form_read(1);
        }
        assert_eq!(table.recommended_record_form(), RecordForm::Bundled);
        let dir = tempfile::tempdir().expect("temporary edge table directory");
        table
            .flush(
                dir.path(),
                crate::compression::CompressionType::Zstd { level: 3 },
            )
            .expect("flush succeeds");
        let mut loaded = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        loaded.load(dir.path()).expect("load succeeds");
        assert!(!loaded.form_profile_snapshot().is_unknown());
        assert_eq!(loaded.recommended_record_form(), RecordForm::Bundled);
    }
}
