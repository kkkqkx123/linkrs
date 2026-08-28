//! Transport-independent vector payload types shared across crates.
//!
//! # Type categories
//!
//! This module contains two logically distinct groups of types:
//!
//! ## Core shared types (`PointId`, `Payload`, `PayloadValue`, `PayloadSchemaType`)
//! True foundational types used by the wire layer (`graphdb-wire`), the storage
//! engine, and the query layer. These must remain in `graphdb-core` because
//! `graphdb-wire` depends on `graphdb-core` but cannot depend on `vector-search`
//! (it would drag in heavy transitive deps like rayon/memmap2 into a lightweight
//! wire crate).
//!
//! ## Vector filter DSL types (`VectorFilter`, `FilterCondition`, `ConditionType`, etc.)
//! Query-filter types that conceptually belong to `vector-search`. They remain
//! in core due to the dependency DAG (`graphdb-core` → `vector-search` is not
//! allowed; `graphdb-wire` needs `VectorFilter`/`PayloadSelector` for wire DTOs).
//! The canonical implementations and evaluation logic live in
//! `vector-search::filter`. These types are re-exported by `vector-search::types`
//! so downstream crates can import them from either path.
//!
//! # Future improvement
//! A dedicated `graphdb-vector-types` crate could break this coupling. The new
//! crate would sit at the same level as `graphdb-core` in the DAG, and both
//! `graphdb-core` and `vector-search` would depend on it. See
//! `docs/plan/fulltext_vector_architecture_refactor.md` for details.

use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Core shared types
// ---------------------------------------------------------------------------

/// Payload attached to a vector point: a JSON object keyed by field name.
pub type Payload = HashMap<String, serde_json::Value>;

/// A scalar payload value (alias of `serde_json::Value`).
pub type PayloadValue = serde_json::Value;

/// Point identifier: numeric or UUID-shaped string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum PointId {
    Num(u64),
    Uuid(String),
}

impl From<u64> for PointId {
    fn from(v: u64) -> Self {
        Self::Num(v)
    }
}

impl From<String> for PointId {
    fn from(v: String) -> Self {
        // Numeric strings normalize to `Num` so an id round-trips through
        // its string form regardless of the wire encoding.
        if let Ok(n) = v.parse::<u64>() {
            Self::Num(n)
        } else {
            Self::Uuid(v)
        }
    }
}

impl From<&str> for PointId {
    fn from(v: &str) -> Self {
        Self::from(v.to_string())
    }
}

impl std::fmt::Display for PointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Num(n) => write!(f, "{n}"),
            Self::Uuid(s) => write!(f, "{s}"),
        }
    }
}

/// Declared schema of an indexed payload field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PayloadSchemaType {
    Keyword,
    Integer,
    Float,
    Text,
    Bool,
    Geo,
    Datetime,
}

impl PayloadSchemaType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Integer => "integer",
            Self::Float => "float",
            Self::Text => "text",
            Self::Bool => "bool",
            Self::Geo => "geo",
            Self::Datetime => "datetime",
        }
    }
}

// ---------------------------------------------------------------------------
// Vector filter DSL types
//
// These are logically part of `vector-search` but live in core due to the
// dependency DAG constraint. The filter evaluation logic is in
// `vector-search::filter`. Downstream crates may import these from either
// `graphdb_core::vector` or `vector_search::types`.
// ---------------------------------------------------------------------------

/// Geographic point (`lat`, `lon` in degrees).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

/// Radius filter around a geographic center.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeoRadius {
    pub center: GeoPoint,
    /// Radius in meters.
    pub radius: f64,
}

impl GeoRadius {
    pub fn new(center: GeoPoint, radius: f64) -> Self {
        Self { center, radius }
    }
}

/// Lat/lon bounding-box filter.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GeoBoundingBox {
    pub top_left: GeoPoint,
    pub bottom_right: GeoPoint,
}

impl GeoBoundingBox {
    pub fn new(top_left: GeoPoint, bottom_right: GeoPoint) -> Self {
        Self {
            top_left,
            bottom_right,
        }
    }
}

/// Range bound on the number of values an array field holds.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ValuesCountCondition {
    pub gt: Option<u64>,
    pub gte: Option<u64>,
    pub lt: Option<u64>,
    pub lte: Option<u64>,
}

impl ValuesCountCondition {
    pub fn new() -> Self {
        Self {
            gt: None,
            gte: None,
            lt: None,
            lte: None,
        }
    }

    pub fn gt(mut self, value: u64) -> Self {
        self.gt = Some(value);
        self
    }

    pub fn gte(mut self, value: u64) -> Self {
        self.gte = Some(value);
        self
    }

    pub fn lt(mut self, value: u64) -> Self {
        self.lt = Some(value);
        self
    }

    pub fn lte(mut self, value: u64) -> Self {
        self.lte = Some(value);
        self
    }
}

impl Default for ValuesCountCondition {
    fn default() -> Self {
        Self::new()
    }
}

