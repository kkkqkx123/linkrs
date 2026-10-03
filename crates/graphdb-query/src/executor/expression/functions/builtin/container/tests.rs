use super::*;

#[test]
fn test_head_function() {
    let list = Value::list(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    });
    let result = ContainerFunction::Head
        .execute(&[list])
        .expect("head function should succeed");
    assert_eq!(result, Value::Int(1));
}

#[test]
fn test_head_empty_list() {
    let list = Value::list(List { values: vec![] });
    let result = ContainerFunction::Head
        .execute(&[list])
        .expect("head function should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_last_function() {
    let list = Value::list(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    });
    let result = ContainerFunction::Last
        .execute(&[list])
        .expect("last function should succeed");
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_tail_function() {
    let list = Value::list(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    });
    let result = ContainerFunction::Tail
        .execute(&[list])
        .expect("tail function should succeed");
    assert_eq!(
        result,
        Value::list(List {
            values: vec![Value::Int(2), Value::Int(3)]
        })
    );
}

#[test]
fn test_size_string() {
    let result = ContainerFunction::Size
        .execute(&[Value::string("hello")])
        .expect("size function should succeed");
    assert_eq!(result, Value::Int(5));
}

#[test]
fn test_size_list() {
    let list = Value::list(List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    });
    let result = ContainerFunction::Size
        .execute(&[list])
        .expect("size function should succeed");
    assert_eq!(result, Value::Int(3));
}

#[test]
fn test_range_basic() {
    let result = ContainerFunction::Range
        .execute(&[Value::Int(1), Value::Int(5)])
        .expect("range function should succeed");
    assert_eq!(
        result,
        Value::list(List {
            values: vec![
                Value::Int(1),
                Value::Int(2),
                Value::Int(3),
                Value::Int(4),
                Value::Int(5)
            ]
        })
    );
}

#[test]
fn test_range_with_step() {
    let result = ContainerFunction::Range
        .execute(&[Value::Int(0), Value::Int(10), Value::Int(2)])
        .expect("range function should succeed");
    assert_eq!(
        result,
        Value::list(List {
            values: vec![
                Value::Int(0),
                Value::Int(2),
                Value::Int(4),
                Value::Int(6),
                Value::Int(8),
                Value::Int(10)
            ]
        })
    );
}

#[test]
fn test_null_handling() {
    let null_value = Value::Null(NullType::Null);

    assert_eq!(
        ContainerFunction::Head
            .execute(std::slice::from_ref(&null_value))
            .expect("head should handle NULL"),
        Value::Null(NullType::Null)
    );
    assert_eq!(
        ContainerFunction::Last
            .execute(std::slice::from_ref(&null_value))
            .expect("last should handle NULL"),
        Value::Null(NullType::Null)
    );
    assert_eq!(
        ContainerFunction::Tail
            .execute(std::slice::from_ref(&null_value))
            .expect("tail should handle NULL"),
        Value::Null(NullType::Null)
    );
    assert_eq!(
        ContainerFunction::Size
            .execute(std::slice::from_ref(&null_value))
            .expect("size should handle NULL"),
        Value::Null(NullType::Null)
    );
}

#[test]
fn test_list_distinct() {
    let list = Value::list(List {
        values: vec![
            Value::Int(1),
            Value::Int(2),
            Value::Int(1),
            Value::Int(3),
            Value::Int(2),
        ],
    });
    let result = ContainerFunction::ListDistinct
        .execute(&[list])
        .expect("list_distinct should succeed");
    assert_eq!(
        result,
        Value::list(List {
            values: vec![Value::Int(1), Value::Int(2), Value::Int(3)]
        })
    );
}

