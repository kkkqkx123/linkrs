use super::*;

use crate::executor::expression::functions::FunctionRegistry;

#[test]
fn test_length() {
    let registry = FunctionRegistry::new();
    let result = registry
        .execute("length", &[Value::string("hello")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Int(5));
}

#[test]
fn test_upper() {
    let func = StringFunction::Upper;
    let result = func
        .execute(&[Value::string("hello")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("HELLO"));
}

#[test]
fn test_lower() {
    let func = StringFunction::Lower;
    let result = func
        .execute(&[Value::string("HELLO")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_trim() {
    let func = StringFunction::Trim;
    let result = func
        .execute(&[Value::string("  hello  ")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("hello"));
}

#[test]
fn test_substring() {
    let func = StringFunction::Substring;
    let result = func
        .execute(&[Value::string("hello"), Value::Int(1), Value::Int(3)])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("ell"));
}

#[test]
fn test_concat() {
    let func = StringFunction::Concat;
    let result = func
        .execute(&[
            Value::string("hello"),
            Value::string(" "),
            Value::string("world"),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("hello world"));
}

#[test]
fn test_contains() {
    let func = StringFunction::Contains;
    let result = func
        .execute(&[Value::string("hello world"), Value::string("world")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_starts_with() {
    let func = StringFunction::StartsWith;
    let result = func
        .execute(&[Value::string("hello world"), Value::string("hello")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_ends_with() {
    let func = StringFunction::EndsWith;
    let result = func
        .execute(&[Value::string("hello world"), Value::string("world")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_null_handling() {
    let registry = FunctionRegistry::new();
    let result = registry
        .execute("length", &[Value::Null(NullType::Null)])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_string_insert() {
    let func = StringFunction::StringInsert;
    let result = func
        .execute(&[
            Value::string("Hello World"),
            Value::Int(5),
            Value::Int(0),
            Value::string(","),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("Hello, World"));
}

#[test]
fn test_translate() {
    let func = StringFunction::Translate;
    let result = func
        .execute(&[
            Value::string("hello"),
            Value::string("ae"),
            Value::string("xy"),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("hyllo"));
}

#[test]
fn test_format() {
    let func = StringFunction::Format;
    let result = func
        .execute(&[
            Value::string("Hello {0}, your score is {1}"),
            Value::string("Alice"),
            Value::Int(95),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("Hello Alice, your score is 95"));
}

#[test]
fn test_string_split() {
    let func = StringFunction::StringSplit;
    let result = func
        .execute(&[Value::string("a,b,c"), Value::string(",")])
        .expect("Execution should succeed");
    assert_eq!(
        result,
        Value::list(List {
            values: vec![Value::string("a"), Value::string("b"), Value::string("c"),]
        })
    );
}

#[test]
fn test_reverse() {
    let func = StringFunction::Reverse;
    let result = func
        .execute(&[Value::string("hello")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("olleh"));
}

#[test]
fn test_substring_non_ascii() {
    let func = StringFunction::Substring;
    let result = func
        .execute(&[Value::string("你好世界"), Value::Int(1), Value::Int(2)])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("好世"));
}

#[test]
fn test_substring_non_ascii_single_char() {
    // Byte slicing would cut 1/3 of a Chinese character and panic;
    // character semantics return the whole character.
    let func = StringFunction::Substring;
    let result = func
        .execute(&[Value::string("你好"), Value::Int(0), Value::Int(1)])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("你"));
}

#[test]
fn test_substring_boundaries() {
    let func = StringFunction::Substring;
    let empty = Value::string(String::new());
    assert_eq!(
        func.execute(&[Value::string("你好"), Value::Int(5), Value::Int(2)])
            .unwrap(),
        empty
    );
    assert_eq!(
        func.execute(&[Value::string("你好"), Value::Int(-1), Value::Int(2)])
            .unwrap(),
        empty
    );
    assert_eq!(
        func.execute(&[Value::string("你好"), Value::Int(0), Value::Int(0)])
            .unwrap(),
        empty
    );
    assert_eq!(
        func.execute(&[Value::string(""), Value::Int(0), Value::Int(3)])
            .unwrap(),
        empty
    );
    // Length beyond the end is clamped, not an error.
    assert_eq!(
        func.execute(&[Value::string("你好"), Value::Int(1), Value::Int(10)])
            .unwrap(),
        Value::string("好")
    );
}

#[test]
fn test_lpad_rpad_non_ascii() {
    assert_eq!(
        StringFunction::Lpad
            .execute(&[Value::string("你好"), Value::Int(4), Value::string("ab")])
            .unwrap(),
        Value::string("ab你好")
    );
    assert_eq!(
        StringFunction::Rpad
            .execute(&[Value::string("你好"), Value::Int(4), Value::string("ab")])
            .unwrap(),
        Value::string("你好ab")
    );
    // Truncation keeps whole characters.
    assert_eq!(
        StringFunction::Lpad
            .execute(&[Value::string("你好世界"), Value::Int(2), Value::string("x")])
            .unwrap(),
        Value::string("你好")
    );
    assert_eq!(
        StringFunction::Rpad
            .execute(&[Value::string("你好世界"), Value::Int(2), Value::string("x")])
            .unwrap(),
        Value::string("你好")
    );
    // Empty padding cannot extend the string; return it unchanged
    // instead of looping forever.
    assert_eq!(
        StringFunction::Lpad
            .execute(&[Value::string("hi"), Value::Int(5), Value::string("")])
            .unwrap(),
        Value::string("hi")
    );
    // Negative target length is rejected.
    assert!(StringFunction::Lpad
        .execute(&[Value::string("hi"), Value::Int(-1), Value::string("x")])
        .is_err());
}

#[test]
fn test_left_right_non_ascii() {
    assert_eq!(
        StringFunction::Left
            .execute(&[Value::string("你好世界"), Value::Int(2)])
            .unwrap(),
        Value::string("你好")
    );
    assert_eq!(
        StringFunction::Right
            .execute(&[Value::string("你好世界"), Value::Int(2)])
            .unwrap(),
        Value::string("世界")
    );
}

#[test]
fn test_position_non_ascii_returns_char_position() {
    let result = StringFunction::Position
        .execute(&[Value::string("你好世界"), Value::string("世界")])
        .expect("Execution should succeed");
    assert_eq!(result, Value::Int(3));
    let missing = StringFunction::Position
        .execute(&[Value::string("你好"), Value::string("世")])
        .expect("Execution should succeed");
    assert_eq!(missing, Value::Int(0));
}

#[test]
fn test_string_insert_non_ascii() {
    let result = StringFunction::StringInsert
        .execute(&[
            Value::string("你好世界"),
            Value::Int(2),
            Value::Int(1),
            Value::string("X"),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("你好X界"));
    // Position beyond the end leaves the string unchanged.
    let result = StringFunction::StringInsert
        .execute(&[
            Value::string("你好"),
            Value::Int(5),
            Value::Int(1),
            Value::string("X"),
        ])
        .expect("Execution should succeed");
    assert_eq!(result, Value::string("你好"));
}

#[test]
fn test_arity_error_type() {
    use crate::executor::expression::ExpressionErrorType;
    let err = StringFunction::Substring
        .execute(&[Value::string("hi"), Value::Int(0)])
        .unwrap_err();
    assert_eq!(err.error_type, ExpressionErrorType::InvalidArgumentCount);
    assert!(err.message.contains("substring"));
}
