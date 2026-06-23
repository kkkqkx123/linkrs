//! Value Module - Graph Database Value Type System
//!
//! This module provides the core value type system in the graph database.
//!
//! ## Module Structure
//!
//! - `null` - NullType definition
//! - `value` - Value enum definition and basic methods
//! - `value_compare` - Comparison logic (PartialEq, Eq, Ord, Hash)
//! - `value_arithmetic` - Arithmetic/logical/bitwise operations
//! - `value_convert` - Type conversion
//! - `list` - List type
//! - `date_time` - Date and time types
//! - `decimal128` - Decimal128 high-precision numeric
//! - `geography` - Geospatial types
//! - `json` - JSON/JSONB types
//! - `memory` - Memory estimation

#[allow(non_snake_case)]
pub mod date_time;
pub mod decimal128;
pub mod geography;
pub mod interval;
pub mod json;
pub mod list;
pub mod memory;
pub mod null;
pub mod uuid;
pub mod value_arithmetic;
pub mod value_compare;
pub mod value_convert;
pub mod value_def;
pub mod vector;

// Re-export all public types
pub use date_time::{DateTimeValue, DateValue, TimeValue};
pub use decimal128::Decimal128Value;
pub use geography::{
    GeoJsonFeature, GeoJsonFeatureCollection, GeoJsonGeometry, Geography, GeographyValue,
    LineStringValue, MultiLineStringValue, MultiPointValue, MultiPolygonValue, PolygonValue,
};
pub use interval::{IntervalError, IntervalValue};
pub use json::{Json, JsonB, JsonError};
pub use list::List;
pub use null::NullType;
pub use uuid::{UuidError, UuidValue};
pub use value_def::Value;
pub use vector::VectorValue;
