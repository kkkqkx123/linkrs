//! Value Type Definition - Core Enum and Basic Methods

use crate::DataSet;
use crate::{
    types::storage_ids::{EdgeId, VertexId},
    types::DataType,
    value::{
        date_time::{DateTimeValue, DateValue, TimeValue},
        decimal128::Decimal128Value,
        geography::Geography,
        interval::IntervalValue,
        json::{Json, JsonB, JsonError},
        list::List,
        null::NullType,
        uuid::UuidValue,
        vector::VectorValue,
    },
    vertex_edge_path::{Edge, Path, Vertex},
    ArrayTypeInfo, StructTypeInfo,
};
use compact_str::CompactString;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
    sync::Arc,
};

/// Indicates values that can be stored in node/edge attributes
/// Simplified design following PostgreSQL type system
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Value {
    Empty,
    Null(NullType),
    Bool(bool),
    // Integer types: simplified to 3 types (aligned with PostgreSQL)
    SmallInt(i16), // 2 bytes, corresponds to PostgreSQL smallint
    Int(i32),      // 4 bytes, corresponds to PostgreSQL integer
    BigInt(i64),   // 8 bytes, corresponds to PostgreSQL bigint
    // Floating point types: 2 types (standard practice)
    Float(f32),  // 4 bytes, single precision
    Double(f64), // 8 bytes, double precision
    Decimal128(Decimal128Value),
    String(CompactString),
    /// Fixed-length strings for optimized storage of short strings.
    /// Content is always padded/truncated to the declared length, so the
    /// length is derivable via `chars().count()`.
    FixedString(String),
    /// Binary data
    Blob(Vec<u8>),
    Date(DateValue),
    Time(TimeValue),
    DateTime(DateTimeValue),
    Vertex(Box<Vertex>),
    Edge(Box<Edge>),
    Path(Box<Path>),
    List(Box<List>),
    /// Map with generalized keys: any hashable `Value` (string keys remain
    /// the common case; float keys use the normalized NaN/±0 hashing).
    Map(Box<HashMap<Value, Value>>),
    Set(Box<HashSet<Value>>),
    Geography(Geography),
    Vector(VectorValue),
    DataSet(Box<DataSet>),

    /// JSON type (text format)
    Json(Box<Json>),
    /// JSONB type (binary format)
    JsonB(Box<JsonB>),
    /// UUID type (16 bytes binary)
    Uuid(UuidValue),
    /// Interval type (PostgreSQL compatible)
    Interval(IntervalValue),

    /// Lightweight vertex ID reference (no heap allocation).
    /// Used by expand fast path when only the vertex ID is needed
    /// (e.g. count-only aggregation downstream).
    VertexId(VertexId),
    /// Lightweight edge ID reference (no heap allocation).
    /// Used by expand fast path when only the edge ID is needed.
    EdgeId(EdgeId),

    /// STRUCT value: ordered named fields.
    Struct(Box<StructValue>),
    /// ARRAY value: element-homogeneous array.
    Array(Box<ArrayValue>),
}

/// STRUCT value: ordered field table.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructValue {
    pub fields: Vec<(String, Value)>,
}

/// ARRAY value: fixed-size array.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArrayValue {
    pub values: Vec<Value>,
}

impl StructValue {
    pub fn new(fields: Vec<(String, Value)>) -> Self {
        Self { fields }
    }
}

impl ArrayValue {
    pub fn new(values: Vec<Value>) -> Self {
        Self { values }
    }
}

impl Value {
    /// Create a string value from a string-like type.
    ///
    /// `CompactString` stores up to 22 bytes inline without heap allocation,
    /// falling back to a heap-allocated `String` for longer content.
    pub fn string(s: impl AsRef<str>) -> Self {
        Value::String(CompactString::new(s.as_ref()))
    }

    /// Create a string value from an owned `String`.
    pub fn string_from_owned(s: String) -> Self {
        Value::String(CompactString::from(s))
    }

