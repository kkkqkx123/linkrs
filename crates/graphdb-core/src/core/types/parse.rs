//! String parsing for `DataType` — the single source of truth.
//!
//! Every "string -> DataType" parser in the codebase (HTTP schema DDL, SQL
//! DDL, CAST targets) delegates to `DataType::from_str`. The parser is the
//! exact mirror of the `Display` impl: scalars roundtrip through their
//! canonical upper-case names, aliases follow PostgreSQL conventions, and
//! parameterized types (`FIXEDSTRING(n)`, `VECTOR(n)`, `STRUCT<...>`,
//! `ARRAY<...>(len)`) are parsed recursively.

use super::type_info::{ArrayTypeInfo, StructTypeInfo};
use super::DataType;
use std::str::FromStr;
use std::sync::Arc;

/// Error parsing a `DataType` from its string representation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown data type '{name}': {reason}")]
pub struct ParseDataTypeError {
    /// The original input that failed to parse.
    pub name: String,
    /// Human-readable explanation of why parsing failed.
    pub reason: String,
}

impl ParseDataTypeError {
    fn new(name: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            reason: reason.into(),
        }
    }
}

/// Maximum STRUCT/ARRAY nesting depth (aligned with the DDL parser's
/// `MAX_COMPOSITE_TYPE_DEPTH`).
const MAX_COMPOSITE_TYPE_DEPTH: usize = 16;

impl FromStr for DataType {
    type Err = ParseDataTypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_type(s, 0, s)
    }
}

/// Parse a type name. `depth` is the STRUCT/ARRAY nesting depth of the
/// current call (0 for the top level); `full` is the original top-level
/// input, used verbatim in error messages.
fn parse_type(input: &str, depth: usize, full: &str) -> Result<DataType, ParseDataTypeError> {
    if depth >= MAX_COMPOSITE_TYPE_DEPTH {
        return Err(ParseDataTypeError::new(
            full,
            format!(
                "type nesting exceeds the maximum depth of {}",
                MAX_COMPOSITE_TYPE_DEPTH
            ),
        ));
    }
    // Normalize whitespace (handles "DOUBLE PRECISION" with any spacing) and
    // case before matching. The original input is kept for STRUCT/ARRAY field
    // names, which are preserved as-is.
    let upper = input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();

    match upper.as_str() {
        "EMPTY" => Ok(DataType::Empty),
        // UNKNOWN is a binding-time sentinel; parsing it is accepted so the
        // Display output roundtrips (it never appears in user schema DDL).
        "UNKNOWN" => Ok(DataType::Unknown),
        "NULL" => Ok(DataType::Null),
        "BOOL" | "BOOLEAN" => Ok(DataType::Bool),
        // Integer aliases follow PostgreSQL conventions by bit width:
        // INT2 -> SmallInt, INT4/INT16/INT32 -> Int, INT8/INT64 -> BigInt.
        "SMALLINT" | "INT2" => Ok(DataType::SmallInt),
        "INT" | "INTEGER" | "INT4" | "INT16" | "INT32" => Ok(DataType::Int),
        "BIGINT" | "INT8" | "INT64" => Ok(DataType::BigInt),
        "FLOAT" | "FLOAT4" | "REAL" => Ok(DataType::Float),
        "DOUBLE" | "FLOAT8" | "DOUBLE PRECISION" => Ok(DataType::Double),
        "DECIMAL128" => Ok(DataType::Decimal128),
        "STRING" | "VARCHAR" | "TEXT" | "STR" => Ok(DataType::String),
        "DATE" => Ok(DataType::Date),
        "TIME" => Ok(DataType::Time),
        // TIMESTAMP is not a distinct type; it normalizes to DATETIME (kept
        // uniform across DDL/HTTP/CAST after the alias ruling).
        "DATETIME" | "TIMESTAMP" => Ok(DataType::DateTime),
        "VERTEX" => Ok(DataType::Vertex),
        "EDGE" => Ok(DataType::Edge),
        "PATH" => Ok(DataType::Path),
        "LIST" => Ok(DataType::List),
        "MAP" => Ok(DataType::Map),
        "SET" => Ok(DataType::Set),
        "GEOGRAPHY" => Ok(DataType::Geography),
        "DATASET" => Ok(DataType::DataSet),
        "BLOB" => Ok(DataType::Blob),
        "VECTOR" => Ok(DataType::Vector),
        "JSON" => Ok(DataType::Json),
        "JSONB" => Ok(DataType::JsonB),
        "UUID" => Ok(DataType::Uuid),
        "INTERVAL" => Ok(DataType::Interval),
        // The removed `VID` type is explicitly rejected with a hint.
        "VID" => Err(ParseDataTypeError::new(
            full,
            "VID is not a property type; use INT64 or STRING instead",
        )),
        _ => parse_parameterized(input, &upper, depth, full),
    }
}