/// Top-level payload filter with Qdrant-style clause groups.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct VectorFilter {
    pub must: Option<Vec<FilterCondition>>,
    pub must_not: Option<Vec<FilterCondition>>,
    pub should: Option<Vec<FilterCondition>>,
    pub min_should: Option<MinShouldCondition>,
}

impl VectorFilter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn must(mut self, condition: FilterCondition) -> Self {
        self.must.get_or_insert_with(Vec::new).push(condition);
        self
    }

    pub fn must_not(mut self, condition: FilterCondition) -> Self {
        self.must_not.get_or_insert_with(Vec::new).push(condition);
        self
    }

    pub fn should(mut self, condition: FilterCondition) -> Self {
        self.should.get_or_insert_with(Vec::new).push(condition);
        self
    }
}

/// `should` clause with an explicit minimum match count.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MinShouldCondition {
    pub conditions: Vec<FilterCondition>,
    pub min_count: usize,
}

/// A single filter condition: field plus its predicate.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FilterCondition {
    pub field: String,
    pub condition: ConditionType,
}

impl FilterCondition {
    pub fn new(field: impl Into<String>, condition: ConditionType) -> Self {
        Self {
            field: field.into(),
            condition,
        }
    }

    pub fn match_value(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            field,
            ConditionType::Match {
                value: value.into(),
            },
        )
    }

    pub fn match_any(field: impl Into<String>, values: Vec<PayloadValue>) -> Self {
        Self::new(field, ConditionType::MatchAny { values })
    }

    pub fn range(field: impl Into<String>, range: RangeCondition) -> Self {
        Self::new(field, ConditionType::Range(range))
    }

    pub fn is_empty(field: impl Into<String>) -> Self {
        Self::new(field, ConditionType::IsEmpty)
    }

    pub fn is_null(field: impl Into<String>) -> Self {
        Self::new(field, ConditionType::IsNull)
    }

    pub fn has_id(ids: Vec<String>) -> Self {
        Self::new("_id", ConditionType::HasId { ids })
    }

    pub fn geo_radius(field: impl Into<String>, radius: GeoRadius) -> Self {
        Self::new(field, ConditionType::GeoRadius(radius))
    }

    pub fn geo_bounding_box(field: impl Into<String>, bbox: GeoBoundingBox) -> Self {
        Self::new(field, ConditionType::GeoBoundingBox(bbox))
    }

    pub fn values_count(field: impl Into<String>, count: ValuesCountCondition) -> Self {
        Self::new(field, ConditionType::ValuesCount(count))
    }

    pub fn contains(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            field,
            ConditionType::Contains {
                value: value.into(),
            },
        )
    }
}

/// Predicate of a [`FilterCondition`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ConditionType {
    /// Match a scalar field value.
    Match {
        value: String,
    },
    /// Match any of the given values (OR semantics).
    MatchAny {
        values: Vec<PayloadValue>,
    },
    /// Numeric range over the field value.
    Range(RangeCondition),
    IsEmpty,
    IsNull,
    HasId {
        ids: Vec<String>,
    },
    Nested {
        filter: Box<VectorFilter>,
    },
    GeoRadius(GeoRadius),
    GeoBoundingBox(GeoBoundingBox),
    ValuesCount(ValuesCountCondition),
    Contains {
        value: String,
    },
}

/// Numeric range bounds; absent bounds are open.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RangeCondition {
    pub gt: Option<f64>,
    pub gte: Option<f64>,
    pub lt: Option<f64>,
    pub lte: Option<f64>,
}

impl RangeCondition {
    pub fn new() -> Self {
        Self {
            gt: None,
            gte: None,
            lt: None,
            lte: None,
        }
    }

    pub fn gt(mut self, value: f64) -> Self {
        self.gt = Some(value);
        self
    }

    pub fn gte(mut self, value: f64) -> Self {
        self.gte = Some(value);
        self
    }

    pub fn lt(mut self, value: f64) -> Self {
        self.lt = Some(value);
        self
    }

    pub fn lte(mut self, value: f64) -> Self {
        self.lte = Some(value);
        self
    }
}

impl Default for RangeCondition {
    fn default() -> Self {
        Self::new()
    }
}

/// Returned-payload projection for search results.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PayloadSelector {
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

impl PayloadSelector {
    /// Apply this selector to a payload map: include-list keeps only the
    /// listed fields, exclude-list removes them.
    pub fn apply(&self, payload: &Payload) -> Payload {
        let mut out = if let Some(include) = &self.include {
            include
                .iter()
                .filter_map(|k| payload.get(k).map(|v| (k.clone(), v.clone())))
                .collect::<Payload>()
        } else {
            payload.clone()
        };
        if let Some(exclude) = &self.exclude {
            for k in exclude {
                out.remove(k);
            }
        }
        out
    }

    pub fn include(fields: Vec<String>) -> Self {
        Self {
            include: Some(fields),
            exclude: None,
        }
    }

    pub fn exclude(fields: Vec<String>) -> Self {
        Self {
            include: None,
            exclude: Some(fields),
        }
    }

    pub fn all() -> Self {
        Self {
            include: None,
            exclude: None,
        }
    }
}
