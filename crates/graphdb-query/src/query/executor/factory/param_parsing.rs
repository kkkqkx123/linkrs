//! Parameter Parsing Module
//!
//! Responsible for parsing string parameters into execution configuration types.
//! Used by executor builders to convert user input into internal representations.

use crate::core::{EdgeDirection, Value};

/// Parse the vertex ID string into a list of Values.
/// Supports multiple IDs separated by commas.
/// Tries to parse as integer first, then falls back to string.
pub fn parse_vertex_ids(src_vids: &str) -> Vec<Value> {
    src_vids
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            // Try to parse as integer first
            if let Ok(i) = s.parse::<i64>() {
                Value::BigInt(i)
            } else {
                Value::String(s.to_string())
            }
        })
        .collect()
}

/// Parse a string representing an edge direction into the EdgeDirection enumeration.
pub fn parse_edge_direction(direction_str: &str) -> EdgeDirection {
    match direction_str.to_uppercase().as_str() {
        "OUT" => EdgeDirection::Out,
        "IN" => EdgeDirection::In,
        _ => EdgeDirection::Both,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vertex_ids() {
        let result = parse_vertex_ids("1, 2, 3");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], Value::Int(1));
        assert_eq!(result[1], Value::Int(2));
        assert_eq!(result[2], Value::Int(3));
    }

    #[test]
    fn test_parse_vertex_ids_mixed() {
        let result = parse_vertex_ids("1, abc, 3");
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], Value::Int(1));
        assert_eq!(result[1], Value::String("abc".to_string()));
        assert_eq!(result[2], Value::Int(3));
    }

    #[test]
    fn test_parse_edge_direction() {
        assert_eq!(parse_edge_direction("OUT"), EdgeDirection::Out);
        assert_eq!(parse_edge_direction("out"), EdgeDirection::Out);
        assert_eq!(parse_edge_direction("IN"), EdgeDirection::In);
        assert_eq!(parse_edge_direction("in"), EdgeDirection::In);
        assert_eq!(parse_edge_direction("BOTH"), EdgeDirection::Both);
        assert_eq!(parse_edge_direction("unknown"), EdgeDirection::Both);
    }
}