/// Parse the parameterized forms: `STRUCT<...>`, `ARRAY<...>(len)`,
/// `FIXEDSTRING(n)`, `FIXED_STRING(n)`, `VECTOR(n)`, `VECTOR_DENSE(n)` and
/// `VECTOR_SPARSE(n)`.
fn parse_parameterized(
    input: &str,
    upper: &str,
    depth: usize,
    full: &str,
) -> Result<DataType, ParseDataTypeError> {
    if upper.starts_with("STRUCT<") {
        let (content, rest) = take_angle_brackets(input)
            .ok_or_else(|| ParseDataTypeError::new(full, "unterminated STRUCT: missing '>'"))?;
        if !rest.trim().is_empty() {
            return Err(ParseDataTypeError::new(
                full,
                format!(
                    "unexpected trailing content after STRUCT: '{}'",
                    rest.trim()
                ),
            ));
        }
        let fields = parse_struct_fields(content, depth, full)?;
        return Ok(DataType::Struct(Arc::new(StructTypeInfo::new(fields))));
    }
    if upper.starts_with("ARRAY<") {
        let (content, rest) = take_angle_brackets(input)
            .ok_or_else(|| ParseDataTypeError::new(full, "unterminated ARRAY: missing '>'"))?;
        let element = parse_type(content, depth + 1, full)?;
        let len = parse_optional_length(rest, full)?;
        return Ok(DataType::Array(Arc::new(ArrayTypeInfo::new(element, len))));
    }
    for (prefix, kind) in [
        ("FIXEDSTRING(", SizedKind::FixedString),
        ("FIXED_STRING(", SizedKind::FixedString),
        ("VECTOR_DENSE(", SizedKind::VectorDense),
        ("VECTOR_SPARSE(", SizedKind::VectorSparse),
        ("VECTOR(", SizedKind::VectorDense),
    ] {
        if let Some(rest) = upper.strip_prefix(prefix) {
            let n = parse_size_param(rest, full)?;
            return Ok(match kind {
                SizedKind::FixedString => DataType::FixedString(n),
                SizedKind::VectorDense => DataType::VectorDense(n),
                SizedKind::VectorSparse => DataType::VectorSparse(n),
            });
        }
    }
    Err(ParseDataTypeError::new(full, "unknown data type name"))
}

enum SizedKind {
    FixedString,
    VectorDense,
    VectorSparse,
}

