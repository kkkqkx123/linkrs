use super::*;

#[test]
fn test_coalesce() {
    let func = UtilityFunction::Coalesce;
    let result = func
        .execute(&[Value::Null(NullType::Null), Value::Int(42), Value::Int(100)])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_hash() {
    let func = UtilityFunction::Hash;
    let result = func
        .execute(&[Value::string("test")])
        .expect("Execution should succeed");
    assert!(matches!(result, Value::BigInt(_)));
}

#[test]
fn test_ifnull() {
    assert_eq!(
        UtilityFunction::IfNull
            .execute(&[Value::Null(NullType::Null), Value::Int(42)])
            .unwrap(),
        Value::Int(42)
    );
    assert_eq!(
        UtilityFunction::IfNull
            .execute(&[Value::Int(10), Value::Int(42)])
            .unwrap(),
        Value::Int(10)
    );
}

#[test]
fn test_typeof() {
    assert_eq!(
        UtilityFunction::TypeOf.execute(&[Value::Int(1)]).unwrap(),
        Value::string("int")
    );
    assert_eq!(
        UtilityFunction::TypeOf
            .execute(&[Value::string("hello")])
            .unwrap(),
        Value::string("string")
    );
    assert_eq!(
        UtilityFunction::TypeOf
            .execute(&[Value::Bool(true)])
            .unwrap(),
        Value::string("bool")
    );
    assert_eq!(
        UtilityFunction::TypeOf
            .execute(&[Value::Null(NullType::Null)])
            .unwrap(),
        Value::string("null")
    );
}

#[test]
fn test_version() {
    let result = UtilityFunction::Version.execute(&[]).unwrap();
    assert!(matches!(result, Value::String(_)));
}

#[test]
fn test_current_user() {
    let result = UtilityFunction::CurrentUser.execute(&[]).unwrap();
    assert!(matches!(result, Value::String(_)));
}

#[test]
fn test_current_database() {
    let result = UtilityFunction::CurrentDatabase.execute(&[]).unwrap();
    assert!(matches!(result, Value::String(_)));
}

#[test]
fn test_corr() {
    let xs = Value::list(List {
        values: vec![
            Value::Double(1.0),
            Value::Double(2.0),
            Value::Double(3.0),
            Value::Double(4.0),
            Value::Double(5.0),
        ],
    });
    let ys = Value::list(List {
        values: vec![
            Value::Double(2.0),
            Value::Double(4.0),
            Value::Double(6.0),
            Value::Double(8.0),
            Value::Double(10.0),
        ],
    });
    let result = UtilityFunction::Corr
        .execute(&[xs, ys])
        .expect("corr should succeed");
    assert!(matches!(result, Value::Double(v) if (v - 1.0).abs() < 1e-10));
}

#[test]
fn test_covar_pop() {
    let xs = Value::list(List {
        values: vec![Value::Double(1.0), Value::Double(2.0)],
    });
    let ys = Value::list(List {
        values: vec![Value::Double(3.0), Value::Double(4.0)],
    });
    let result = UtilityFunction::CovarPop
        .execute(&[xs, ys])
        .expect("covar_pop should succeed");
    assert!(matches!(result, Value::Double(_)));
}

#[test]
fn test_octet_length() {
    let result = UtilityFunction::OctetLength
        .execute(&[Value::string("hello")])
        .expect("octet_length should succeed");
    assert_eq!(result, Value::Int(5));
}

#[test]
fn test_encode() {
    let result = UtilityFunction::Encode
        .execute(&[Value::string("hello")])
        .expect("encode should succeed");
    assert!(matches!(result, Value::Blob(_)));
}

#[test]
fn test_decode() {
    let blob = Value::Blob(b"hello".to_vec());
    let result = UtilityFunction::Decode
        .execute(&[blob])
        .expect("decode should succeed");
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_union_value() {
    let result = UtilityFunction::UnionValue
        .execute(&[Value::Int(0), Value::Int(42)])
        .expect("union_value should succeed");
    match &result {
        Value::Map(m) => {
            let tag = m.get(&Value::string("__tag")).expect("missing __tag");
            assert_eq!(tag, &Value::Int(0));
            let val = m.get(&Value::string("__value")).expect("missing __value");
            assert_eq!(val, &Value::Int(42));
        }
        _ => panic!("expected map"),
    }
}

#[test]
fn test_union_value_null_propagation() {
    let result = UtilityFunction::UnionValue
        .execute(&[Value::Null(NullType::Null), Value::Int(42)])
        .expect("union_value should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_union_value_negative_tag() {
    let result = UtilityFunction::UnionValue.execute(&[Value::Int(-1), Value::Int(42)]);
    assert!(result.is_err());
}

#[test]
fn test_union_tag() {
    let mut map = HashMap::new();
    map.insert(Value::string("__tag"), Value::Int(2));
    map.insert(Value::string("__value"), Value::string("hello"));
    let union_val = Value::map(map);
    let result = UtilityFunction::UnionTag
        .execute(&[union_val])
        .expect("union_tag should succeed");
    assert_eq!(result, Value::Int(2));
}

#[test]
fn test_union_tag_missing_key() {
    let map = HashMap::new();
    let union_val = Value::map(map);
    let result = UtilityFunction::UnionTag.execute(&[union_val]);
    assert!(result.is_err());
}

#[test]
fn test_union_tag_null() {
    let result = UtilityFunction::UnionTag
        .execute(&[Value::Null(NullType::Null)])
        .expect("union_tag should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_union_extract() {
    let mut map = HashMap::new();
    map.insert(Value::string("__tag"), Value::Int(0));
    map.insert(Value::string("__value"), Value::string("hello"));
    let union_val = Value::map(map);
    let result = UtilityFunction::UnionExtract
        .execute(&[union_val])
        .expect("union_extract should succeed");
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_union_extract_missing_key() {
    let map = HashMap::new();
    let union_val = Value::map(map);
    let result = UtilityFunction::UnionExtract.execute(&[union_val]);
    assert!(result.is_err());
}

#[test]
fn test_union_extract_null() {
    let result = UtilityFunction::UnionExtract
        .execute(&[Value::Null(NullType::Null)])
        .expect("union_extract should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_union_roundtrip() {
    let u = UtilityFunction::UnionValue
        .execute(&[Value::Int(1), Value::Double(3.5)])
        .expect("create union");
    let tag = UtilityFunction::UnionTag
        .execute(std::slice::from_ref(&u))
        .expect("get tag");
    assert_eq!(tag, Value::Int(1));
    let val = UtilityFunction::UnionExtract
        .execute(&[u])
        .expect("extract value");
    assert_eq!(val, Value::Double(3.5));
}

#[test]
fn test_octet_length_counts_bytes() {
    let result = UtilityFunction::OctetLength
        .execute(&[Value::string("你好")])
        .expect("octet_length should succeed");
    assert_eq!(result, Value::BigInt(6));
    let result = UtilityFunction::OctetLength
        .execute(&[Value::string("hello")])
        .expect("octet_length should succeed");
    assert_eq!(result, Value::BigInt(5));
}

#[test]
fn test_length_size_octet_length_contrast() {
    use crate::executor::expression::functions::{ContainerFunction, PathFunction};
    let s = Value::string("hello你好");
    let length = PathFunction::PathLength
        .execute(std::slice::from_ref(&s))
        .expect("length should succeed");
    let size = ContainerFunction::Size
        .execute(std::slice::from_ref(&s))
        .expect("size should succeed");
    let octets = UtilityFunction::OctetLength
        .execute(std::slice::from_ref(&s))
        .expect("octet_length should succeed");
    assert_eq!(length, Value::BigInt(7));
    assert_eq!(size, Value::BigInt(7));
    assert_eq!(octets, Value::BigInt(11));
}

#[test]
fn test_utility_arity() {
    use crate::executor::expression::ExpressionErrorType;
    let err = UtilityFunction::OctetLength.execute(&[]).unwrap_err();
    assert_eq!(err.error_type, ExpressionErrorType::InvalidArgumentCount);
    assert!(err.message.contains("octet_length"));
}
