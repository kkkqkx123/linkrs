//! Integrated testing of built-in functions
//!
//! Scope: two layers.
//! - Unit layer (direct `FunctionRegistry::execute` on hand-built values)
//!   pins function semantics.
//! - Pipeline layer (tests at the bottom of this file going through
//!   `TestScenario` + SQL `RETURN`) proves the functions are reachable
//!   and correctly wired end to end.
//!
//! Test scope:
//! Image-related functions: id, tags, labels, properties, type, src, dst, rank
//! Container operation functions: head, last, tail, size, range, keys
//! Path function: nodes, relationships
//! Mathematical functions: bit_and, bit_or, bit_xor, asin, acos, atan, cbrt, hypot
//! String function: split
//! Practical functions: coalesce, hash

mod common;

use common::test_scenario::TestScenario;
use graphdb_core::types::VertexId;
use graphdb_core::vertex_edge_path::{Edge, Path, Step, Tag, Vertex};
use graphdb_core::{List, NullType, Value};
use graphdb_query::executor::expression::functions::FunctionRegistry;
use std::collections::HashMap;

/// Create vertices for testing purposes.
fn create_test_vertex(vid: i64, tags: Vec<(&str, HashMap<&str, Value>)>) -> Vertex {
    let tags: Vec<Tag> = tags
        .into_iter()
        .map(|(name, props)| {
            let props: HashMap<String, Value> =
                props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
            Tag::new(name.to_string(), props)
        })
        .collect();
    Vertex::new(VertexId::from_int64(vid), tags)
}

/// Create edges for testing purposes.
fn create_test_edge(
    src: i64,
    dst: i64,
    edge_type: &str,
    rank: i64,
    props: HashMap<&str, Value>,
) -> Edge {
    let props: HashMap<String, Value> =
        props.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
    Edge::new(
        VertexId::from_int64(src),
        VertexId::from_int64(dst),
        edge_type.to_string(),
        rank,
        props,
    )
}

/// Create a path for testing purposes.
fn create_test_path() -> Path {
    let v1 = create_test_vertex(
        1,
        vec![("Person", {
            let mut m = HashMap::new();
            m.insert("name", Value::string("Alice"));
            m.insert("age", Value::Int(30));
            m
        })],
    );
    let v2 = create_test_vertex(
        2,
        vec![("Person", {
            let mut m = HashMap::new();
            m.insert("name", Value::string("Bob"));
            m.insert("age", Value::Int(25));
            m
        })],
    );
    let v3 = create_test_vertex(
        3,
        vec![("Person", {
            let mut m = HashMap::new();
            m.insert("name", Value::string("Charlie"));
            m.insert("age", Value::Int(35));
            m
        })],
    );

    let e1 = create_test_edge(1, 2, "KNOWS", 0, HashMap::new());
    let e2 = create_test_edge(2, 3, "KNOWS", 0, HashMap::new());

    let mut path = Path::new(v1);
    path.add_step(Step {
        edge: Box::new(e1),
        dst: Box::new(v2.clone()),
    });
    path.add_step(Step {
        edge: Box::new(e2),
        dst: Box::new(v3.clone()),
    });
    path
}

// ==================== Testing of functions related to graphics ====================

#[test]
fn test_id_function() {
    let registry = FunctionRegistry::new();
    let vertex = create_test_vertex(100, vec![("Person", HashMap::new())]);

    let result = registry.execute("id", &[Value::Vertex(Box::new(vertex))]);
    assert!(result.is_ok());
    assert_eq!(result.expect("idshould succeed"), Value::Int(100));
}