/// Split `input` at the angle bracket that closes the opening '<', returning
/// `(content, rest)` where content excludes the brackets.
fn take_angle_brackets(input: &str) -> Option<(&str, &str)> {
    let open = input.find('<')?;
    let mut depth = 0usize;
    for (i, c) in input[open + 1..].char_indices() {
        let i = open + 1 + i;
        match c {
            '<' => depth += 1,
            '>' if depth == 0 => return Some((&input[open + 1..i], &input[i + 1..])),
            '>' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Split on commas that are not nested inside angle brackets.
fn split_top_level_commas(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn parse_struct_fields(
    content: &str,
    depth: usize,
    full: &str,
) -> Result<Vec<(String, DataType)>, ParseDataTypeError> {
    let mut fields = Vec::new();
    for part in split_top_level_commas(content) {
        let part = part.trim();
        if part.is_empty() {
            return Err(ParseDataTypeError::new(
                full,
                "STRUCT field must not be empty".to_string(),
            ));
        }
        let Some((name, ty)) = part.split_once(char::is_whitespace) else {
            return Err(ParseDataTypeError::new(
                full,
                format!("STRUCT field '{part}' requires a type"),
            ));
        };
        let (name, ty) = (name.trim(), ty.trim());
        if name.is_empty() || ty.is_empty() {
            return Err(ParseDataTypeError::new(
                full,
                format!("STRUCT field '{part}' requires a name and a type"),
            ));
        }
        // Field names are preserved as-is; the type is parsed recursively.
        fields.push((name.to_string(), parse_type(ty, depth + 1, full)?));
    }
    if fields.is_empty() {
        return Err(ParseDataTypeError::new(
            full,
            "STRUCT requires at least one field".to_string(),
        ));
    }
    Ok(fields)
}

/// Parse the optional `(len)` suffix of an ARRAY; `rest` is everything after
/// the closing '>'.
fn parse_optional_length(rest: &str, full: &str) -> Result<Option<usize>, ParseDataTypeError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Ok(None);
    }
    if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        let inner = inner.trim();
        if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit()) {
            return Err(ParseDataTypeError::new(
                full,
                format!("invalid ARRAY length '{inner}': expected a non-negative integer"),
            ));
        }
        let len = inner.parse::<usize>().map_err(|_| {
            ParseDataTypeError::new(full, format!("ARRAY length '{inner}' is out of range"))
        })?;
        Ok(Some(len))
    } else {
        Err(ParseDataTypeError::new(
            full,
            format!("unexpected trailing content after ARRAY: '{}'", rest),
        ))
    }
}

