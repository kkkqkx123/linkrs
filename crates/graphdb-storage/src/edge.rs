//! Edge Storage Module
//!
//! Provides CSR (Compressed Sparse Row) based edge storage.
//!
//! ## Components
//!
//! - `MutableCsr`: Mutable CSR supporting dynamic edge operations
//! - `SingleMutableCsr`: Optimized mutable CSR for single-edge scenarios
//! - `ImmutableCsr`: Frozen packed CSR for read-mostly groups (explicit
//!   freeze/unfreeze only, writes rejected while frozen)
//! - `CsrVariant`: Enum wrapper for runtime CSR selection
//! - `CsrWithProperties`: Ladybug-style columnar property storage
//! - `CsrShardSet`: Node-group sharded topology container routing by endpoint interval
//! - `EdgeStore`: Node-group sharded edge table combining out/in shards and property storage
//!
//! ## CSR Type Selection (two-level)
//!
//! Selection is two-level: first the record form (`RecordForm`), then the
//! row strategy (`EdgeStrategy`) under the columnar form.
//!
//! Level 1 — `RecordForm` fixes bytes per edge and capability ceiling:
//!
//! | RecordForm | Bytes/edge | Use Case |
//! |------------|------------|----------|
//! | `Pure` | 12 | Pure topology: no rank, no timestamps, no properties |
//! | `Bundled` | 20 | One inline scalar property, read-mostly, stable schema |
//! | `Columnar` (default) | 32 + columnar properties | General multi-property edges |
//!
//! Level 2 — under `Columnar`, `EdgeStrategy` picks the CSR variant:
//!
//! | Strategy | CSR Type | Use Case | Time Complexity |
//! |----------|----------|----------|-----------------|
//! | `Multiple` | `MutableCsr` | General multi-edge relationships | O(degree) |
//! | `Single` | `SingleMutableCsr` | One-to-one relationships (spouse, current_employer) | O(1) |
//! | `None` | - | No edges stored | - |
//!
//! The form is locked at table creation and persisted; loading never
//! re-derives it, and precondition-breaking changes go through explicit
//! migration. Only `Pure` and `Columnar` are auto-derived; `Bundled` must
//! be selected explicitly because of its capability ceiling.

pub(crate) mod bundled_csr;
pub(crate) mod csr_shared;
pub mod csr_trait;
pub mod csr_variant;
pub mod csr_with_properties;
pub mod edge_table;
pub mod fragmentation_stats;
pub mod immutable_csr;
pub mod mutable_csr;
pub mod node_group;
pub mod property_schema;
pub(crate) mod pure_csr;
pub mod record_form_policy;
pub mod schema;
pub mod single_mutable_csr;
pub mod slot;

pub use csr_trait::{CsrBase, MutableCsrTrait};
pub use csr_variant::{CsrRowIter, CsrVariant};
pub use csr_with_properties::CsrWithProperties;
pub use edge_table::core::UpdateEdgePropertyByKeyParams;
pub use edge_table::{EdgeIndexStatus, EdgeStore, EdgeWalRecoveryMode, IncidentDeletedEdge};
pub use fragmentation_stats::{
    FragmentationStats, VertexFragmentation, GROUP_FRAGMENTATION_THRESHOLD,
};
pub use graphdb_core::types::EdgeStrategy;
use graphdb_core::types::{EdgeId, Timestamp, VertexId};
use graphdb_core::Value;
pub use mutable_csr::{EdgePosition, MutableCsr, MutableCsrIterator};
pub use node_group::{
    region_id_for_local, region_local_range, regions_per_group, CsrShardSet, EdgeCheckpointKind,
    FreezeBlockReason, FreezeFeasibility, GroupDirty, NodeGroupStats, RegionDirty,
    RegionMergeScope, ShardCsrIterator, TableShardManifest, DEFAULT_NODE_GROUP_BITS,
    GROUP_MERGE_MIN_DENSITY, LEAF_REGION_ROWS, REGION_MERGE_MIN_DENSITY,
};
pub use single_mutable_csr::{SingleMutableCsr, SingleMutableCsrIterator};

pub use bundled_csr::{decode_scalar, encode_scalar, BundledCsr};
pub use edge_table::checkpoint::snapshot::{
    MappedFrozen, MappedFrozenIterator, MappedFrozenRowIter,
};
pub use graphdb_core::types::INVALID_EDGE_ID;
pub use immutable_csr::{FrozenRowIter, ImmutableCsr, ImmutableCsrIterator};
pub use pure_csr::{PureAllIter, PureRowIter, PureTopologyCsr};