#[test]
fn test_tags_function() {
    let registry = FunctionRegistry::new();
    let vertex = create_test_vertex(
        1,
        vec![("Person", HashMap::new()), ("Employee", HashMap::new())],
    );

    let result = registry.execute("tags", &[Value::Vertex(Box::new(vertex))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("tagsshould succeed") {
        assert_eq!(list.values.len(), 2);
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_labels_function() {
    let registry = FunctionRegistry::new();
    let vertex = create_test_vertex(1, vec![("Person", HashMap::new())]);

    let result = registry.execute("labels", &[Value::Vertex(Box::new(vertex))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("labelsshould succeed") {
        assert_eq!(list.values.len(), 1);
        assert_eq!(list.values[0], Value::string("Person"));
    } else {
        panic!("The expected return value is of the list type.");
    }
}

#[test]
fn test_properties_vertex_function() {
    let registry = FunctionRegistry::new();
    let vertex = create_test_vertex(
        1,
        vec![("Person", {
            let mut m = HashMap::new();
            m.insert("name", Value::string("Alice"));
            m.insert("age", Value::Int(30));
            m
        })],
    );

    let result = registry.execute("properties", &[Value::Vertex(Box::new(vertex))]);
    assert!(result.is_ok());

    if let Value::Map(map) = result.expect("propertiesshould succeed") {
        assert!(map.contains_key(&Value::string("name")));
        assert!(map.contains_key(&Value::string("age")));
        assert_eq!(
            map.get(&Value::string("name")),
            Some(&Value::string("Alice"))
        );
        assert_eq!(map.get(&Value::string("age")), Some(&Value::Int(30)));
    } else {
        panic!("The expected return type is a map.");
    }
}

#[test]
fn test_type_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(1, 2, "KNOWS", 0, HashMap::new());

    let result = registry.execute("type", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert_eq!(result.expect("typeshould succeed"), Value::string("KNOWS"));
}

#[test]
fn test_src_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(100, 200, "KNOWS", 0, HashMap::new());

    let result = registry.execute("src", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert_eq!(result.expect("srcshould succeed"), Value::Int(100));
}

#[test]
fn test_dst_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(100, 200, "KNOWS", 0, HashMap::new());

    let result = registry.execute("dst", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert_eq!(result.expect("dstshould succeed"), Value::Int(200));
}

#[test]
fn test_rank_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(1, 2, "KNOWS", 42, HashMap::new());

    let result = registry.execute("rank", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert_eq!(result.expect("rankshould succeed"), Value::BigInt(42));
}

// ==================== Testing of Container Operation Functions ====================

#[test]
fn test_head_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    let result = registry.execute("head", &[list]);
    assert!(result.is_ok());
    assert_eq!(result.expect("headshould succeed"), Value::Int(1));
}

#[test]
fn test_last_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    let result = registry.execute("last", &[list]);
    assert!(result.is_ok());
    assert_eq!(result.expect("lastshould succeed"), Value::Int(3));
}

#[test]
fn test_tail_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    let result = registry.execute("tail", &[list]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("tailshould succeed") {
        assert_eq!(list.values.len(), 2);
        assert_eq!(list.values[0], Value::Int(2));
        assert_eq!(list.values[1], Value::Int(3));
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_size_list_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    let result = registry.execute("size", &[list]);
    assert!(result.is_ok());
    assert_eq!(result.expect("sizeshould succeed"), Value::Int(3));
}

#[test]
fn test_size_string_function() {
    let registry = FunctionRegistry::new();
    let string = Value::string("hello");

    let result = registry.execute("size", &[string]);
    assert!(result.is_ok());
    assert_eq!(result.expect("sizeshould succeed"), Value::Int(5));
}

#[test]
fn test_range_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("range", &[Value::Int(1), Value::Int(5)]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("rangeshould succeed") {
        assert_eq!(list.values.len(), 5);
        assert_eq!(list.values[0], Value::Int(1));
        assert_eq!(list.values[4], Value::Int(5));
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_range_with_step_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("range", &[Value::Int(0), Value::Int(10), Value::Int(2)]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("rangeshould succeed") {
        assert_eq!(list.values.len(), 6);
        assert_eq!(list.values[0], Value::Int(0));
        assert_eq!(list.values[1], Value::Int(2));
        assert_eq!(list.values[5], Value::Int(10));
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_keys_map_function() {
    let registry = FunctionRegistry::new();
    let mut map = HashMap::new();
    map.insert(Value::string("name"), Value::string("Alice"));
    map.insert(Value::string("age"), Value::Int(30));
    let map_value = Value::Map(Box::new(map));

    let result = registry.execute("keys", &[map_value]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("keysshould succeed") {
        assert_eq!(list.values.len(), 2);
        assert!(list.values.contains(&Value::string("name")));
        assert!(list.values.contains(&Value::string("age")));
    } else {
        panic!("The expected return type is a list.");
    }
}

// ==================== Path Function Testing ====================

#[test]
fn test_nodes_function() {
    let registry = FunctionRegistry::new();
    let path = create_test_path();

    let result = registry.execute("nodes", &[Value::Path(Box::new(path))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("nodesshould succeed") {
        assert_eq!(list.values.len(), 3);
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_relationships_function() {
    let registry = FunctionRegistry::new();
    let path = create_test_path();

    let result = registry.execute("relationships", &[Value::Path(Box::new(path))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("relationshipsshould succeed") {
        assert_eq!(list.values.len(), 2);
    } else {
        panic!("The expected return value is of the list type.");
    }
}

// ==================== Testing of Mathematical Functions ====================

#[test]
fn test_bit_and_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("bit_and", &[Value::Int(0b1010), Value::Int(0b1100)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("bit_andshould succeed"), Value::Int(0b1000));
}

#[test]
fn test_bit_or_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("bit_or", &[Value::Int(0b1010), Value::Int(0b1100)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("bit_orshould succeed"), Value::Int(0b1110));
}

#[test]
fn test_bit_xor_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("bit_xor", &[Value::Int(0b1010), Value::Int(0b1100)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("bit_xorshould succeed"), Value::Int(0b0110));
}

#[test]
fn test_asin_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("asin", &[Value::Float(0.5)]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("asinshould succeed") {
        assert!((val - std::f32::consts::PI / 6.0).abs() < 1e-6);
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

#[test]
fn test_acos_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("acos", &[Value::Float(0.5)]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("acosshould succeed") {
        assert!((val - std::f32::consts::PI / 3.0).abs() < 1e-6);
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

#[test]
fn test_atan_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("atan", &[Value::Float(1.0)]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("atanshould succeed") {
        assert!((val - std::f32::consts::PI / 4.0).abs() < 1e-6);
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

#[test]
fn test_cbrt_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("cbrt", &[Value::Float(27.0)]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("cbrtshould succeed") {
        assert!((val - 3.0).abs() < 1e-10);
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

#[test]
fn test_hypot_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("hypot", &[Value::Float(3.0), Value::Float(4.0)]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("hypotshould succeed") {
        assert!((val - 5.0).abs() < 1e-10);
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

// ==================== Testing of String Functions ====================

#[test]
fn test_split_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "split",
        &[Value::string("hello,world,test"), Value::string(",")],
    );
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("splitshould succeed") {
        assert_eq!(list.values.len(), 3);
        assert_eq!(list.values[0], Value::string("hello"));
        assert_eq!(list.values[1], Value::string("world"));
        assert_eq!(list.values[2], Value::string("test"));
    } else {
        panic!("The expected return value is of the list type.");
    }
}

// ==================== Testing of Practical Functions ====================

#[test]
fn test_coalesce_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "coalesce",
        &[
            Value::Null(NullType::Null),
            Value::Int(42),
            Value::string("test"),
        ],
    );
    assert!(result.is_ok());
    assert_eq!(result.expect("coalesceshould succeed"), Value::Int(42));
}

#[test]
fn test_coalesce_all_null() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "coalesce",
        &[Value::Null(NullType::Null), Value::Null(NullType::Null)],
    );
    assert!(result.is_ok());
    assert_eq!(
        result.expect("coalesceshould succeed"),
        Value::Null(NullType::Null)
    );
}

#[test]
fn test_hash_string_function() {
    let registry = FunctionRegistry::new();

    let result1 = registry.execute("hash", &[Value::string("test")]);
    let result2 = registry.execute("hash", &[Value::string("test")]);

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(
        result1.expect("hashshould succeed"),
        result2.expect("hashshould succeed")
    );
}

#[test]
fn test_hash_int_function() {
    let registry = FunctionRegistry::new();

    let result1 = registry.execute("hash", &[Value::Int(12345)]);
    let result2 = registry.execute("hash", &[Value::Int(12345)]);

    assert!(result1.is_ok());
    assert!(result2.is_ok());
    assert_eq!(
        result1.expect("hashshould succeed"),
        result2.expect("hashshould succeed")
    );
}

// ==================== NULL Handling Test ====================

#[test]
fn test_null_handling() {
    let registry = FunctionRegistry::new();

    //  id(NULL)
    let result = registry.execute("id", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("idshould succeed"),
        Value::Null(NullType::Null)
    );

    //  tags(NULL)
    let result = registry.execute("tags", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("tagsshould succeed"),
        Value::Null(NullType::Null)
    );

    //  head(NULL)
    let result = registry.execute("head", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("headshould succeed"),
        Value::Null(NullType::Null)
    );

    //  size(NULL)
    let result = registry.execute("size", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("sizeshould succeed"),
        Value::Null(NullType::Null)
    );

    //  nodes(NULL)
    let result = registry.execute("nodes", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("nodesshould succeed"),
        Value::Null(NullType::Null)
    );

    //  hash(NULL)
    let result = registry.execute("hash", &[Value::Null(NullType::Null)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("hashshould succeed"),
        Value::Null(NullType::Null)
    );
}

// ==================== Boundary Case Testing ====================

#[test]
fn test_empty_list_operations() {
    let registry = FunctionRegistry::new();
    let empty_list = Value::List(Box::new(List { values: vec![] }));

    // head()  NULL
    let result = registry.execute("head", std::slice::from_ref(&empty_list));
    assert!(result.is_ok());
    assert_eq!(
        result.expect("headshould succeed"),
        Value::Null(NullType::Null)
    );

    // last()  NULL
    let result = registry.execute("last", std::slice::from_ref(&empty_list));
    assert!(result.is_ok());
    assert_eq!(
        result.expect("lastshould succeed"),
        Value::Null(NullType::Null)
    );

    // tail()
    let result = registry.execute("tail", std::slice::from_ref(&empty_list));
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("tailshould succeed") {
        assert!(list.values.is_empty());
    } else {
        panic!("The expected return type is a list.");
    }

    // size()  0
    let result = registry.execute("size", &[empty_list]);
    assert!(result.is_ok());
    assert_eq!(result.expect("sizeshould succeed"), Value::Int(0));
}

#[test]
fn test_empty_path() {
    let registry = FunctionRegistry::new();
    let v1 = create_test_vertex(1, vec![("Person", HashMap::new())]);
    let empty_path = Path::new(v1);

    // nodes(path)
    let result = registry.execute("nodes", &[Value::Path(Box::new(empty_path.clone()))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("nodesshould succeed") {
        assert_eq!(list.values.len(), 1);
    } else {
        panic!("The expected return type is a list.");
    }

    // relationships(path)
    let result = registry.execute("relationships", &[Value::Path(Box::new(empty_path))]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("relationshipsshould succeed") {
        assert!(list.values.is_empty());
    } else {
        panic!("The expected return type is a list.");
    }
}

#[test]
fn test_single_element_list() {
    let registry = FunctionRegistry::new();
    let single_list = Value::List(Box::new(List {
        values: vec![Value::Int(42)],
    }));

    let result = registry.execute("head", std::slice::from_ref(&single_list));
    assert!(result.is_ok());
    assert_eq!(result.expect("headshould succeed"), Value::Int(42));

    let result = registry.execute("last", std::slice::from_ref(&single_list));
    assert!(result.is_ok());
    assert_eq!(result.expect("lastshould succeed"), Value::Int(42));

    let result = registry.execute("tail", &[single_list]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("tailshould succeed") {
        assert!(list.values.is_empty());
    } else {
        panic!("The expected return type is a list.");
    }
}

// ==================== Additional tests for date and time functions ====================

#[test]
fn test_time_function() {
    let registry = FunctionRegistry::new();

    //  time()
    let result = registry.execute("time", &[]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("timeshould succeed"),
        Value::Time(_)
    ));

    //  time(string)
    let result = registry.execute("time", &[Value::string("14:30:00")]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("timeshould succeed"),
        Value::Time(_)
    ));
}

#[test]
fn test_datetime_function() {
    let registry = FunctionRegistry::new();

    //  datetime()
    let result = registry.execute("datetime", &[]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("datetimeshould succeed"),
        Value::DateTime(_)
    ));

    //  datetime(string)
    let result = registry.execute("datetime", &[Value::string("2024-01-15 14:30:00")]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("datetimeshould succeed"),
        Value::DateTime(_)
    ));
}

#[test]
fn test_timestamp_function() {
    let registry = FunctionRegistry::new();

    //  timestamp()
    let result = registry.execute("timestamp", &[]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("timestampshould succeed"),
        Value::BigInt(_)
    ));

    //  timestamp(datetime)
    let dt = Value::DateTime(graphdb_core::value::DateTimeValue {
        year: 2024,
        month: 1,
        day: 15,
        hour: 0,
        minute: 0,
        sec: 0,
        microsec: 0,
    });
    let result = registry.execute("timestamp", &[dt]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("timestampshould succeed"),
        Value::BigInt(_)
    ));
}

// ==================== Testing of functions related to new images =====================

#[test]
fn test_startnode_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(100, 200, "KNOWS", 0, HashMap::new());

    let result = registry.execute("startnode", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("startnodeshould succeed"),
        Value::Vertex(_)
    ));
}

#[test]
fn test_endnode_function() {
    let registry = FunctionRegistry::new();
    let edge = create_test_edge(100, 200, "KNOWS", 0, HashMap::new());

    let result = registry.execute("endnode", &[Value::Edge(Box::new(edge))]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("endnodeshould succeed"),
        Value::Vertex(_)
    ));
}

// ==================== Added tests for new mathematical functions ====================

#[test]
fn test_sign_function() {
    let registry = FunctionRegistry::new();

    // Positive numbers
    let result = registry.execute("sign", &[Value::Int(42)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("signshould succeed"), Value::Int(1));

    // negative numbers
    let result = registry.execute("sign", &[Value::Int(-42)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("signshould succeed"), Value::Int(-1));

    // Zero
    let result = registry.execute("sign", &[Value::Int(0)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("signshould succeed"), Value::Int(0));

    // Floating-point number
    let result = registry.execute("sign", &[Value::Float(-2.5_f32)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("signshould succeed"), Value::Int(-1));
}

#[test]
fn test_rand_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("rand", &[]);
    assert!(result.is_ok());

    if let Value::Float(val) = result.expect("randshould succeed") {
        assert!((0.0..1.0).contains(&val));
    } else {
        panic!("The expectation is to receive a value of the floating-point type.");
    }
}

#[test]
fn test_rand32_function() {
    let registry = FunctionRegistry::new();

    // No parameters
    let result = registry.execute("rand32", &[]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("rand32should succeed"),
        Value::Int(_)
    ));

    // There is a range.
    let result = registry.execute("rand32", &[Value::Int(100)]);
    assert!(result.is_ok());
    if let Value::Int(val) = result.expect("rand32should succeed") {
        assert!((0..100).contains(&val));
    } else {
        panic!("The expected return value is of the integer type.");
    }

    // Specify the minimum and maximum values.
    let result = registry.execute("rand32", &[Value::Int(10), Value::Int(20)]);
    assert!(result.is_ok());
    if let Value::Int(val) = result.expect("rand32should succeed") {
        assert!((10..20).contains(&val));
    } else {
        panic!("Expect to return an integer type");
    }
}

#[test]
fn test_rand64_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("rand64", &[]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("rand64should succeed"),
        Value::BigInt(_)
    ));
}

#[test]
fn test_e_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("e", &[]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("eshould succeed"),
        Value::Float(std::f32::consts::E)
    );
}

#[test]
fn test_pi_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("pi", &[]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("pishould succeed"),
        Value::Float(std::f32::consts::PI)
    );
}

#[test]
fn test_exp2_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("exp2", &[Value::Int(3)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("exp2should succeed"), Value::Float(8.0));
}

#[test]
fn test_log2_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("log2", &[Value::Float(8.0)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("log2should succeed"), Value::Float(3.0));
}

#[test]
fn test_radians_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("radians", &[Value::Int(180)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("radiansshould succeed"),
        Value::Float(std::f32::consts::PI)
    );
}

// ==================== New string function test ====================

#[test]
fn test_lpad_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "lpad",
        &[Value::string("hello"), Value::Int(10), Value::string("*")],
    );
    assert!(result.is_ok());
    assert_eq!(
        result.expect("lpadshould succeed"),
        Value::string("*****hello")
    );
}

#[test]
fn test_rpad_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "rpad",
        &[Value::string("hello"), Value::Int(10), Value::string("*")],
    );
    assert!(result.is_ok());
    assert_eq!(
        result.expect("rpadshould succeed"),
        Value::string("hello*****")
    );
}

#[test]
fn test_concat_ws_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute(
        "concat_ws",
        &[
            Value::string(","),
            Value::string("a"),
            Value::string("b"),
            Value::string("c"),
        ],
    );
    assert!(result.is_ok());
    assert_eq!(
        result.expect("concat_wsshould succeed"),
        Value::string("a,b,c")
    );
}

#[test]
fn test_strcasecmp_function() {
    let registry = FunctionRegistry::new();

    // equivalent
    let result = registry.execute(
        "strcasecmp",
        &[Value::string("Hello"), Value::string("hello")],
    );
    assert!(result.is_ok());
    assert_eq!(result.expect("strcasecmpshould succeed"), Value::Int(0));

    // less than
    let result = registry.execute(
        "strcasecmp",
        &[Value::string("apple"), Value::string("banana")],
    );
    assert!(result.is_ok());
    assert_eq!(result.expect("strcasecmpshould succeed"), Value::Int(-1));

    // more than
    let result = registry.execute(
        "strcasecmp",
        &[Value::string("banana"), Value::string("apple")],
    );
    assert!(result.is_ok());
    assert_eq!(result.expect("strcasecmpshould succeed"), Value::Int(1));
}

// ==================== New container function test ====================

#[test]
fn test_toset_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(1), Value::Int(3)],
    }));

    let result = registry.execute("toset", &[list]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("tosetshould succeed"),
        Value::Set(_)
    ));
}

#[test]
fn test_reverse_list_function() {
    let registry = FunctionRegistry::new();
    let list = Value::List(Box::new(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    let result = registry.execute("reverse", &[list]);
    assert!(result.is_ok());

    if let Value::List(list) = result.expect("reverseshould succeed") {
        assert_eq!(list.values.len(), 3);
        assert_eq!(list.values[0], Value::Int(3));
        assert_eq!(list.values[1], Value::Int(2));
        assert_eq!(list.values[2], Value::Int(1));
    } else {
        panic!("Expected return list type");
    }
}

// ==================== New JSON function test ====================

#[test]
fn test_json_extract_function() {
    let registry = FunctionRegistry::new();
    let json = Value::string(r#"{"name": "Alice", "age": 30}"#);

    let result = registry.execute("json_extract", &[json, Value::string("name")]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("json_extractshould succeed"),
        Value::string("Alice")
    );
}

// ==================== New geospatial function test ====================

#[test]
fn test_st_point_function() {
    let registry = FunctionRegistry::new();

    let result = registry.execute("st_point", &[Value::Float(116.4074), Value::Float(39.9042)]);
    assert!(result.is_ok());
    assert!(matches!(
        result.expect("st_pointshould succeed"),
        Value::Geography(_)
    ));
}

#[test]
fn test_st_distance_function() {
    let registry = FunctionRegistry::new();
    use graphdb_core::value::geography::{Geography, GeographyValue};

    let beijing = Value::Geography(Geography::Point(GeographyValue {
        longitude: 116.4074,
        latitude: 39.9042,
    }));
    let shanghai = Value::Geography(Geography::Point(GeographyValue {
        longitude: 121.4737,
        latitude: 31.2304,
    }));

    let result = registry.execute("st_distance", &[beijing, shanghai]);
    assert!(result.is_ok());

    if let Value::Float(distance) = result.expect("st_distanceshould succeed") {
        assert!(distance > 1000.0 && distance < 1100.0);
    } else {
        panic!("Expect to return a floating point type");
    }
}

#[test]
fn test_st_isvalid_function() {
    let registry = FunctionRegistry::new();
    use graphdb_core::value::geography::{Geography, GeographyValue};

    let valid_point = Value::Geography(Geography::Point(GeographyValue {
        longitude: 116.4074,
        latitude: 39.9042,
    }));

    let result = registry.execute("st_isvalid", &[valid_point]);
    assert!(result.is_ok());
    assert_eq!(result.expect("st_isvalidshould succeed"), Value::Bool(true));
}

#[test]
fn test_st_dwithin_function() {
    let registry = FunctionRegistry::new();
    use graphdb_core::value::geography::{Geography, GeographyValue};

    let point1 = Value::Geography(Geography::Point(GeographyValue {
        longitude: 116.4074,
        latitude: 39.9042,
    }));
    let point2 = Value::Geography(Geography::Point(GeographyValue {
        longitude: 116.4075,
        latitude: 39.9043,
    }));

    let result = registry.execute("st_dwithin", &[point1, point2, Value::Float(1.0)]);
    assert!(result.is_ok());
    assert_eq!(result.expect("st_dwithinshould succeed"), Value::Bool(true));
}

#[test]
fn test_st_astext_function() {
    let registry = FunctionRegistry::new();
    use graphdb_core::value::geography::{Geography, GeographyValue};

    let point = Value::Geography(Geography::Point(GeographyValue {
        longitude: 116.4074,
        latitude: 39.9042,
    }));

    let result = registry.execute("st_astext", &[point]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("st_astextshould succeed"),
        Value::string("POINT(116.4074 39.9042)")
    );
}

// ==================== Function Existence Tests ====================

#[test]
fn test_all_functions_registered() {
    let registry = FunctionRegistry::new();

    // Graph Related Functions
    assert!(registry.contains("id"));
    assert!(registry.contains("tags"));
    assert!(registry.contains("labels"));
    assert!(registry.contains("properties"));
    assert!(registry.contains("type"));
    assert!(registry.contains("src"));
    assert!(registry.contains("dst"));
    assert!(registry.contains("rank"));
    assert!(registry.contains("startnode"));
    assert!(registry.contains("endnode"));

    // Container Manipulation Functions
    assert!(registry.contains("head"));
    assert!(registry.contains("last"));
    assert!(registry.contains("tail"));
    assert!(registry.contains("size"));
    assert!(registry.contains("range"));
    assert!(registry.contains("keys"));
    assert!(registry.contains("reverse"));

    // path function
    assert!(registry.contains("nodes"));
    assert!(registry.contains("relationships"));

    // math function
    assert!(registry.contains("bit_and"));
    assert!(registry.contains("bit_or"));
    assert!(registry.contains("bit_xor"));
    assert!(registry.contains("asin"));
    assert!(registry.contains("acos"));
    assert!(registry.contains("atan"));
    assert!(registry.contains("cbrt"));
    assert!(registry.contains("hypot"));
    assert!(registry.contains("sign"));
    assert!(registry.contains("rand"));
    assert!(registry.contains("rand32"));
    assert!(registry.contains("rand64"));
    assert!(registry.contains("e"));
    assert!(registry.contains("pi"));
    assert!(registry.contains("exp2"));
    assert!(registry.contains("log2"));
    assert!(registry.contains("radians"));

    // string function
    assert!(registry.contains("split"));
    assert!(registry.contains("lpad"));
    assert!(registry.contains("rpad"));
    assert!(registry.contains("concat_ws"));
    assert!(registry.contains("strcasecmp"));

    // utility function
    assert!(registry.contains("coalesce"));
    assert!(registry.contains("hash"));
    assert!(registry.contains("json_extract"));

    // datetime function
    assert!(registry.contains("time"));
    assert!(registry.contains("datetime"));
    assert!(registry.contains("timestamp"));

    // type conversion function
    assert!(registry.contains("toset"));

    // Geospatial functions
    assert!(registry.contains("st_point"));
    assert!(registry.contains("st_distance"));
    assert!(registry.contains("st_isvalid"));
    assert!(registry.contains("st_dwithin"));
    assert!(registry.contains("st_astext"));
}

// ==================== Pipeline Layer (SQL end to end) ====================

#[test]
fn test_sql_math_functions() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("funcs_sql_math")
        .query("RETURN abs(0 - 42) AS a, pi() AS p")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Int(42)]);
}

#[test]
fn test_sql_string_functions() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("funcs_sql_string")
        .query("RETURN split('a,b,c', ',') AS sp, length('hello') AS n")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Int(5)]);
}

#[test]
fn test_sql_container_functions() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("funcs_sql_container")
        .query("RETURN size([1, 2, 3]) AS n, head([1, 2, 3]) AS h")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Int(3), Value::Int(1)]);
}

#[test]
fn test_sql_coalesce_function() {
    TestScenario::new()
        .expect("Failed to create test scenario")
        .setup_space("funcs_sql_coalesce")
        .query("RETURN coalesce(null, 7) AS v")
        .assert_success()
        .assert_result_count(1)
        .assert_result_contains(vec![Value::Int(7)]);
}
