//! Record-form selection policy: enums, shared failure reasons and
//! the admission/validation gates every write and migration path uses.

use crate::types::StoragePropertyDef;
use graphdb_core::types::EdgeStrategy;

/// Resolved record form for an edge table, persisted in `meta.bin`.
///
/// Determined once at table creation by the selector; never re-inferred on
/// load. The choice locks the physical layout: later property additions,
/// type changes or rank usage breaking the preconditions need an explicit
/// migration (`EdgeStore::migration_plan`, `migrate_record_form` or
/// `switch_record_form_online`) followed by a checkpoint, never an
/// in-place reinterpretation.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum RecordForm {
    /// Pure topology: 12 bytes/edge, no rank, no timestamps.
    Pure,
    /// Bundled: 20 bytes/edge, inline single scalar value column.
    Bundled,
    /// Standard columnar property storage (default / fallback).
    #[default]
    Columnar,
}

/// User-facing preference for record form selection at table creation time.
///
/// `Auto` derives a safe default from the schema (no properties to pure,
/// anything else to columnar) and reports the result through the table
/// construction log; the resolved form then locks and persists. Later
/// evolution breaking the preconditions must migrate explicitly.
/// `Columnar` forces the standard multi/single/none strategy path.
/// `Bundled` is an explicit opt-in to the single-scalar inline form: `Auto`
/// never selects it, so a table only takes the inline limits (no rank, no
/// MVCC version chain, no online schema change) when the operator asks for
/// them by name.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum RecordFormPreference {
    /// Auto-select Pure/Columnar based on schema properties.
    Auto,
    /// Force columnar storage regardless of schema.
    #[default]
    Columnar,
    /// Explicit opt-in to the bundled inline form for one encodable scalar.
    ///
    /// Creation fails when the schema is not bundled-eligible (arity, type
    /// or single-edge direction), reporting the same reason the migration
    /// precheck reports.
    Bundled,
}

/// Shared rejection for a single-edge strategy paired with an inline form.
///
/// Single directions need fixed single slots, which only the columnar form
/// provides. Every construction, load and migration gate reports this exact
/// wording so operators see one conflict and one way out.
pub(crate) const SINGLE_REQUIRES_COLUMNAR_MSG: &str =
    "single edge strategy requires the columnar record form; adjust the strategy or keep the columnar form, see migration_plan/migrate_record_form";

/// Shared rejection for schema changes on an inline-form table.
///
/// Pure and bundled tables carry no independent property columns, so
/// add/drop-column must rebuild the record form first. Every staged
/// schema gate reports this exact wording so operators see one conflict
/// and one way out.
pub(crate) const INLINE_FORM_SCHEMA_CHANGE_MSG: &str =
    "schema change on an inline-form table requires a record-form rebuild (migrate_record_form or switch_record_form_online)";

/// Shared rejection for a topology-only table carrying properties.
///
/// Pure tables hold no property columns, so any property count above zero
/// must stay on another form. The central target check and every plan entry
/// report this exact wording so operators see one conflict and one way out.
pub(crate) const PURE_REQUIRES_ZERO_PROPERTIES_MSG: &str =
    "pure record form requires zero properties; drop properties or keep the columnar form, see migration_plan/migrate_record_form";

/// Shared rejection for a second staged schema change.
///
/// Add, drop and rename share one pending slot, so a second prepare while
/// one change is staged reports this exact wording from every entry.
pub(crate) const SCHEMA_CHANGE_PENDING_MSG: &str = "another schema change is already pending";

/// Shared rejection for writes against a direction storing no edges.
///
/// Placeholder groups hold vertex capacity only. Every insert and
/// result-returning delete reports this exact wording so callers see one
/// conflict and one way out instead of per-entry phrasing.
pub(crate) const NO_EDGES_STORED_MSG: &str = "no edges stored for this edge type";

/// Shared rejection for positional writes crossing variants.
///
/// Row positions are variant-local. Every positional entry validates the
/// edge id and refuses stale or foreign positions with this exact wording
/// instead of falling back to an id scan.
pub(crate) const ROW_POSITION_CROSS_VARIANT_MSG: &str =
    "row position must not cross variants; re-resolve by edge id";

/// Shared rejection for nonzero ranks on an inline-form table.
///
/// Pure and bundled layouts carry no rank column, so any nonzero rank must
/// stay on the columnar form. Every write, bulk and migration gate reports
/// this exact wording so operators see one conflict and one way out.
pub const BUNDLED_RANK_REQUIRES_COLUMNAR_MSG: &str =
    "nonzero rank requires the columnar record form; keep the columnar form, see migration_plan/migrate_record_form/switch_record_form_online";