#[test]
fn test_list_extract() {
    let list = Value::list(List {
        values: vec![Value::Int(10), Value::Int(20), Value::Int(30)],
    });
    assert_eq!(
        ContainerFunction::ListExtract
            .execute(&[list.clone(), Value::Int(0)])
            .unwrap(),
        Value::Int(10)
    );
    assert_eq!(
        ContainerFunction::ListExtract
            .execute(&[list.clone(), Value::Int(1)])
            .unwrap(),
        Value::Int(20)
    );
    assert_eq!(
        ContainerFunction::ListExtract
            .execute(&[list.clone(), Value::Int(-1)])
            .unwrap(),
        Value::Int(30)
    );
}

#[test]
fn test_list_extract_out_of_bounds() {
    let list = Value::list(List {
        values: vec![Value::Int(1)],
    });
    let result = ContainerFunction::ListExtract
        .execute(&[list, Value::Int(5)])
        .expect("list_extract should handle out of bounds");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_struct_pack() {
    let result = ContainerFunction::StructPack
        .execute(&[
            Value::string("name"),
            Value::string("Alice"),
            Value::string("age"),
            Value::Int(30),
        ])
        .expect("struct_pack should succeed");
    assert!(matches!(result, Value::Map(_)));
}

#[test]
fn test_struct_extract() {
    let mut map = std::collections::HashMap::new();
    map.insert("name".to_string(), Value::string("Alice"));
    map.insert("age".to_string(), Value::Int(30));
    let result = ContainerFunction::StructExtract
        .execute(&[Value::string_map(map), Value::string("name")])
        .expect("struct_extract should succeed");
    assert_eq!(result, Value::string("Alice"));
}

#[test]
fn test_map_creation() {
    let result = ContainerFunction::MapCreation
        .execute(&[
            Value::string("x"),
            Value::Int(1),
            Value::string("y"),
            Value::Int(2),
        ])
        .expect("map should succeed");
    assert!(matches!(result, Value::Map(_)));
}

#[test]
fn test_element_at_list() {
    let list = Value::list(List {
        values: vec![Value::Int(10), Value::Int(20), Value::Int(30)],
    });
    let result = ContainerFunction::ElementAt
        .execute(&[list, Value::Int(1)])
        .expect("element_at should succeed");
    assert_eq!(result, Value::Int(20));
}

#[test]
fn test_element_at_map() {
    let mut map = std::collections::HashMap::new();
    map.insert("key".to_string(), Value::string("value"));
    let result = ContainerFunction::ElementAt
        .execute(&[Value::string_map(map), Value::string("key")])
        .expect("element_at should succeed");
    assert_eq!(result, Value::string("value"));
}

#[test]
fn test_cardinality() {
    let map = std::collections::HashMap::new();
    let result = ContainerFunction::Cardinality
        .execute(&[Value::string_map(map)])
        .expect("cardinality should succeed");
    assert_eq!(result, Value::Int(0));
}

#[test]
fn test_map_keys() {
    let mut map = std::collections::HashMap::new();
    map.insert("a".to_string(), Value::Int(1));
    map.insert("b".to_string(), Value::Int(2));
    let result = ContainerFunction::MapKeys
        .execute(&[Value::string_map(map)])
        .expect("map_keys should succeed");
    assert!(matches!(result, Value::List(_)));
}

#[test]
fn test_map_values() {
    let mut map = std::collections::HashMap::new();
    map.insert("a".to_string(), Value::Int(1));
    map.insert("b".to_string(), Value::Int(2));
    let result = ContainerFunction::MapValues
        .execute(&[Value::string_map(map)])
        .expect("map_values should succeed");
    assert!(matches!(result, Value::List(_)));
}

#[test]
fn test_size_non_ascii_counts_chars() {
    let result = ContainerFunction::Size
        .execute(&[Value::string("你好世界")])
        .expect("size function should succeed");
    assert_eq!(result, Value::BigInt(4));
}

#[test]
fn test_size_arity() {
    use crate::executor::expression::ExpressionErrorType;
    let err = ContainerFunction::Size.execute(&[]).unwrap_err();
    assert_eq!(err.error_type, ExpressionErrorType::InvalidArgumentCount);
    assert!(err.message.contains("size"));
}