pub use record_form_policy::{
    bundled_ineligibility_reason, inline_form_accepts_rank, is_bundled_eligible,
    is_scalar_encodable, validate_record_form_target, validate_strategy_form, RecordForm,
    RecordFormPreference, BUNDLED_RANK_REQUIRES_COLUMNAR_MSG,
};
pub(crate) use record_form_policy::{
    INLINE_FORM_SCHEMA_CHANGE_MSG, NO_EDGES_STORED_MSG, ROW_POSITION_CROSS_VARIANT_MSG,
    SCHEMA_CHANGE_PENDING_MSG, SINGLE_REQUIRES_COLUMNAR_MSG,
};
pub use schema::{EdgeMultiplicity, EdgeRecord, EdgeSchema, IndexConsistency, StorageDirection};
pub use slot::{ColdStamps, HotNbr, Nbr};

/// One edge batch-insert entry: `(src, dst, rank, properties, ts)`.
pub type BatchInsertEntry<'a> = (u32, u32, i64, &'a [(String, Value)], Timestamp);

/// Decoded neighbor for bulk puts: `(endpoint, rank, edge_id, ts)`.
pub type EdgePut = (u32, i64, EdgeId, Timestamp);

/// One source row's bulk-put batch: `(local_src, entries)`.
pub type RowEdgeBatch = (u32, Vec<EdgePut>);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StoragePropertyDef;

    #[test]
    fn hot_half_stays_within_single_cache_line_budget() {
        // Per-edge memory budget, pinned exactly: 24-byte hot half plus
        // 8-byte cold half is 32 bytes per edge with no padding waste.
        // Rank stays 64-bit because it is a caller-controlled multigraph
        // key shared with the query layer, WAL redo records and endpoint
        // key packing; narrowing it would reject legal inputs instead of
        // storing them. create_ts lives in the EdgeTimestamps authority,
        // not inline; only delete_ts is kept for row-level reclaim.
        assert_eq!(std::mem::size_of::<HotNbr>(), 24);
        assert_eq!(std::mem::size_of::<ColdStamps>(), 8);
        assert_eq!(std::mem::size_of::<Nbr>(), 32);
    }

    #[test]
    fn slot_halves_roundtrip_through_nbr() {
        let nbr = Nbr::with_create_ts(7, 2, EdgeId(9), 100);
        let assembled = Nbr::from_parts(nbr.hot(), nbr.cold());
        assert_eq!(assembled, nbr);
        assert_eq!(
            Nbr::from_parts(HotNbr::dead_gap(), ColdStamps::dead_gap()),
            Nbr::dead_gap()
        );
    }

    #[test]
    fn test_edge_schema_validation_both_none() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "invalid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::None,
            ie_strategy: EdgeStrategy::None,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least one"));
    }

    #[test]
    fn test_edge_schema_validation_both_enabled() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "valid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::Multiple,
            ie_strategy: EdgeStrategy::Single,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_ok());
    }

    #[test]
    fn pure_target_rejection_comes_from_the_shared_constant() {
        use graphdb_core::DataType;
        let props = vec![StoragePropertyDef {
            name: "p".to_string(),
            data_type: DataType::Double,
            nullable: true,
            default_value: None,
        }];
        let err = validate_record_form_target(
            &props,
            EdgeStrategy::Multiple,
            EdgeStrategy::Multiple,
            RecordForm::Pure,
        )
        .expect_err("pure target with properties must be rejected");
        assert!(
            err.to_string()
                .contains(record_form_policy::PURE_REQUIRES_ZERO_PROPERTIES_MSG),
            "unexpected wording: {}",
            err
        );
    }

    #[test]
    fn test_edge_schema_validation_ie_only() {
        let schema = EdgeSchema {
            label_id: 0,
            label_name: "valid_edge".to_string(),
            src_label: 0,
            dst_label: 0,
            properties: vec![],
            oe_strategy: EdgeStrategy::None,
            ie_strategy: EdgeStrategy::Multiple,
            schema_version: 1,
            record_form: RecordForm::default(),
        };

        let result = schema.validate();
        assert!(result.is_ok());
        assert_eq!(schema.storage_direction(), StorageDirection::InOnly);
        assert!(!schema.has_out());
        assert!(schema.has_in());
    }
}
