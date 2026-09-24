//! Automatic record-form selection for edge tables.
//!
//! Creation-time `Auto` only derives the safe defaults (pure for empty
//! schemas, columnar otherwise) and never picks the inline form, so narrow
//! single-scalar tables stay columnar until an operator acts. This module
//! closes that loop: [`EdgeStore::recommended_record_form`] folds the
//! bundled-eligibility check into the recommendation, and
//! [`EdgeStore::auto_migrate_record_form_if_beneficial`] performs the online
//! switch when the recommendation differs.
//!
//! Automatic invocation from background maintenance is deliberately absent:
//! every switch fences pre-switch WAL redo and arms the mandatory checkpoint,
//! fencing writes until the checkpoint lands. The caller runs the returned
//! switch and then checkpoints; silent background switches would stall the
//! write path.

use super::core::EdgeStore;
use super::record_form::MigrateStats;
use crate::edge::{is_bundled_eligible, RecordForm};
use graphdb_core::StorageResult;

impl EdgeStore {
    /// Recommend the record form for the current schema and strategies.
    ///
    /// Pure recommendation over schema shape only (no I/O, no state change):
    /// empty property sets on multi-edge legs stay pure, bundled-eligible
    /// single scalars resolve to bundled, everything else stays columnar.
    /// Single-edge directions always resolve to columnar because only that
    /// form provides fixed single slots.
    pub fn recommended_record_form(&self) -> RecordForm {
        if self.schema.properties.is_empty()
            && !matches!(self.schema.oe_strategy, crate::edge::EdgeStrategy::Single)
            && !matches!(self.schema.ie_strategy, crate::edge::EdgeStrategy::Single)
        {
            return RecordForm::Pure;
        }
        if is_bundled_eligible(
            &self.schema.properties,
            self.schema.oe_strategy,
            self.schema.ie_strategy,
        ) {
            return RecordForm::Bundled;
        }
        RecordForm::Columnar
    }

    /// Switch to the recommended form when it differs from the current one.
    ///
    /// Explicit opt-in entry: returns `None` when the table already holds the
    /// recommended form, otherwise runs the same online switch as the manual
    /// path (pure rebuild, WAL fence, mandatory checkpoint afterwards).
    /// Guard failures (pending schema change, pending checkpoint, illegal
    /// target for the live data) propagate as errors with the table
    /// untouched. The caller must checkpoint after a successful switch.
    pub fn auto_migrate_record_form_if_beneficial(
        &mut self,
    ) -> StorageResult<Option<MigrateStats>> {
        let target = self.recommended_record_form();
        if target == self.schema.record_form {
            return Ok(None);
        }
        // Quote cost first so illegal targets fail with the plan wording
        // before paying the rebuild.
        let _ = self.migration_plan(target)?;
        let stats = self.switch_record_form_online(target)?;
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
        assert_eq!(columnar.recommended_record_form(), RecordForm::Bundled);

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

    #[test]
    fn auto_migrate_switches_narrow_columnar_to_bundled() {
        let mut table = EdgeStore::with_config(weight_schema(), EdgeTableConfig::default())
            .expect("columnar table builds");
        table
            .insert_edge(0, 1, 0, &[("weight".to_string(), Value::Double(1.5))], 100)
            .expect("insert");
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
            .prepare_add_property("extra".to_string(), DataType::Double, true, None)
            .expect("prepare add");
        assert!(table.auto_migrate_record_form_if_beneficial().is_err());
        assert_eq!(table.schema.record_form, RecordForm::Columnar);
    }
}
