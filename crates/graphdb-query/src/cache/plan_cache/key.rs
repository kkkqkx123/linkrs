use crate::cache::config::{CachePriority, PlanCacheConfig};
use crate::cache::stats::PlanCacheStats;
use crate::planning::plan::execution_plan::PartitionSpec;
use std::hash::Hasher;
use std::time::{Duration, Instant};

/// Parameter location information
#[derive(Debug, Clone)]
pub struct ParamPosition {
    /// Parameter Index
    pub index: usize,
    /// Parameter name (named parameter)
    pub name: Option<String>,
    /// Position of the parameter in the query
    pub position: usize,
    /// Desired data types
    pub expected_type: Option<graphdb_core::types::DataType>,
}

/// Context stored with a cached plan.
#[derive(Debug, Clone, Default)]
pub struct PlanCachePutContext {
    pub dependent_tables: Vec<String>,
    pub space_name: Option<String>,
    pub schema_version: Option<u64>,
    pub index_version: Option<u64>,
    pub is_dml: bool,
    pub is_transaction: bool,
    pub optimizer_version: u64,
    pub planning_config_hash: u64,
}

/// Complete request/catalog/runtime context used for every plan-cache operation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlanCacheContext {
    pub space_name: Option<String>,
    pub schema_version: Option<u64>,
    pub index_version: Option<u64>,
    pub param_type_signature: Option<u64>,
    pub optimizer_version: u64,
    pub planning_config_hash: u64,
}

/// Query Plan Cache Key
///
/// Supports fast lookups using the hash of the query text as the key.
/// Also store query text for conflict detection.
///
/// When a plan carries a non-stale `PartitionSpec` the key includes a
/// partition fingerprint so that a change in the underlying data layout
/// (e.g. re-indexing) automatically yields a cache miss, forcing the
/// planner to produce a fresh physical plan.  Callers that only have
/// the query text (the common lookup path before planning) query with
/// `fingerprint == None`, which will *not* match entries that were stored
/// with a fingerprint — effectively isolating partitioned plans from the
/// cache until the caller can provide the current layout version.
///
/// **M0**: Cache key now includes optional `space_name` and `schema_version`
/// to prevent cross-space plan reuse and stale plans after schema changes.
///
/// **M1.6**: Cache key includes optional `param_type_signature` so that the
/// same query text with different parameter type signatures produces a
/// different key, but different parameter *values* do not (allowing cached
/// plan reuse across executions with different values of the same types).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PlanCacheKey {
    /// Query the hash value of the text
    pub hash: u64,
    /// Query text (for conflict detection, not just debugging)
    pub(crate) query_text: String,
    /// Partition layout fingerprint.  `Some` when the cached plan holds a
    /// `PartitionSpec`; absent for single-tree plans.
    pub(crate) partition_fingerprint: Option<u64>,
    /// Space/catalog identity — prevents cross-space plan reuse (M0).
    pub(crate) space_name: Option<String>,
    /// Schema version at planning time — forces replan after DDL (M0).
    pub(crate) schema_version: Option<u64>,
    /// Parameter type signature — prevents reuse when param types differ (M1.6).
    /// Does NOT include parameter values, only their declared types.
    pub(crate) param_type_signature: Option<u64>,
    /// Index version at planning time — forces replan after index DDL.
    pub(crate) index_version: Option<u64>,
    pub(crate) optimizer_version: u64,
    pub(crate) planning_config_hash: u64,
}

