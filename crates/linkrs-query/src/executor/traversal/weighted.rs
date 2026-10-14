//! Shared edge-weight reading for weighted path semantics.
//!
//! Both `MATCH ... *WEIGHTED(prop)` (traversal runtime) and
//! `FIND SHORTEST PATH ... WEIGHT prop` (path algorithms) must agree on
//! what an edge costs, so the rule lives here:
//!
//! - `ranking` reads the edge's ranking field;
//! - any other name reads the edge property of that name;
//! - a missing or non-numeric value costs `1.0`;
//! - negative (and non-finite) values are clamped to `0.0` so Dijkstra's
//!   non-negative assumption holds.

use linkrs_core::{Edge, Value};

/// Read a numeric edge weight named by `name`, defaulting to `1.0` when the
/// property is absent or non-numeric, and clamping negatives to `0.0`.
pub(crate) fn read_edge_weight(edge: &Edge, name: &str) -> f64 {
    if name == "ranking" {
        let value = edge.ranking() as f64;
        return clamp_weight(value);
    }
    let Some(raw) = edge.get_property(name) else {
        return 1.0;
    };
    let value = match raw {
        Value::Float(v) => f64::from(*v),
        Value::Double(v) => *v,
        Value::Int(v) => f64::from(*v),
        Value::BigInt(v) => *v as f64,
        Value::SmallInt(v) => f64::from(*v),
        _ => return 1.0,
    };
    clamp_weight(value)
}

fn clamp_weight(value: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn edge_with(props: HashMap<Arc<str>, Value>) -> Edge {
        let vid =
            |n| linkrs_core::types::storage_ids::VertexId::try_from_int64(n).expect("valid vid");
        Edge {
            src: vid(1),
            dst: vid(2),
            edge_type: "knows".to_string(),
            ranking: 7,
            props,
        }
    }

    #[test]
    fn missing_property_defaults_to_one() {
        assert_eq!(read_edge_weight(&edge_with(HashMap::new()), "weight"), 1.0);
    }

    #[test]
    fn numeric_properties_are_used() {
        let mut props = HashMap::new();
        props.insert(Arc::from("weight"), Value::Double(2.5));
        props.insert(Arc::from("bad"), Value::Bool(true));
        let edge = edge_with(props);
        assert_eq!(read_edge_weight(&edge, "weight"), 2.5);
        assert_eq!(read_edge_weight(&edge, "bad"), 1.0);
    }

    #[test]
    fn every_numeric_kind_is_read() {
        let mut props = HashMap::new();
        props.insert(Arc::from("small"), Value::SmallInt(2));
        props.insert(Arc::from("int"), Value::Int(3));
        props.insert(Arc::from("big"), Value::BigInt(4));
        props.insert(Arc::from("float"), Value::Float(5.5));
        let edge = edge_with(props);
        assert_eq!(read_edge_weight(&edge, "small"), 2.0);
        assert_eq!(read_edge_weight(&edge, "int"), 3.0);
        assert_eq!(read_edge_weight(&edge, "big"), 4.0);
        assert_eq!(read_edge_weight(&edge, "float"), 5.5);
    }

    #[test]
    fn negative_and_non_finite_clamp_to_zero() {
        let mut props = HashMap::new();
        props.insert(Arc::from("neg"), Value::Double(-3.0));
        props.insert(Arc::from("inf"), Value::Double(f64::INFINITY));
        let edge = edge_with(props);
        assert_eq!(read_edge_weight(&edge, "neg"), 0.0);
        assert_eq!(read_edge_weight(&edge, "inf"), 0.0);
    }

    #[test]
    fn ranking_reads_the_rank_field() {
        assert_eq!(read_edge_weight(&edge_with(HashMap::new()), "ranking"), 7.0);
    }
}
