//! Table construction and identity accessors.

use super::super::super::{CsrShardSet, EdgeSchema};
use super::super::config::EdgeTableConfig;
use super::super::mvcc::MVCCManager;
use super::EdgeStore;
use crate::edge::property_schema::PropertySchema;
use crate::edge::CsrWithProperties;
use crate::schema::{LabelVersionHistory, SchemaObjectType};
use graphdb_core::types::{EdgeId, LabelId, Timestamp};
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

        let mut out_csr = CsrShardSet::new(
            schema.oe_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        let mut in_csr = CsrShardSet::new(
            schema.ie_strategy,
            config.node_group_bits,
            config.overflow_chunk_edges,
        )?;
        // Pre-create groups covering the configured initial row space so
        // small tables start with their full address range addressable.
        let initial_groups = config
            .initial_vertex_capacity
            .div_ceil(out_csr.group_size())
            .max(1);
        if schema.oe_strategy != super::super::super::EdgeStrategy::None {
            out_csr.resize_groups(initial_groups)?;
        }
        if schema.ie_strategy != super::super::super::EdgeStrategy::None {
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
        let properties = CsrWithProperties::new(prop_schemas);

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
            is_open: true,
            next_edge_id: EdgeId(0),
            config,
            stats_manager: None,
            version_history,
            property_index_cache,
            property_index: None,
            index_write_failures: 0,
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

    pub(crate) fn schema_mut(&mut self) -> &mut EdgeSchema {
        &mut self.schema
    }

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
