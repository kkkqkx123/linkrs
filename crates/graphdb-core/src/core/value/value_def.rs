//! Value Type Definition - Core Enum and Basic Methods

use crate::core::DataSet;
use crate::core::{
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
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    hash::Hash,
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
    String(String),
    /// Fixed-length strings for optimized storage of short strings
    FixedString {
        len: usize,
        data: String,
    },
    /// Binary data
    Blob(Vec<u8>),
    Date(DateValue),
    Time(TimeValue),
    DateTime(DateTimeValue),
    Vertex(Box<Vertex>),
    Edge(Box<Edge>),
    Path(Box<Path>),
    List(Box<List>),
    Map(Box<HashMap<String, Value>>),
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
}

impl Value {
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
            Value::FixedString { len, .. } => DataType::FixedString(*len),
            Value::Blob(_) => DataType::Blob,
            Value::Date(_) => DataType::Date,
            Value::Time(_) => DataType::Time,
            Value::DateTime(_) => DataType::DateTime,
            Value::Vertex(_) => DataType::Vertex,
            Value::Edge(_) => DataType::Edge,
            Value::Path(_) => DataType::Path,
            Value::List(_) => DataType::List,
            Value::Map(_) => DataType::Map,
            Value::Set(_) => DataType::Set,
            Value::Geography(_) => DataType::Geography,
            Value::Vector(v) => DataType::VectorDense(v.dimension()),
            Value::DataSet(_) => DataType::DataSet,
            Value::Json(_) => DataType::Json,
            Value::JsonB(_) => DataType::JsonB,
            Value::Uuid(_) => DataType::Uuid,
            Value::Interval(_) => DataType::Interval,
        }
    }

    /// Alias for get_type
    pub fn data_type(&self) -> DataType {
        self.get_type()
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
            Value::FixedString { data, .. } => Some(data),
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

    /// Create fixed-length string value
    pub fn fixed_string(len: usize, data: String) -> Self {
        let padded_data = if data.len() > len {
            data.chars().take(len).collect()
        } else {
            format!("{:<width$}", data, width = len)
        };
        Value::FixedString {
            len,
            data: padded_data,
        }
    }

    /// Get the length of a fixed-length string
    pub fn fixed_string_len(&self) -> Option<usize> {
        match self {
            Value::FixedString { len, .. } => Some(*len),
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
            Value::FixedString { data, .. } => std::mem::size_of::<Self>() + data.capacity(),
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
                size +=
                    m.capacity() * (std::mem::size_of::<String>() + std::mem::size_of::<Value>());
                for (k, v) in m.as_ref() {
                    size += k.capacity();
                    size += v.estimated_size();
                }
                size
            }
            Value::Set(s) => {
                let mut size = std::mem::size_of::<Self>();
                size += s.capacity() * std::mem::size_of::<Value>();
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

    /// Create a new Map value (wraps in Box)
    pub fn map(map: HashMap<String, Value>) -> Self {
        Value::Map(Box::new(map))
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
            Value::FixedString { len, data } => write!(f, "\"{}\"[fixed:{}]", data, len),
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
        }
    }
}

impl Value {
    pub fn to_bytes(&self) -> Vec<u8> {
        match self {
            Value::Empty => vec![0],
            Value::Null(_) => vec![1],
            Value::Bool(b) => {
                let mut buf = vec![2];
                buf.push(if *b { 1 } else { 0 });
                buf
            }
            Value::SmallInt(i) => {
                let mut buf = vec![3];
                buf.extend_from_slice(&i.to_le_bytes());
                buf
            }
            Value::Int(i) => {
                let mut buf = vec![4];
                buf.extend_from_slice(&i.to_le_bytes());
                buf
            }
            Value::BigInt(i) => {
                let mut buf = vec![5];
                buf.extend_from_slice(&i.to_le_bytes());
                buf
            }
            Value::Float(f) => {
                let mut buf = vec![6];
                buf.extend_from_slice(&f.to_le_bytes());
                buf
            }
            Value::Double(d) => {
                let mut buf = vec![7];
                buf.extend_from_slice(&d.to_le_bytes());
                buf
            }
            Value::String(s) => {
                let mut buf = vec![8];
                let bytes = s.as_bytes();
                buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                buf.extend_from_slice(bytes);
                buf
            }
            Value::Blob(b) => {
                let mut buf = vec![9];
                buf.extend_from_slice(&(b.len() as u32).to_le_bytes());
                buf.extend_from_slice(b);
                buf
            }
            Value::Date(d) => {
                let mut buf = vec![10];
                buf.extend_from_slice(&d.year.to_le_bytes());
                buf.extend_from_slice(&d.month.to_le_bytes());
                buf.extend_from_slice(&d.day.to_le_bytes());
                buf
            }
            Value::Time(t) => {
                let mut buf = vec![11];
                buf.extend_from_slice(&t.hour.to_le_bytes());
                buf.extend_from_slice(&t.minute.to_le_bytes());
                buf.extend_from_slice(&t.sec.to_le_bytes());
                buf.extend_from_slice(&t.microsec.to_le_bytes());
                buf
            }
            Value::DateTime(dt) => {
                let mut buf = vec![12];
                buf.extend_from_slice(&dt.year.to_le_bytes());
                buf.extend_from_slice(&dt.month.to_le_bytes());
                buf.extend_from_slice(&dt.day.to_le_bytes());
                buf.extend_from_slice(&dt.hour.to_le_bytes());
                buf.extend_from_slice(&dt.minute.to_le_bytes());
                buf.extend_from_slice(&dt.sec.to_le_bytes());
                buf.extend_from_slice(&dt.microsec.to_le_bytes());
                buf
            }
            _ => vec![0],
        }
    }

    pub fn from_bytes(data: &[u8]) -> Option<(Value, usize)> {
        if data.is_empty() {
            return None;
        }

        let type_byte = data[0];
        match type_byte {
            0 => Some((Value::Empty, 1)),
            1 => Some((Value::Null(NullType::Null), 1)),
            2 => {
                if data.len() < 2 {
                    return None;
                }
                Some((Value::Bool(data[1] == 1), 2))
            }
            3 => {
                if data.len() < 3 {
                    return None;
                }
                let bytes: [u8; 2] = data[1..3].try_into().ok()?;
                Some((Value::SmallInt(i16::from_le_bytes(bytes)), 3))
            }
            4 => {
                if data.len() < 5 {
                    return None;
                }
                let bytes: [u8; 4] = data[1..5].try_into().ok()?;
                Some((Value::Int(i32::from_le_bytes(bytes)), 5))
            }
            5 => {
                if data.len() < 9 {
                    return None;
                }
                let bytes: [u8; 8] = data[1..9].try_into().ok()?;
                Some((Value::BigInt(i64::from_le_bytes(bytes)), 9))
            }
            6 => {
                if data.len() < 5 {
                    return None;
                }
                let bytes: [u8; 4] = data[1..5].try_into().ok()?;
                Some((Value::Float(f32::from_le_bytes(bytes)), 5))
            }
            7 => {
                if data.len() < 9 {
                    return None;
                }
                let bytes: [u8; 8] = data[1..9].try_into().ok()?;
                Some((Value::Double(f64::from_le_bytes(bytes)), 9))
            }
            8 => {
                if data.len() < 5 {
                    return None;
                }
                let len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
                if data.len() < 5 + len {
                    return None;
                }
                let s = String::from_utf8(data[5..5 + len].to_vec()).ok()?;
                Some((Value::String(s), 5 + len))
            }
            9 => {
                if data.len() < 5 {
                    return None;
                }
                let len = u32::from_le_bytes(data[1..5].try_into().ok()?) as usize;
                if data.len() < 5 + len {
                    return None;
                }
                Some((Value::Blob(data[5..5 + len].to_vec()), 5 + len))
            }
            10 => {
                if data.len() < 16 {
                    return None;
                }
                let year = i32::from_le_bytes(data[1..5].try_into().ok()?);
                let month = u32::from_le_bytes(data[5..9].try_into().ok()?);
                let day = u32::from_le_bytes(data[9..13].try_into().ok()?);
                Some((Value::Date(DateValue { year, month, day }), 13))
            }
            11 => {
                if data.len() < 17 {
                    return None;
                }
                let hour = u32::from_le_bytes(data[1..5].try_into().ok()?);
                let minute = u32::from_le_bytes(data[5..9].try_into().ok()?);
                let sec = u32::from_le_bytes(data[9..13].try_into().ok()?);
                let microsec = u32::from_le_bytes(data[13..17].try_into().ok()?);
                Some((
                    Value::Time(TimeValue {
                        hour,
                        minute,
                        sec,
                        microsec,
                    }),
                    17,
                ))
            }
            12 => {
                if data.len() < 29 {
                    return None;
                }
                let year = i32::from_le_bytes(data[1..5].try_into().ok()?);
                let month = u32::from_le_bytes(data[5..9].try_into().ok()?);
                let day = u32::from_le_bytes(data[9..13].try_into().ok()?);
                let hour = u32::from_le_bytes(data[13..17].try_into().ok()?);
                let minute = u32::from_le_bytes(data[17..21].try_into().ok()?);
                let sec = u32::from_le_bytes(data[21..25].try_into().ok()?);
                let microsec = u32::from_le_bytes(data[25..29].try_into().ok()?);
                Some((
                    Value::DateTime(DateTimeValue {
                        year,
                        month,
                        day,
                        hour,
                        minute,
                        sec,
                        microsec,
                    }),
                    29,
                ))
            }
            _ => None,
        }
    }
}