impl PlanCacheKey {
    /// Creating Cache Keys from Query Text
    pub fn from_query(query: &str) -> Self {
        let hash = Self::hash_query(query);
        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: None,
            space_name: None,
            schema_version: None,
            param_type_signature: None,
            index_version: None,
            optimizer_version: 0,
            planning_config_hash: 0,
        }
    }

    /// Create a key scoped to a specific space and schema version.
    ///
    /// M1.6: `param_type_signature` is a hash of the parameter types (not
    /// values) so the same query with different param values but same types
    /// reuses the cached plan.
    pub fn from_query_with_space(
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
    ) -> Self {
        Self::from_query_with_full_context(query, space_name, schema_version, None, None)
    }

    /// Create a key with all available context dimensions.
    pub fn from_query_with_full_context(
        query: &str,
        space_name: Option<String>,
        schema_version: Option<u64>,
        param_type_signature: Option<u64>,
        index_version: Option<u64>,
    ) -> Self {
        Self::from_query_with_context(
            query,
            PlanCacheContext {
                space_name,
                schema_version,
                index_version,
                param_type_signature,
                ..Default::default()
            },
        )
    }

    pub fn from_query_with_context(query: &str, context: PlanCacheContext) -> Self {
        use std::hash::Hash;

        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        if let Some(ref name) = context.space_name {
            name.hash(&mut hasher);
        }
        context.schema_version.hash(&mut hasher);
        context.param_type_signature.hash(&mut hasher);
        context.index_version.hash(&mut hasher);
        context.optimizer_version.hash(&mut hasher);
        context.planning_config_hash.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: None,
            space_name: context.space_name,
            schema_version: context.schema_version,
            param_type_signature: context.param_type_signature,
            index_version: context.index_version,
            optimizer_version: context.optimizer_version,
            planning_config_hash: context.planning_config_hash,
        }
    }

    /// Create a key that additionally captures a physical partition layout,
    /// preventing stale cached plans from being reused after layout changes.
    pub fn from_query_with_partition(query: &str, spec: &PartitionSpec) -> Self {
        Self::from_query_with_partition_and_context(query, spec, PlanCacheContext::default())
    }

    /// Create a partition-scoped key that also carries the full compatibility
    /// context (space / schema / index / param types / optimizer / planning
    /// config).  Used so partitioned plans are invalidated on DDL, cross-space
    /// reuse is blocked, and a changed layout fingerprint forces a replan.
    pub fn from_query_with_partition_and_context(
        query: &str,
        spec: &PartitionSpec,
        context: PlanCacheContext,
    ) -> Self {
        use std::hash::Hash;

        let fp = Self::compute_fingerprint(spec);

        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        fp.hash(&mut hasher);
        if let Some(ref name) = context.space_name {
            name.hash(&mut hasher);
        }
        context.schema_version.hash(&mut hasher);
        context.param_type_signature.hash(&mut hasher);
        context.index_version.hash(&mut hasher);
        context.optimizer_version.hash(&mut hasher);
        context.planning_config_hash.hash(&mut hasher);
        let hash = hasher.finish();

        Self {
            hash,
            query_text: query.to_string(),
            partition_fingerprint: Some(fp),
            space_name: context.space_name,
            schema_version: context.schema_version,
            param_type_signature: context.param_type_signature,
            index_version: context.index_version,
            optimizer_version: context.optimizer_version,
            planning_config_hash: context.planning_config_hash,
        }
    }

    fn hash_query(query: &str) -> u64 {
        use std::hash::Hash;
        let mut hasher = Self::hasher();
        Self::normalize_query(query).hash(&mut hasher);
        hasher.finish()
    }

    /// Canonicalize insignificant whitespace for cache identity while retaining
    /// the original query text for collision diagnostics.
    fn normalize_query(query: &str) -> String {
        query.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn hasher() -> std::collections::hash_map::DefaultHasher {
        std::collections::hash_map::DefaultHasher::new()
    }

    /// Produce a numeric fingerprint from the partition ranges, source, and
    /// layout version so that any change in the range layout, the data domain
    /// it maps over, or the storage layout version yields a different key.
    fn compute_fingerprint(spec: &PartitionSpec) -> u64 {
        use std::hash::Hash;
        let mut hasher = Self::hasher();
        for range in spec.ranges() {
            range.start.hash(&mut hasher);
            range.end.hash(&mut hasher);
        }
        spec.source().to_string().hash(&mut hasher);
        spec.layout_version().hash(&mut hasher);
        hasher.finish()
    }

    /// Verify that the query text matches (for conflict detection)
    pub fn verify_query(&self, query: &str) -> bool {
        self.query_text == query
    }

    /// Get query text (for debugging or logging)
    pub fn query_text(&self) -> &str {
        &self.query_text
    }

    /// Whether this key holds a partition fingerprint.
    pub fn has_partition_fingerprint(&self) -> bool {
        self.partition_fingerprint.is_some()
    }

    /// Return the space name, if any.
    pub fn space_name(&self) -> Option<&str> {
        self.space_name.as_deref()
    }

    /// Return the schema version, if any.
    pub fn schema_version(&self) -> Option<u64> {
        self.schema_version
    }
}