    /// Getting the type of value
    pub fn get_type(&self) -> DataType {
        match self {
            Value::Empty => DataType::Empty,
            Value::Null(_) => DataType::Null,
            Value::Bool(_) => DataType::Bool,
            Value::SmallInt(_) => DataType::SmallInt,
            Value::Int(_) => DataType::Int,
            Value::BigInt(_) => DataType::BigInt,
            Value::Float(_) => DataType::Float,
            Value::Double(_) => DataType::Double,
            Value::Decimal128(_) => DataType::Decimal128,
            Value::String(_) => DataType::String,
            Value::FixedString(data) => DataType::FixedString(data.chars().count()),
            Value::Blob(_) => DataType::Blob,
            Value::Date(_) => DataType::Date,
            Value::Time(_) => DataType::Time,
            Value::DateTime(_) => DataType::DateTime,
            Value::Vertex(_) => DataType::Vertex,
            Value::Edge(_) => DataType::Edge,
            Value::Path(_) => DataType::Path,
            Value::List(l) => DataType::List(Box::new(Self::container_element_type(l.iter()))),
            Value::Map(m) => DataType::Map(Box::new(Self::container_element_type(m.values()))),
            Value::Set(s) => DataType::Set(Box::new(Self::container_element_type(s.iter()))),
            Value::Geography(_) => DataType::Geography,
            Value::Vector(v) => DataType::VectorDense(v.dimension()),
            Value::DataSet(_) => DataType::DataSet,
            Value::Json(_) => DataType::Json,
            Value::JsonB(_) => DataType::JsonB,
            Value::Uuid(_) => DataType::Uuid,
            Value::Interval(_) => DataType::Interval,
            Value::VertexId(_) => DataType::Vertex,
            Value::EdgeId(_) => DataType::Edge,
            Value::Struct(s) => DataType::Struct(Arc::new(StructTypeInfo::new(
                s.fields
                    .iter()
                    .map(|(name, value)| (name.clone(), value.get_type()))
                    .collect(),
            ))),
            Value::Array(a) => DataType::Array(Arc::new(ArrayTypeInfo::new(
                a.values
                    .first()
                    .map(|v| v.get_type())
                    .unwrap_or(DataType::Empty),
                None,
            ))),
        }
    }

    /// Alias for get_type
    pub fn data_type(&self) -> DataType {
        self.get_type()
    }

    /// Common type of a container's elements.
    ///
    /// Folds `Value::get_type` over the element types with the numeric/temporal
    /// promotion hierarchy. Returns `DataType::Empty` for an empty container
    /// (the "untyped container" marker on the parameterized `List`/`Map`/`Set`
    /// variants).
    pub fn container_element_type<'a, I>(elements: I) -> DataType
    where
        I: IntoIterator<Item = &'a Value>,
    {
        let mut common = DataType::Empty;
        for element in elements {
            common = crate::type_system::TypeUtils::get_common_type(&common, &element.get_type());
            if common == DataType::Empty {
                break;
            }
        }
        common
    }