/// Parse the size parameter of `FIXEDSTRING(n)` / `VECTOR_DENSE(n)` /
/// `VECTOR_SPARSE(n)`. `rest` is everything after the opening prefix.
fn parse_size_param(rest: &str, full: &str) -> Result<usize, ParseDataTypeError> {
    let Some(inner) = rest.strip_suffix(')') else {
        return Err(ParseDataTypeError::new(
            full,
            format!("missing ')' after parameter: '{}'", rest),
        ));
    };
    let inner = inner.trim();
    if inner.is_empty() || !inner.chars().all(|c| c.is_ascii_digit()) {
        return Err(ParseDataTypeError::new(
            full,
            format!("invalid size parameter '{inner}': expected a non-negative integer"),
        ));
    }
    inner.parse::<usize>().map_err(|_| {
        ParseDataTypeError::new(full, format!("size parameter '{inner}' is out of range"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every `DataType` variant, including parameterized forms, with the
    /// exact metadata the `Display` impl emits.
    fn all_data_types() -> Vec<DataType> {
        vec![
            DataType::Empty,
            DataType::Unknown,
            DataType::Null,
            DataType::Bool,
            DataType::SmallInt,
            DataType::Int,
            DataType::BigInt,
            DataType::Float,
            DataType::Double,
            DataType::Decimal128,
            DataType::String,
            DataType::Date,
            DataType::Time,
            DataType::DateTime,
            DataType::Vertex,
            DataType::Edge,
            DataType::Path,
            DataType::List,
            DataType::Map,
            DataType::Set,
            DataType::Geography,
            DataType::DataSet,
            DataType::FixedString(8),
            DataType::Blob,
            DataType::Vector,
            DataType::VectorDense(3),
            DataType::VectorSparse(3),
            DataType::Json,
            DataType::JsonB,
            DataType::Uuid,
            DataType::Interval,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                ("city".to_string(), DataType::String),
                (
                    "geo".to_string(),
                    DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                        "lat".to_string(),
                        DataType::Double,
                    )]))),
                ),
            ]))),
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3)))),
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::String, None))),
        ]
    }

    /// Exhaustive gate: every `DataType` variant must be listed here.
    /// Adding a variant requires extending this match and the roundtrip list.
    #[test]
    fn test_exhaustive_variant_gate() {
        for data_type in all_data_types() {
            match data_type {
                DataType::Empty
                | DataType::Unknown
                | DataType::Null
                | DataType::Bool
                | DataType::SmallInt
                | DataType::Int
                | DataType::BigInt
                | DataType::Float
                | DataType::Double
                | DataType::Decimal128
                | DataType::String
                | DataType::Date
                | DataType::Time
                | DataType::DateTime
                | DataType::Vertex
                | DataType::Edge
                | DataType::Path
                | DataType::List
                | DataType::Map
                | DataType::Set
                | DataType::Geography
                | DataType::DataSet
                | DataType::FixedString(_)
                | DataType::Blob
                | DataType::Vector
                | DataType::VectorDense(_)
                | DataType::VectorSparse(_)
                | DataType::Json
                | DataType::JsonB
                | DataType::Uuid
                | DataType::Interval
                | DataType::Struct(_)
                | DataType::Array(_) => {}
            }
        }
    }

    /// Display output of every variant roundtrips back through `from_str`.
    #[test]
    fn test_display_roundtrip_for_all_variants() {
        for data_type in all_data_types() {
            let s = data_type.to_string();
            let parsed =
                DataType::from_str(&s).unwrap_or_else(|e| panic!("cannot parse '{}': {e}", s));
            assert_eq!(parsed, data_type, "roundtrip mismatch for '{s}'");
        }
    }

    #[test]
    fn test_postgresql_integer_aliases() {
        assert_eq!("INT2".parse::<DataType>(), Ok(DataType::SmallInt));
        assert_eq!("INT4".parse::<DataType>(), Ok(DataType::Int));
        assert_eq!("INT8".parse::<DataType>(), Ok(DataType::BigInt));
        assert_eq!("INT16".parse::<DataType>(), Ok(DataType::Int));
        assert_eq!("INT32".parse::<DataType>(), Ok(DataType::Int));
        assert_eq!("INT64".parse::<DataType>(), Ok(DataType::BigInt));
        assert_eq!("INTEGER".parse::<DataType>(), Ok(DataType::Int));
        assert_eq!("BOOLEAN".parse::<DataType>(), Ok(DataType::Bool));
        assert_eq!("VARCHAR".parse::<DataType>(), Ok(DataType::String));
        assert_eq!("TEXT".parse::<DataType>(), Ok(DataType::String));
        assert_eq!("FLOAT4".parse::<DataType>(), Ok(DataType::Float));
        assert_eq!("FLOAT8".parse::<DataType>(), Ok(DataType::Double));
        assert_eq!("DOUBLE PRECISION".parse::<DataType>(), Ok(DataType::Double));
        // Whitespace between the words is normalized before matching.
        assert_eq!(
            "DOUBLE  PRECISION".parse::<DataType>(),
            Ok(DataType::Double)
        );
    }

    #[test]
    fn test_timestamp_normalizes_to_datetime() {
        // TIMESTAMP is not a distinct data type; it normalizes to DATETIME.
        assert_eq!("TIMESTAMP".parse::<DataType>(), Ok(DataType::DateTime));
        assert_eq!("timestamp".parse::<DataType>(), Ok(DataType::DateTime));
    }

    #[test]
    fn test_vid_is_rejected_with_hint() {
        let err = "VID".parse::<DataType>().unwrap_err();
        assert_eq!(err.name, "VID");
        assert!(
            err.reason.contains("INT64"),
            "VID error should suggest a replacement: {}",
            err.reason
        );
        assert!("vid".parse::<DataType>().is_err());
    }

    #[test]
    fn test_case_insensitive_and_whitespace() {
        assert_eq!("  int  ".parse::<DataType>(), Ok(DataType::Int));
        assert_eq!(
            "fIxEdStRiNg(8)".parse::<DataType>(),
            Ok(DataType::FixedString(8))
        );
        assert_eq!("double precision".parse::<DataType>(), Ok(DataType::Double));
        assert_eq!(
            "struct<a int>".parse::<DataType>(),
            Ok(DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                "a".to_string(),
                DataType::Int
            )]))))
        );
    }

    #[test]
    fn test_parameterized_forms() {
        assert_eq!(
            "FIXEDSTRING(8)".parse::<DataType>(),
            Ok(DataType::FixedString(8))
        );
        assert_eq!(
            "FIXED_STRING(8)".parse::<DataType>(),
            Ok(DataType::FixedString(8))
        );
        assert_eq!(
            "VECTOR(3)".parse::<DataType>(),
            Ok(DataType::VectorDense(3))
        );
        assert_eq!(
            "VECTOR_DENSE(3)".parse::<DataType>(),
            Ok(DataType::VectorDense(3))
        );
        assert_eq!(
            "VECTOR_SPARSE(3)".parse::<DataType>(),
            Ok(DataType::VectorSparse(3))
        );
        // VECTOR(3) normalizes to VECTOR_DENSE(3): Display roundtrip holds.
        assert_eq!(
            "VECTOR(3)".parse::<DataType>().unwrap().to_string(),
            "VECTOR_DENSE(3)"
        );
    }

    #[test]
    fn test_invalid_parameters_are_rejected() {
        for bad in [
            "FIXEDSTRING(abc)",
            "FIXEDSTRING(-1)",
            "FIXEDSTRING()",
            "FIXEDSTRING(12.5)",
            "FIXEDSTRING(999999999999999999999999999999)",
            "FIXEDSTRING(8) junk",
            "FIXEDSTRING",
            "VECTOR_DENSE(3.5)",
            "VECTOR_DENSE(3",
            "VECTOR_SPARSE()",
            "VECTOR(3))",
        ] {
            assert!(
                DataType::from_str(bad).is_err(),
                "expected error for '{bad}'"
            );
        }
    }

    #[test]
    fn test_struct_and_array_composites() {
        let ty =
            DataType::from_str("STRUCT<city STRING, geo STRUCT<lat DOUBLE, lon DOUBLE>>").unwrap();
        assert_eq!(
            ty,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                ("city".to_string(), DataType::String),
                (
                    "geo".to_string(),
                    DataType::Struct(Arc::new(StructTypeInfo::new(vec![
                        ("lat".to_string(), DataType::Double),
                        ("lon".to_string(), DataType::Double),
                    ]))),
                ),
            ])))
        );

        let ty = DataType::from_str("ARRAY<DOUBLE>(3)").unwrap();
        assert_eq!(
            ty,
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::Double, Some(3))))
        );

        // Variable-length array (no length suffix).
        let ty = DataType::from_str("ARRAY<STRING>").unwrap();
        assert_eq!(
            ty,
            DataType::Array(Arc::new(ArrayTypeInfo::new(DataType::String, None)))
        );

        // Field names are preserved as-is; type names are case-insensitive.
        let ty = DataType::from_str("struct<City int>").unwrap();
        assert_eq!(
            ty,
            DataType::Struct(Arc::new(StructTypeInfo::new(vec![(
                "City".to_string(),
                DataType::Int,
            )])))
        );
    }

    #[test]
    fn test_composite_errors() {
        for bad in [
            "STRUCT<>",
            "STRUCT<a>",
            "STRUCT<a INT> junk",
            "STRUCT",
            "ARRAY<>",
            "ARRAY<DOUBLE> junk",
            "ARRAY<DOUBLE>(abc)",
            "ARRAY<DOUBLE>(-1)",
            "ARRAY<DOUBLE>(3",
        ] {
            assert!(
                DataType::from_str(bad).is_err(),
                "expected error for '{bad}'"
            );
        }
    }

    #[test]
    fn test_unknown_type_is_rejected() {
        let err = "FIZZ".parse::<DataType>().unwrap_err();
        assert_eq!(err.name, "FIZZ");
        assert!("".parse::<DataType>().is_err());
    }

    #[test]
    fn test_nesting_depth_limit() {
        // 15 levels are allowed; 16 must error.
        let mut s = String::from("ARRAY<");
        for _ in 0..14 {
            s.push_str("ARRAY<");
        }
        s.push_str("INT");
        for _ in 0..15 {
            s.push('>');
        }
        assert!(
            DataType::from_str(&s).is_ok(),
            "15 levels of nesting must parse"
        );

        let mut deep = String::from("ARRAY<");
        for _ in 0..15 {
            deep.push_str("ARRAY<");
        }
        deep.push_str("INT");
        for _ in 0..16 {
            deep.push('>');
        }
        let err = DataType::from_str(&deep).unwrap_err();
        assert!(
            err.reason.contains("maximum depth"),
            "depth error should mention the limit: {}",
            err.reason
        );
    }
}