/// Whether an inline-form table may accept one write with the given rank.
///
/// Single source of truth for the rank gate shared by prevalidation and the
/// caller-side guard: inline forms (`Pure`/`Bundled`) pin rank to zero, so a
/// nonzero rank must stay columnar. Callers planning rank writes on a
/// bundled candidate probe this before staging the batch instead of paying
/// a whole-batch rollback after the commit rejects it.
pub fn inline_form_accepts_rank(record_form: RecordForm, rank: i64) -> bool {
    match record_form {
        RecordForm::Pure | RecordForm::Bundled => rank == 0,
        RecordForm::Columnar => true,
    }
}

/// Whether one direction may pair a strategy with a record form.
///
/// Single source of truth for the single-plus-inline rule, shared by shard
/// construction, fresh-variant creation and migration prechecks.
pub fn validate_strategy_form(
    strategy: EdgeStrategy,
    record_form: RecordForm,
) -> graphdb_core::StorageResult<()> {
    if strategy == EdgeStrategy::Single && record_form != RecordForm::Columnar {
        return Err(graphdb_core::StorageError::invalid_operation(
            SINGLE_REQUIRES_COLUMNAR_MSG.to_string(),
        ));
    }
    Ok(())
}

/// Check whether a `DataType` can be encoded as a 64-bit scalar for the
/// `Bundled` record form.
pub fn is_scalar_encodable(dt: &graphdb_core::DataType) -> bool {
    use graphdb_core::DataType;
    matches!(
        dt,
        DataType::Bool
            | DataType::SmallInt
            | DataType::Int
            | DataType::BigInt
            | DataType::Float
            | DataType::Double
            | DataType::Date
            | DataType::Time
            | DataType::DateTime
    )
}

/// Whether a schema may use the `Bundled` record form.
///
/// Single source of truth for the bundled admission rules, shared by the
/// explicit `Bundled` creation preference and the migration precheck:
/// exactly one property, an encodable scalar type, and no single-edge
/// direction. Bundled additionally carries no rank, no MVCC version chain
/// and no online schema change, so the `Auto` selector never picks it even
/// when a schema is eligible here; schemas expecting those capabilities
/// stay columnar without asking.
pub fn is_bundled_eligible(
    properties: &[StoragePropertyDef],
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
) -> bool {
    bundled_ineligibility_reason(properties, oe_strategy, ie_strategy).is_none()
}

/// Why a schema cannot use the `Bundled` record form, if it cannot.
///
/// Returns the same wording the migration precheck reports, so creation and
/// migration refuse a bundled target for the same stated reason.
pub fn bundled_ineligibility_reason(
    properties: &[StoragePropertyDef],
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
) -> Option<String> {
    if oe_strategy == EdgeStrategy::Single || ie_strategy == EdgeStrategy::Single {
        return Some(SINGLE_REQUIRES_COLUMNAR_MSG.to_string());
    }
    if properties.len() != 1 {
        return Some(
            "bundled record form requires exactly one property; adjust the schema or keep the columnar form, see migration_plan/migrate_record_form"
                .to_string(),
        );
    }
    if !is_scalar_encodable(&properties[0].data_type) {
        return Some(format!(
            "property type {:?} cannot inline into the bundled form; keep the columnar form, see migration_plan/migrate_record_form",
            properties[0].data_type
        ));
    }
    None
}

/// Validate strategy-plus-form rules for a target record form.
///
/// Single source of truth for migration target prechecks and resolved-schema
/// validation: direction presence is checked by the caller through
/// `EdgeSchema::validate`, while pairing, pure arity and bundled admission
/// are checked here against the given target.
pub fn validate_record_form_target(
    properties: &[StoragePropertyDef],
    oe_strategy: EdgeStrategy,
    ie_strategy: EdgeStrategy,
    target: RecordForm,
) -> graphdb_core::StorageResult<()> {
    validate_strategy_form(oe_strategy, target)?;
    validate_strategy_form(ie_strategy, target)?;
    match target {
        RecordForm::Pure if !properties.is_empty() => {
            return Err(graphdb_core::StorageError::invalid_operation(
                PURE_REQUIRES_ZERO_PROPERTIES_MSG.to_string(),
            ));
        }
        RecordForm::Bundled => {
            if let Some(reason) = bundled_ineligibility_reason(properties, oe_strategy, ie_strategy)
            {
                return Err(graphdb_core::StorageError::invalid_operation(reason));
            }
        }
        RecordForm::Pure | RecordForm::Columnar => {}
    }
    Ok(())
}
