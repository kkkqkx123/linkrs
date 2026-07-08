use crate::core::value::{
    date_time::{DateTimeValue, DateValue, TimeValue},
    geography::Geography,
    interval::IntervalValue,
    list::List,
    null::NullType,
    Value,
};
use std::{cmp::Ordering as CmpOrdering, collections::HashMap, hash::Hash};

// Manual implementation of PartialEq to handle f64 comparisons correctly
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Empty, Value::Empty) => true,
            (Value::Null(a), Value::Null(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::SmallInt(a), Value::SmallInt(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::BigInt(a), Value::BigInt(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => (a == b) || (a.is_nan() && b.is_nan()),
            (Value::Double(a), Value::Double(b)) => (a == b) || (a.is_nan() && b.is_nan()),
            (Value::Decimal128(a), Value::Decimal128(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::FixedString { data: a, .. }, Value::FixedString { data: b, .. }) => a == b,
            (Value::Date(a), Value::Date(b)) => a == b,
            (Value::Time(a), Value::Time(b)) => a == b,
            (Value::DateTime(a), Value::DateTime(b)) => a == b,
            (Value::Vertex(a), Value::Vertex(b)) => a == b,
            (Value::Edge(a), Value::Edge(b)) => a == b,
            (Value::Path(a), Value::Path(b)) => a == b,
            (Value::List(a), Value::List(b)) => a == b,
            (Value::Map(a), Value::Map(b)) => a == b,
            (Value::Set(a), Value::Set(b)) => a == b,
            (Value::Geography(a), Value::Geography(b)) => a == b,
            (Value::Json(a), Value::Json(b)) => a == b,
            (Value::JsonB(a), Value::JsonB(b)) => a == b,
            // JSON and JSONB can be compared
            (Value::Json(a), Value::JsonB(b)) => a.to_value().ok() == Some(b.as_value().clone()),
            (Value::JsonB(a), Value::Json(b)) => Some(a.as_value().clone()) == b.to_value().ok(),
            (Value::Uuid(a), Value::Uuid(b)) => a == b,
            (Value::Interval(a), Value::Interval(b)) => a == b,

            // Cross-type integer comparisons: promote to i64
            (Value::SmallInt(a), Value::Int(b)) => *a as i64 == *b as i64,
            (Value::Int(a), Value::SmallInt(b)) => *a as i64 == *b as i64,
            (Value::SmallInt(a), Value::BigInt(b)) => *a as i64 == *b,
            (Value::BigInt(a), Value::SmallInt(b)) => *a == *b as i64,
            (Value::Int(a), Value::BigInt(b)) => *a as i64 == *b,
            (Value::BigInt(a), Value::Int(b)) => *a == *b as i64,

            // Integer to float comparisons
            (Value::SmallInt(a), Value::Float(b)) => *a as f32 == *b,
            (Value::Float(a), Value::SmallInt(b)) => *a == *b as f32,
            (Value::Int(a), Value::Float(b)) => *a as f32 == *b,
            (Value::Float(a), Value::Int(b)) => *a == *b as f32,
            (Value::BigInt(a), Value::Float(b)) => *a as f32 == *b,
            (Value::Float(a), Value::BigInt(b)) => *a == *b as f32,
            (Value::SmallInt(a), Value::Double(b)) => *a as f64 == *b,
            (Value::Double(a), Value::SmallInt(b)) => *a == *b as f64,
            (Value::Int(a), Value::Double(b)) => *a as f64 == *b,
            (Value::Double(a), Value::Int(b)) => *a == *b as f64,
            (Value::BigInt(a), Value::Double(b)) => *a as f64 == *b,
            (Value::Double(a), Value::BigInt(b)) => *a == *b as f64,
            (Value::Float(a), Value::Double(b)) => *a as f64 == *b,
            (Value::Double(a), Value::Float(b)) => *a == *b as f64,

            _ => false,
        }
    }
}

// Eq is implemented manually, since f64 does not implement Eq
impl Eq for Value {}

impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<CmpOrdering> {
        Some(self.cmp(other))
    }
}

impl Ord for Value {
    fn cmp(&self, other: &Self) -> CmpOrdering {
        match (self, other) {
            // Comparison of the same type
            (Value::Empty, Value::Empty) => CmpOrdering::Equal,
            (Value::Null(a), Value::Null(b)) => Self::cmp_null(a, b),
            (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
            (Value::SmallInt(a), Value::SmallInt(b)) => a.cmp(b),
            (Value::Int(a), Value::Int(b)) => a.cmp(b),
            (Value::BigInt(a), Value::BigInt(b)) => a.cmp(b),
            (Value::Float(a), Value::Float(b)) => Self::cmp_f32(*a, *b),
            (Value::Double(a), Value::Double(b)) => Self::cmp_f64(*a, *b),
            (Value::Decimal128(a), Value::Decimal128(b)) => a.cmp(b),
            (Value::String(a), Value::String(b)) => a.cmp(b),
            (Value::FixedString { data: a, .. }, Value::FixedString { data: b, .. }) => a.cmp(b),
            (Value::Date(a), Value::Date(b)) => Self::cmp_date(a, b),
            (Value::Time(a), Value::Time(b)) => Self::cmp_time(a, b),
            (Value::DateTime(a), Value::DateTime(b)) => Self::cmp_datetime(a, b),
            (Value::Vertex(a), Value::Vertex(b)) => a.cmp(b),
            (Value::Edge(a), Value::Edge(b)) => a.cmp(b),
            (Value::Path(a), Value::Path(b)) => a.cmp(b),
            (Value::List(a), Value::List(b)) => Self::cmp_list(a, b),
            (Value::Map(a), Value::Map(b)) => Self::cmp_map(a, b),
            (Value::Set(a), Value::Set(b)) => Self::cmp_set(a, b),
            (Value::Geography(a), Value::Geography(b)) => Self::cmp_geography(a, b),
            (Value::Json(a), Value::Json(b)) => match (a.to_value(), b.to_value()) {
                (Ok(a_val), Ok(b_val)) => Self::cmp_json_values(&a_val, &b_val),
                _ => CmpOrdering::Equal,
            },
            (Value::JsonB(a), Value::JsonB(b)) => a.cmp(b),
            // Cross-type comparison
            (Value::Json(a), Value::JsonB(b)) => match a.to_value() {
                Ok(a_val) => Self::cmp_json_values(&a_val, b.as_value()),
                _ => CmpOrdering::Equal,
            },
            (Value::JsonB(a), Value::Json(b)) => match b.to_value() {
                Ok(b_val) => Self::cmp_json_values(a.as_value(), &b_val),
                _ => CmpOrdering::Equal,
            },
            (Value::Uuid(a), Value::Uuid(b)) => a.cmp(b),
            (Value::Interval(a), Value::Interval(b)) => Self::cmp_interval(a, b),

            // Cross-type integer comparisons: promote to i64
            (Value::SmallInt(a), Value::Int(b)) => (*a as i64).cmp(&(*b as i64)),
            (Value::Int(a), Value::SmallInt(b)) => (*a as i64).cmp(&(*b as i64)),
            (Value::SmallInt(a), Value::BigInt(b)) => (*a as i64).cmp(b),
            (Value::BigInt(a), Value::SmallInt(b)) => a.cmp(&(*b as i64)),
            (Value::Int(a), Value::BigInt(b)) => (*a as i64).cmp(b),
            (Value::BigInt(a), Value::Int(b)) => a.cmp(&(*b as i64)),

            // Integer to float comparisons
            (Value::SmallInt(a), Value::Float(b)) => {
                (*a as f32).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Float(a), Value::SmallInt(b)) => {
                a.partial_cmp(&(*b as f32)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Int(a), Value::Float(b)) => {
                (*a as f32).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Float(a), Value::Int(b)) => {
                a.partial_cmp(&(*b as f32)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::BigInt(a), Value::Float(b)) => {
                (*a as f32).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Float(a), Value::BigInt(b)) => {
                a.partial_cmp(&(*b as f32)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::SmallInt(a), Value::Double(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Double(a), Value::SmallInt(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Int(a), Value::Double(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Double(a), Value::Int(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::BigInt(a), Value::Double(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Double(a), Value::BigInt(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Float(a), Value::Double(b)) => {
                (*a as f64).partial_cmp(b).unwrap_or(CmpOrdering::Equal)
            }
            (Value::Double(a), Value::Float(b)) => {
                a.partial_cmp(&(*b as f64)).unwrap_or(CmpOrdering::Equal)
            }

            // Comparison between different types: based on type prioritization
            (a, b) => Self::cmp_by_type_priority(a, b),
        }
    }
}

// Manually implementing Hash to handle f64 hashes
impl Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Value::Empty => 0u8.hash(state),
            Value::Null(n) => {
                1u8.hash(state);
                n.hash(state);
            }
            Value::Bool(b) => {
                2u8.hash(state);
                b.hash(state);
            }
            Value::SmallInt(i) => {
                3u8.hash(state);
                i.hash(state);
            }
            Value::Int(i) => {
                4u8.hash(state);
                i.hash(state);
            }
            Value::BigInt(i) => {
                5u8.hash(state);
                i.hash(state);
            }
            Value::Float(f) => {
                6u8.hash(state);
                // Creating a hash from a bitwise representation of a floating point number
                if f.is_nan() {
                    // All NaN values should hash to the same value
                    0x7fc00000_u32.hash(state);
                } else if *f == 0.0 {
                    // Ensure +0.0 and -0.0 hash to the same value
                    0.0_f32.to_bits().hash(state);
                } else {
                    f.to_bits().hash(state);
                }
            }
            Value::Double(f) => {
                7u8.hash(state);
                // Creating a hash from a bitwise representation of a floating point number
                if f.is_nan() {
                    // All NaN values should hash to the same value
                    0x7ff8000000000000_u64.hash(state);
                } else if *f == 0.0 {
                    // Ensure +0.0 and -0.0 hash to the same value
                    0.0_f64.to_bits().hash(state);
                } else {
                    f.to_bits().hash(state);
                }
            }
            Value::Decimal128(d) => {
                8u8.hash(state);
                d.hash(state);
            }
            Value::String(s) => {
                9u8.hash(state);
                s.hash(state);
            }
            Value::FixedString { data, .. } => {
                10u8.hash(state);
                data.hash(state);
            }
            Value::Blob(b) => {
                11u8.hash(state);
                b.hash(state);
            }
            Value::Date(d) => {
                12u8.hash(state);
                d.hash(state);
            }
            Value::Time(t) => {
                13u8.hash(state);
                t.hash(state);
            }
            Value::DateTime(dt) => {
                14u8.hash(state);
                dt.hash(state);
            }
            Value::Vertex(v) => {
                15u8.hash(state);
                v.hash(state);
            }
            Value::Edge(e) => {
                16u8.hash(state);
                e.hash(state);
            }
            Value::Path(p) => {
                17u8.hash(state);
                p.hash(state);
            }
            Value::List(l) => {
                18u8.hash(state);
                l.hash(state);
            }
            Value::Map(m) => {
                19u8.hash(state);
                // Hash mapping by sorted key-value pairs
                let mut pairs: Vec<_> = m.iter().collect();
                pairs.sort_by_key(|&(k, _)| k);
                pairs.hash(state);
            }
            Value::Set(s) => {
                20u8.hash(state);
                // For collections, we will hash all values in sorted order to ensure consistency
                let mut values: Vec<_> = s.iter().collect();
                values.sort();
                values.hash(state);
            }
            Value::Geography(g) => {
                21u8.hash(state);
                g.hash(state);
            }
            Value::Json(j) => {
                22u8.hash(state);
                j.hash(state);
            }
            Value::JsonB(j) => {
                23u8.hash(state);
                j.hash(state);
            }
            Value::DataSet(ds) => {
                24u8.hash(state);
                ds.hash(state);
            }
            Value::Vector(v) => {
                25u8.hash(state);
                v.hash(state);
            }
            Value::Uuid(u) => {
                26u8.hash(state);
                u.hash(state);
            }
            Value::Interval(i) => {
                27u8.hash(state);
                i.hash(state);
            }
        }
    }
}

impl Value {
    // Null type comparison helper function
    fn cmp_null(a: &NullType, b: &NullType) -> CmpOrdering {
        match (a, b) {
            (NullType::Null, NullType::Null) => CmpOrdering::Equal,
            (NullType::NaN, NullType::NaN) => CmpOrdering::Equal,
            (NullType::BadData, NullType::BadData) => CmpOrdering::Equal,
            (NullType::BadType, NullType::BadType) => CmpOrdering::Equal,
            _ => {
                let priority_a = Self::null_type_priority(a);
                let priority_b = Self::null_type_priority(b);
                priority_a.cmp(&priority_b)
            }
        }
    }

    // Null type priority mapping function
    fn null_type_priority(typ: &NullType) -> u8 {
        match typ {
            NullType::Null => 0,
            NullType::NaN => 1,
            NullType::BadData => 2,
            NullType::BadType => 2,
            NullType::ErrOverflow => 3,
            NullType::UnknownProp => 4,
            NullType::DivByZero => 5,
            NullType::OutOfRange => 6,
        }
    }

    // Floating Point Comparison Helper Functions (f32)
    fn cmp_f32(a: f32, b: f32) -> CmpOrdering {
        if a.is_nan() && b.is_nan() {
            CmpOrdering::Equal
        } else if a.is_nan() {
            CmpOrdering::Less
        } else if b.is_nan() {
            CmpOrdering::Greater
        } else {
            a.partial_cmp(&b).unwrap_or(CmpOrdering::Equal)
        }
    }

    // Floating Point Comparison Helper Functions (f64)
    fn cmp_f64(a: f64, b: f64) -> CmpOrdering {
        if a.is_nan() && b.is_nan() {
            CmpOrdering::Equal
        } else if a.is_nan() {
            CmpOrdering::Less
        } else if b.is_nan() {
            CmpOrdering::Greater
        } else {
            a.partial_cmp(&b).unwrap_or(CmpOrdering::Equal)
        }
    }

    // Date Comparison Helper Functions
    fn cmp_date(a: &DateValue, b: &DateValue) -> CmpOrdering {
        match a.year.cmp(&b.year) {
            CmpOrdering::Equal => match a.month.cmp(&b.month) {
                CmpOrdering::Equal => a.day.cmp(&b.day),
                ord => ord,
            },
            ord => ord,
        }
    }

    // Time Comparison Auxiliary Functions
    fn cmp_time(a: &TimeValue, b: &TimeValue) -> CmpOrdering {
        match a.hour.cmp(&b.hour) {
            CmpOrdering::Equal => match a.minute.cmp(&b.minute) {
                CmpOrdering::Equal => match a.sec.cmp(&b.sec) {
                    CmpOrdering::Equal => a.microsec.cmp(&b.microsec),
                    ord => ord,
                },
                ord => ord,
            },
            ord => ord,
        }
    }

    // Date-Time Comparison Helper Functions
    fn cmp_datetime(a: &DateTimeValue, b: &DateTimeValue) -> CmpOrdering {
        match a.year.cmp(&b.year) {
            CmpOrdering::Equal => match a.month.cmp(&b.month) {
                CmpOrdering::Equal => match a.day.cmp(&b.day) {
                    CmpOrdering::Equal => match a.hour.cmp(&b.hour) {
                        CmpOrdering::Equal => match a.minute.cmp(&b.minute) {
                            CmpOrdering::Equal => match a.sec.cmp(&b.sec) {
                                CmpOrdering::Equal => a.microsec.cmp(&b.microsec),
                                ord => ord,
                            },
                            ord => ord,
                        },
                        ord => ord,
                    },
                    ord => ord,
                },
                ord => ord,
            },
            ord => ord,
        }
    }

    // List Comparison Helper Functions
    fn cmp_list(a: &List, b: &List) -> CmpOrdering {
        a.values.len().cmp(&b.values.len()).then_with(|| {
            a.values
                .iter()
                .zip(b.values.iter())
                .fold(CmpOrdering::Equal, |acc, (a_val, b_val)| {
                    if acc == CmpOrdering::Equal {
                        a_val.cmp(b_val)
                    } else {
                        acc
                    }
                })
        })
    }

    // Mapping Comparison Helper Functions
    fn cmp_map(a: &HashMap<String, Value>, b: &HashMap<String, Value>) -> CmpOrdering {
        a.len().cmp(&b.len()).then_with(|| {
            let mut a_pairs: Vec<_> = a.iter().collect();
            let mut b_pairs: Vec<_> = b.iter().collect();
            a_pairs.sort_by_key(|&(k, _)| k);
            b_pairs.sort_by_key(|&(k, _)| k);
            a_pairs.iter().zip(b_pairs.iter()).fold(
                CmpOrdering::Equal,
                |acc, ((a_k, a_v), (b_k, b_v))| {
                    if acc == CmpOrdering::Equal {
                        a_k.cmp(b_k).then_with(|| a_v.cmp(b_v))
                    } else {
                        acc
                    }
                },
            )
        })
    }

    // Collection comparison helper functions
    fn cmp_set(
        a: &std::collections::HashSet<Value>,
        b: &std::collections::HashSet<Value>,
    ) -> CmpOrdering {
        a.len().cmp(&b.len()).then_with(|| {
            let mut a_values: Vec<_> = a.iter().collect();
            let mut b_values: Vec<_> = b.iter().collect();
            a_values.sort();
            b_values.sort();
            a_values
                .iter()
                .zip(b_values.iter())
                .fold(CmpOrdering::Equal, |acc, (a_val, b_val)| {
                    if acc == CmpOrdering::Equal {
                        a_val.cmp(b_val)
                    } else {
                        acc
                    }
                })
        })
    }

    // Geographic comparison helper functions
    fn cmp_geography(a: &Geography, b: &Geography) -> CmpOrdering {
        use crate::core::value::geography::Geography::*;

        fn type_priority(geo: &Geography) -> u8 {
            match geo {
                Point(_) => 0,
                LineString(_) => 1,
                Polygon(_) => 2,
                MultiPoint(_) => 3,
                MultiLineString(_) => 4,
                MultiPolygon(_) => 5,
            }
        }

        fn cmp_points(
            a: &[crate::core::value::geography::GeographyValue],
            b: &[crate::core::value::geography::GeographyValue],
        ) -> CmpOrdering {
            a.len().cmp(&b.len()).then_with(|| {
                for (pa, pb) in a.iter().zip(b.iter()) {
                    match pa.latitude.partial_cmp(&pb.latitude) {
                        Some(CmpOrdering::Equal) => match pa.longitude.partial_cmp(&pb.longitude) {
                            Some(CmpOrdering::Equal) => continue,
                            Some(ord) => return ord,
                            None => return CmpOrdering::Equal,
                        },
                        Some(ord) => return ord,
                        None => continue,
                    }
                }
                CmpOrdering::Equal
            })
        }

        match (a, b) {
            (Point(pa), Point(pb)) => match pa.latitude.partial_cmp(&pb.latitude) {
                Some(CmpOrdering::Equal) => pa
                    .longitude
                    .partial_cmp(&pb.longitude)
                    .unwrap_or(CmpOrdering::Equal),
                Some(ord) => ord,
                None => CmpOrdering::Equal,
            },
            (LineString(la), LineString(lb)) => cmp_points(&la.points, &lb.points),
            (Polygon(pa), Polygon(pb)) => cmp_points(&pa.exterior.points, &pb.exterior.points)
                .then_with(|| pa.holes.len().cmp(&pb.holes.len())),
            (MultiPoint(ma), MultiPoint(mb)) => cmp_points(&ma.points, &mb.points),
            (MultiLineString(ma), MultiLineString(mb)) => ma
                .linestrings
                .len()
                .cmp(&mb.linestrings.len())
                .then_with(|| {
                    for (la, lb) in ma.linestrings.iter().zip(mb.linestrings.iter()) {
                        let ord = cmp_points(&la.points, &lb.points);
                        if ord != CmpOrdering::Equal {
                            return ord;
                        }
                    }
                    CmpOrdering::Equal
                }),
            (MultiPolygon(ma), MultiPolygon(mb)) => {
                ma.polygons.len().cmp(&mb.polygons.len()).then_with(|| {
                    for (pa, pb) in ma.polygons.iter().zip(mb.polygons.iter()) {
                        let ord = cmp_points(&pa.exterior.points, &pb.exterior.points);
                        if ord != CmpOrdering::Equal {
                            return ord;
                        }
                    }
                    CmpOrdering::Equal
                })
            }
            _ => type_priority(a).cmp(&type_priority(b)),
        }
    }

    // Interval Comparison Helper Functions
    fn cmp_interval(a: &IntervalValue, b: &IntervalValue) -> CmpOrdering {
        match a.months.cmp(&b.months) {
            CmpOrdering::Equal => match a.days.cmp(&b.days) {
                CmpOrdering::Equal => a.microseconds.cmp(&b.microseconds),
                ord => ord,
            },
            ord => ord,
        }
    }

    // JSON value comparison helper functions
    fn cmp_json_values(a: &serde_json::Value, b: &serde_json::Value) -> CmpOrdering {
        use serde_json::Value as JsonValue;
        match (a, b) {
            (JsonValue::Null, JsonValue::Null) => CmpOrdering::Equal,
            (JsonValue::Bool(a), JsonValue::Bool(b)) => a.cmp(b),
            (JsonValue::Number(a), JsonValue::Number(b)) => {
                // Compare numbers
                if let (Some(a_i64), Some(b_i64)) = (a.as_i64(), b.as_i64()) {
                    a_i64.cmp(&b_i64)
                } else if let (Some(a_f64), Some(b_f64)) = (a.as_f64(), b.as_f64()) {
                    a_f64.partial_cmp(&b_f64).unwrap_or(CmpOrdering::Equal)
                } else {
                    CmpOrdering::Equal
                }
            }
            (JsonValue::String(a), JsonValue::String(b)) => a.cmp(b),
            (JsonValue::Array(a), JsonValue::Array(b)) => a.len().cmp(&b.len()).then_with(|| {
                a.iter()
                    .zip(b.iter())
                    .fold(CmpOrdering::Equal, |acc, (a_val, b_val)| {
                        if acc == CmpOrdering::Equal {
                            Self::cmp_json_values(a_val, b_val)
                        } else {
                            acc
                        }
                    })
            }),
            (JsonValue::Object(a), JsonValue::Object(b)) => a.len().cmp(&b.len()).then_with(|| {
                let mut a_pairs: Vec<_> = a.iter().collect();
                let mut b_pairs: Vec<_> = b.iter().collect();
                a_pairs.sort_by_key(|&(k, _)| k);
                b_pairs.sort_by_key(|&(k, _)| k);
                a_pairs.iter().zip(b_pairs.iter()).fold(
                    CmpOrdering::Equal,
                    |acc, ((a_k, a_v), (b_k, b_v))| {
                        if acc == CmpOrdering::Equal {
                            a_k.cmp(b_k).then_with(|| Self::cmp_json_values(a_v, b_v))
                        } else {
                            acc
                        }
                    },
                )
            }),
            // Cross-type comparison: Null < Bool < Number < String < Array < Object
            (JsonValue::Null, _) => CmpOrdering::Less,
            (_, JsonValue::Null) => CmpOrdering::Greater,
            (JsonValue::Bool(_), _) => CmpOrdering::Less,
            (_, JsonValue::Bool(_)) => CmpOrdering::Greater,
            (JsonValue::Number(_), _) => CmpOrdering::Less,
            (_, JsonValue::Number(_)) => CmpOrdering::Greater,
            (JsonValue::String(_), _) => CmpOrdering::Less,
            (_, JsonValue::String(_)) => CmpOrdering::Greater,
            (JsonValue::Array(_), _) => CmpOrdering::Less,
            (_, JsonValue::Array(_)) => CmpOrdering::Greater,
        }
    }

    // Type priority comparison helper functions
    fn cmp_by_type_priority(a: &Value, b: &Value) -> CmpOrdering {
        let priority_a = Self::type_priority(a);
        let priority_b = Self::type_priority(b);
        priority_a.cmp(&priority_b)
    }

    // Type priority mapping function
    fn type_priority(value: &Value) -> u8 {
        match value {
            Value::Empty => 0,
            Value::Null(_) => 1,
            Value::Bool(_) => 2,
            Value::SmallInt(_) => 3,
            Value::Int(_) => 4,
            Value::BigInt(_) => 5,
            Value::Float(_) => 6,
            Value::Double(_) => 7,
            Value::Decimal128(_) => 8,
            Value::Date(_) => 9,
            Value::Time(_) => 10,
            Value::DateTime(_) => 11,
            Value::String(_) => 12,
            Value::FixedString { .. } => 13,
            Value::Blob(_) => 14,
            Value::Vertex(_) => 15,
            Value::Edge(_) => 16,
            Value::Path(_) => 17,
            Value::List(_) => 18,
            Value::Map(_) => 19,
            Value::Set(_) => 20,
            Value::Geography(_) => 21,
            Value::Json(_) => 22,
            Value::JsonB(_) => 23,
            Value::DataSet(_) => 24,
            Value::Vector(_) => 25,
            Value::Uuid(_) => 26,
            Value::Interval(_) => 27,
        }
    }
}
