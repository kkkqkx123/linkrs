//! Table construction and identity accessors.

use super::super::super::{CsrShardSet, EdgeSchema, RecordForm, RecordFormPreference};
use super::super::config::EdgeTableConfig;
use super::super::mvcc::MVCCManager;
use super::EdgeStore;
use crate::edge::property_schema::PropertySchema;
use crate::edge::CsrWithProperties;
use crate::schema::{LabelVersionHistory, SchemaObjectType};
use graphdb_core::types::{EdgeId, EdgeStrategy, LabelId, Timestamp};
use graphdb_core::{StorageError, StorageResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

impl EdgeStore {
    pub fn new(schema: EdgeSchema) -> StorageResult<Self> {
        Self::with_config(schema, EdgeTableConfig::default())
    }

    pub fn with_config(schema: EdgeSchema, config: EdgeTableConfig) -> StorageResult<Self> {
        schema.validate()?;

        if config.overflow_chunk_edges == 0 {
            return Err(StorageError::invalid_operation(
                "overflow_chunk_edges must be greater than zero",
            ));
        }

        // Auto derivation locks at creation: no properties selects pure,
        // anything else selects columnar. The bundled inline form is an
        // explicit opt-in only (`RecordFormPreference::Bundled`): `Auto`
        // never selects it, so a table only takes the inline limits (no
        // rank, no MVCC version chain, no online schema change) when the
        // operator asks for them by name. The bundled admission rules live
        // in `is_bundled_eligible`, shared with the creation and migration
        // prechecks so the entries cannot drift apart. The resolved form
        // persists and never re-derives on load; later precondition breaks
        // must migrate explicitly.
        let record_form = match config.record_form {
            RecordFormPreference::Columnar => RecordForm::Columnar,
            RecordFormPreference::Bundled => {
                if let Some(reason) = crate::edge::bundled_ineligibility_reason(
                    &schema.properties,
                    schema.oe_strategy,
                    schema.ie_strategy,
                ) {
                    return Err(StorageError::invalid_operation(reason));
                }
                RecordForm::Bundled
            }
            RecordFormPreference::Auto => {
                if schema.properties.is_empty()
                    && schema.oe_strategy != EdgeStrategy::Single
                    && schema.ie_strategy != EdgeStrategy::Single
                {
                    RecordForm::Pure
                } else {
                    RecordForm::Columnar
                }
            }
        };
        // The resolved form is authoritative in memory and on disk: load
        // paths never re-infer it. Report the derivation so the locked
        // choice and its later evolution cost stay visible to operators.
        // Bundled is a fast path with hard limits (no rank, no MVCC version
        // chain, no online schema change), reachable only through the
        // explicit preference; evolving schemas stay on the columnar form
        // without asking.
        if record_form == RecordForm::Bundled {
            log::info!(
                "edge table '{}' uses the explicitly requested Bundled inline form for '{}': \
                no rank, no MVCC version chain and no online schema change; \
                migrate to the columnar form when those are needed",
                schema.label_name,
                schema
                    .properties
                    .first()
                    .map(|p| p.name.as_str())
                    .unwrap_or(""),
            );
        } else {
            log::info!(
                "edge table '{}' uses record form {:?} (preference {:?})",
                schema.label_name,
                record_form,
                config.record_form,
            );
        }
        let mut schema = schema;
        schema.record_form = record_form;
        schema.validate_resolved()?;
        let mut out_csr = CsrShardSet::new(
            schema.oe_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
            record_form,
        )?;
        let mut in_csr = CsrShardSet::new(
            schema.ie_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
            record_form,
        )?;
        // Pre-create groups covering the configured initial row space so
        // small tables start with their full address range addressable.
        // Single-direction tables materialize only the stored leg.
        let initial_groups = config
            .initial_vertex_capacity
            .div_ceil(out_csr.group_size())
            .max(1);
        if schema.has_out() {
            out_csr.resize_groups(initial_groups)?;
        }
        if schema.has_in() {
            in_csr.resize_groups(initial_groups)?;
        }

        let prop_schemas: Vec<PropertySchema> = schema
            .properties
            .iter()
            .enumerate()
            .map(|(i, p)| {
                PropertySchema::new(p.name.clone(), i as i32, p.data_type.clone())
                    .nullable(p.nullable)
                    .with_default_value(p.default_value.clone())
            })
            .collect();
        // Inline forms keep a schema-only stub: the single scalar (or no
        // property at all) lives in the CSR value column, never in columns.
        let properties = match record_form {
            RecordForm::Pure | RecordForm::Bundled => CsrWithProperties::inline_stub(prop_schemas),
            RecordForm::Columnar => CsrWithProperties::new(prop_schemas),
        };

        let label_id = schema.label_id;
        let label_name = schema.label_name.clone();

        let version_history = Arc::new(Mutex::new(LabelVersionHistory::new(
            label_id,
            label_name.clone(),
            SchemaObjectType::Edge,
        )));

        let mut property_index_cache = HashMap::new();
        for (idx, prop) in schema.properties.iter().enumerate() {
            property_index_cache.insert(prop.name.clone(), idx);
        }

        Ok(Self {
            label: label_id,
            label_name,
            src_label: schema.src_label,
            dst_label: schema.dst_label,
            schema,
            out_csr,
            in_csr,
            mvcc: MVCCManager::new(),
            properties,
            properties_dirty: false,
            property_column_dirt: HashMap::new(),
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            property_index: None,
            index_write_failures: 0,
            index_consistency: crate::edge::IndexConsistency::BestEffort,
            index_lag_baseline: 0,
            index_pool_capacity: 0,
            index_stale_since: None,
            group_write_counts: HashMap::new(),
            form_width_bytes: std::sync::atomic::AtomicU64::new(0),
            form_width_samples: std::sync::atomic::AtomicU64::new(0),
            form_reads: std::sync::atomic::AtomicU64::new(0),
            form_writes: std::sync::atomic::AtomicU64::new(0),
            last_auto_migrated_to_bundled: false,
            pending_add_column: None,
            pending_drop_column: None,
            pending_rename_column: None,
            edge_owner: super::owner::EdgeOwnerMap::new(),
            segment_stats: HashMap::new(),
            commit_scratch: super::super::staging::CommitScratch::default(),
            last_reclaim_bound: Timestamp::MAX,
            last_reclaim_tombstones: 0,
            wal_dir: None,
            property_fallback_rewrites: 0,
            migration_pending_checkpoint: false,
        })
    }

    pub fn set_stats_manager(&mut self, stats: std::sync::Arc<graphdb_metrics::StatsManager>) {
        self.stats_manager = Some(stats);
    }

    pub fn label(&self) -> LabelId {
        self.label
    }

    pub fn src_label(&self) -> LabelId {
        self.src_label
    }

    pub fn dst_label(&self) -> LabelId {
        self.dst_label
    }

    pub fn schema(&self) -> &EdgeSchema {
        &self.schema
    }

    /// Storage direction derived from the enabled CSR strategies.
    pub fn storage_direction(&self) -> super::super::super::StorageDirection {
        self.schema.storage_direction()
    }

    /// Whether the outgoing leg is stored and served.
    pub fn has_out_edges(&self) -> bool {
        self.schema.has_out()
    }

    /// Cardinality contract of the outgoing direction.
    pub fn out_multiplicity(&self) -> super::super::super::EdgeMultiplicity {
        self.schema.out_multiplicity()
    }

    /// Cardinality contract of the incoming direction.
    pub fn in_multiplicity(&self) -> super::super::super::EdgeMultiplicity {
        self.schema.in_multiplicity()
    }

    /// Whether a direction is available for reads. Missing legs read as
    /// empty adjacency instead of failing.
    pub fn is_direction_available(&self, outgoing: bool) -> bool {
        if outgoing {
            self.schema.has_out()
        } else {
            self.schema.has_in()
        }
    }

    /// Human-readable reason when a direction is not stored. Returns `None`
    /// while the direction is available. Query planning uses this to tell an
    /// empty missing leg apart from a genuinely empty stored leg.
    pub fn direction_note(&self, outgoing: bool) -> Option<String> {
        if self.is_direction_available(outgoing) {
            return None;
        }
        let want = if outgoing { "outgoing" } else { "incoming" };
        Some(format!(
            "table '{}' stores no {} leg (direction {:?}); reads on this leg are empty",
            self.label_name,
            want,
            self.schema.storage_direction()
        ))
    }

    pub(crate) fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

    /// Replace the table schema and rebuild the property index cache.
    ///
    /// Load-only helper: the caller must validate the resolved
    /// strategy-plus-form combination afterwards, since this setter alone
    /// performs no cardinality check.
    pub fn set_schema(&mut self, schema: EdgeSchema) {
        // Rebuild property index cache
        self.property_index_cache.clear();
        for (idx, prop) in schema.properties.iter().enumerate() {
            self.property_index_cache.insert(prop.name.clone(), idx);
        }
        self.schema = schema;
    }

    /// Get reference to version history Arc for shared access
    pub fn version_history_ref(&self) -> Arc<Mutex<LabelVersionHistory>> {
        Arc::clone(&self.version_history)
    }
}