    /// Check if the value is null
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null(_))
    }

    /// Check if the value is a numeric type
    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Value::SmallInt(_)
                | Value::Int(_)
                | Value::BigInt(_)
                | Value::Float(_)
                | Value::Double(_)
                | Value::Decimal128(_)
        )
    }

    /// Check if the value is BadNull
    pub fn is_bad_null(&self) -> bool {
        use super::null::NullType;
        matches!(
            self,
            Value::Null(NullType::BadData) | Value::Null(NullType::BadType)
        )
    }

    /// Check if the value is empty
    pub fn is_empty(&self) -> bool {
        matches!(self, Value::Empty)
    }

    /// Get Boolean value
    pub fn bool_value(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Get String value
    pub fn string_value(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            Value::FixedString(data) => Some(data),
            _ => None,
        }
    }

    /// Get vector value as Vec<f32> from List of Float values or Vector type
    pub fn as_vector(&self) -> Option<Vec<f32>> {
        match self {
            Value::Vector(vec) => Some(vec.to_dense()),
            Value::List(list) => {
                let vector: Option<Vec<f32>> = list
                    .iter()
                    .map(|v| match v {
                        Value::Float(f) => Some(*f),
                        Value::Double(f) => Some(*f as f32),
                        Value::Int(i) => Some(*i as f32),
                        Value::SmallInt(i) => Some(*i as f32),
                        Value::BigInt(i) => Some(*i as f32),
                        _ => None,
                    })
                    .collect();
                vector
            }
            Value::Blob(blob) => {
                if blob.len() % std::mem::size_of::<f32>() == 0 {
                    let len = blob.len() / std::mem::size_of::<f32>();
                    let mut vector = Vec::with_capacity(len);
                    let ptr = blob.as_ptr() as *const f32;
                    for i in 0..len {
                        unsafe {
                            vector.push(*ptr.add(i));
                        }
                    }
                    Some(vector)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get reference to vector data (more efficient than as_vector)
    pub fn as_vector_ref(&self) -> Option<&[f32]> {
        match self {
            Value::Vector(vec) => vec.as_dense(),
            _ => None,
        }
    }

    /// Create a new vector value
    pub fn vector(data: Vec<f32>) -> Self {
        Value::Vector(super::vector::VectorValue::dense(data))
    }

    /// Create a new sparse vector value
    pub fn sparse_vector(indices: Vec<u32>, values: Vec<f32>) -> Self {
        Value::Vector(super::vector::VectorValue::sparse(indices, values))
    }

    /// Create fixed-length string value (truncates or space-pads to `len`)
    pub fn fixed_string(len: usize, data: String) -> Self {
        let padded_data = if data.chars().count() > len {
            data.chars().take(len).collect()
        } else {
            format!("{:<width$}", data, width = len)
        };
        Value::FixedString(padded_data)
    }

    /// Get the length of a fixed-length string
    pub fn fixed_string_len(&self) -> Option<usize> {
        match self {
            Value::FixedString(data) => Some(data.chars().count()),
            _ => None,
        }
    }

    /// Compute the hash of the value
    pub fn hash_value(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut hasher = DefaultHasher::new();
        self.hash(&mut hasher);
        hasher.finish()
    }

    /// Estimate the memory usage of the value
    pub fn estimated_size(&self) -> usize {
        match self {
            Value::Empty => std::mem::size_of::<Self>(),
            Value::Null(_) => std::mem::size_of::<Self>(),
            Value::Bool(_) => std::mem::size_of::<Self>(),
            Value::SmallInt(_) => std::mem::size_of::<Self>(),
            Value::Int(_) => std::mem::size_of::<Self>(),
            Value::BigInt(_) => std::mem::size_of::<Self>(),
            Value::Float(_) => std::mem::size_of::<Self>(),
            Value::Double(_) => std::mem::size_of::<Self>(),
            Value::Decimal128(_) => std::mem::size_of::<Self>(),
            Value::String(s) => std::mem::size_of::<Self>() + s.capacity(),
            Value::FixedString(data) => std::mem::size_of::<Self>() + data.capacity(),
            Value::Blob(b) => std::mem::size_of::<Self>() + b.capacity(),
            Value::Date(_) => std::mem::size_of::<Self>(),
            Value::Time(_) => std::mem::size_of::<Self>(),
            Value::DateTime(_) => std::mem::size_of::<Self>(),
            Value::Vertex(v) => std::mem::size_of::<Self>() + v.estimated_size(),
            Value::Edge(e) => std::mem::size_of::<Self>() + e.estimated_size(),
            Value::Path(p) => std::mem::size_of::<Self>() + p.estimated_size(),
            Value::List(l) => std::mem::size_of::<Self>() + l.estimated_size(),
            Value::Map(m) => {
                let mut size = std::mem::size_of::<Self>();
                // Hash table bucket array overhead: u64 hash per entry
                size += m.capacity()
                    * (8 + std::mem::size_of::<Value>() + std::mem::size_of::<Value>());
                for (k, v) in m.as_ref() {
                    size += k.estimated_size();
                    size += v.estimated_size();
                }
                size
            }
            Value::Set(s) => {
                let mut size = std::mem::size_of::<Self>();
                // Hash table bucket array overhead: u64 hash per entry
                size += s.capacity() * (8 + std::mem::size_of::<Value>());
                for v in s.as_ref() {
                    size += v.estimated_size();
                }
                size
            }
            Value::Geography(g) => std::mem::size_of::<Self>() + g.estimated_size(),
            Value::Vector(v) => std::mem::size_of::<Self>() + v.estimated_size(),
            Value::DataSet(ds) => std::mem::size_of::<Self>() + ds.estimated_size(),
            Value::Json(j) => std::mem::size_of::<Self>() + j.estimated_size(),
            Value::JsonB(j) => std::mem::size_of::<Self>() + j.estimated_size(),
            Value::Uuid(_) => std::mem::size_of::<Self>(),
            Value::Interval(_) => std::mem::size_of::<Self>(),
            Value::VertexId(_) => std::mem::size_of::<Self>(),
            Value::EdgeId(_) => std::mem::size_of::<Self>(),
            Value::Struct(s) => {
                let mut size = std::mem::size_of::<Self>();
                for (name, value) in &s.fields {
                    size += name.capacity();
                    size += value.estimated_size();
                }
                size
            }
            Value::Array(a) => {
                let mut size = std::mem::size_of::<Self>();
                for value in &a.values {
                    size += value.estimated_size();
                }
                size
            }
        }
    }

    /// Create JSON value
    pub fn json(text: &str) -> Result<Self, JsonError> {
        Ok(Value::Json(Box::new(Json::parse(text)?)))
    }

    /// Create JSONB value
    pub fn jsonb(text: &str) -> Result<Self, JsonError> {
        Ok(Value::JsonB(Box::new(JsonB::parse(text)?)))
    }

    /// Create JSON value from serde_json::Value
    pub fn from_json_value(value: serde_json::Value) -> Self {
        Value::JsonB(Box::new(JsonB::from_value(value)))
    }
}

impl Value {
    /// Create a new List value (wraps in Box)
    pub fn list(list: List) -> Self {
        Value::List(Box::new(list))
    }

    /// Create a new Map value (wraps in Box).
    pub fn map(map: HashMap<Value, Value>) -> Self {
        Value::Map(Box::new(map))
    }

    /// Create a new Map value from string-keyed entries (the common case).
    pub fn string_map(map: HashMap<String, Value>) -> Self {
        Value::Map(Box::new(
            map.into_iter()
                .map(|(k, v)| (Value::string(k), v))
                .collect(),
        ))
    }

    /// Create a new Set value (wraps in Box)
    pub fn set(set: HashSet<Value>) -> Self {
        Value::Set(Box::new(set))
    }

    /// Create a new Edge value (wraps in Box)
    pub fn edge(edge: Edge) -> Self {
        Value::Edge(Box::new(edge))
    }

    /// Create a new Path value (wraps in Box)
    pub fn path(path: Path) -> Self {
        Value::Path(Box::new(path))
    }

    /// Create a new DataSet value (wraps in Box)
    pub fn dataset(dataset: DataSet) -> Self {
        Value::DataSet(Box::new(dataset))
    }

    /// Create a new Struct value (wraps in Box).
    pub fn struct_(fields: Vec<(String, Value)>) -> Self {
        Value::Struct(Box::new(StructValue::new(fields)))
    }

    /// Create a new Array value (wraps in Box).
    pub fn array(values: Vec<Value>) -> Self {
        Value::Array(Box::new(ArrayValue::new(values)))
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Empty => write!(f, "EMPTY"),
            Value::Null(n) => write!(f, "NULL({:?})", n),
            Value::Bool(b) => write!(f, "{}", b),
            Value::SmallInt(i) => write!(f, "{}", i),
            Value::Int(i) => write!(f, "{}", i),
            Value::BigInt(i) => write!(f, "{}", i),
            Value::Float(fl) => write!(f, "{}", fl),
            Value::Double(fl) => write!(f, "{}", fl),
            Value::Decimal128(d) => write!(f, "{}", d),
            Value::String(s) => write!(f, "{}", s),
            Value::FixedString(data) => {
                write!(f, "\"{}\"[fixed:{}]", data, data.chars().count())
            }
            Value::Blob(b) => write!(f, "Blob({} bytes)", b.len()),
            Value::Date(d) => write!(f, "{:04}-{:02}-{:02}", d.year, d.month, d.day),
            Value::Time(t) => write!(
                f,
                "{:02}:{:02}:{:02}.{:06}",
                t.hour, t.minute, t.sec, t.microsec
            ),
            Value::DateTime(dt) => write!(
                f,
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
                dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.sec, dt.microsec
            ),
            Value::Vertex(v) => write!(f, "Vertex({:?})", v.id()),
            Value::Edge(e) => write!(f, "Edge({:?} -> {:?})", e.src(), e.dst()),
            Value::Path(p) => write!(f, "Path({:?})", p),
            Value::List(list) => {
                write!(f, "[")?;
                for (i, item) in list.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            Value::Map(map) => {
                write!(f, "{{")?;
                for (i, (k, v)) in map.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, "}}")
            }
            Value::Set(set) => {
                write!(f, "{{")?;
                for (i, item) in set.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "}}")
            }
            Value::Geography(g) => write!(f, "{}", g),
            Value::Vector(v) => write!(f, "{}", v),
            Value::DataSet(ds) => write!(f, "DataSet({} rows)", ds.row_count()),
            Value::Json(j) => write!(f, "Json({})", j.as_str()),
            Value::JsonB(j) => write!(f, "JsonB({})", j.to_json_string()),
            Value::Uuid(u) => write!(f, "Uuid({})", u),
            Value::Interval(i) => write!(f, "Interval({})", i),
            Value::VertexId(vid) => write!(f, "VertexId({:?})", vid),
            Value::EdgeId(eid) => write!(f, "EdgeId({:?})", eid),
            Value::Struct(s) => {
                write!(f, "{{")?;
                for (i, (name, value)) in s.fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", name, value)?;
                }
                write!(f, "}}")
            }
            Value::Array(a) => {
                write!(f, "[")?;
                for (i, value) in a.values.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", value)?;
                }
                write!(f, "]")
            }
        }
    }
}
