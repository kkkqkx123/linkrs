//! Phase 2: Core Type and Expression Integration Testing
//!
//! Test Range.
//! - core::value - value type conversions, comparisons, operations
//! - core::types - type system compatibility checking
//! - expression::evaluator - expression evaluation, context access
//! - expression::functions - Built-in function registration and calling
//! - expression::context - context chaining, cache management

mod common;

use graphdb_query::core::types::expr::Expression;
use graphdb_query::core::types::DataType;
use graphdb_query::core::value::{
    DateTimeValue, DateValue, GeographyValue, List, NullType, TimeValue, Value,
};
use graphdb_query::query::executor::expression::evaluation_context::DefaultExpressionContext;
use graphdb_query::query::executor::expression::functions::FunctionRegistry;
use graphdb_query::query::executor::expression::{ExpressionContext, ExpressionEvaluator};
use graphdb_query::query::DataSet;

// ==================== Value 类型测试 ====================

#[test]
fn test_value_null_type_variants() {
    // Test all NullType variants
    let null_variants = vec![
        NullType::Null,
        NullType::NaN,
        NullType::BadData,
        NullType::BadType,
        NullType::ErrOverflow,
        NullType::UnknownProp,
        NullType::DivByZero,
        NullType::OutOfRange,
    ];

    for variant in &null_variants {
        let value = Value::Null(variant.clone());
        assert!(value.is_null());
        assert_eq!(value.get_type(), DataType::Null);
    }

    // Testing the is_bad method
    assert!(NullType::BadData.is_bad());
    assert!(NullType::BadType.is_bad());
    assert!(NullType::ErrOverflow.is_bad());
    assert!(NullType::OutOfRange.is_bad());
    assert!(!NullType::Null.is_bad());
    assert!(!NullType::NaN.is_bad());

    // Testing the is_computational_error method
    assert!(NullType::NaN.is_computational_error());
    assert!(NullType::DivByZero.is_computational_error());
    assert!(NullType::ErrOverflow.is_computational_error());
}

#[test]
fn test_value_type_checking() {
    // Basic type checking
    assert_eq!(Value::Empty.get_type(), DataType::Empty);
    assert_eq!(Value::Null(NullType::Null).get_type(), DataType::Null);
    assert_eq!(Value::Bool(true).get_type(), DataType::Bool);
    assert_eq!(Value::Int(42).get_type(), DataType::Int);
    assert_eq!(Value::Float(1.5_f32).get_type(), DataType::Float);
    assert_eq!(
        Value::String("test".to_string()).get_type(),
        DataType::String
    );

    // Numeric type checking
    assert!(Value::Int(42).is_numeric());
    assert!(Value::Float(1.5_f32).is_numeric());
    assert!(!Value::String("42".to_string()).is_numeric());
    assert!(!Value::Bool(true).is_numeric());

    // BadNull Check
    assert!(Value::Null(NullType::BadData).is_bad_null());
    assert!(Value::Null(NullType::BadType).is_bad_null());
    assert!(!Value::Null(NullType::Null).is_bad_null());
}

#[test]
fn test_value_boolean_conversion() {
    // Boolean values are returned directly
    assert_eq!(Value::Bool(true).to_bool(), Value::Bool(true));
    assert_eq!(Value::Bool(false).to_bool(), Value::Bool(false));

    // string conversion
    assert_eq!(
        Value::String("true".to_string()).to_bool(),
        Value::Bool(true)
    );
    assert_eq!(
        Value::String("TRUE".to_string()).to_bool(),
        Value::Bool(true)
    );
    assert_eq!(
        Value::String("false".to_string()).to_bool(),
        Value::Bool(false)
    );
    assert_eq!(
        Value::String("FALSE".to_string()).to_bool(),
        Value::Bool(false)
    );
    assert_eq!(
        Value::String("invalid".to_string()).to_bool(),
        Value::Null(NullType::Null)
    );

    // Empty and Null
    assert_eq!(Value::Empty.to_bool(), Value::Null(NullType::Null));
    assert_eq!(
        Value::Null(NullType::Null).to_bool(),
        Value::Null(NullType::Null)
    );

    // Numeric types convert to boolean (non-zero = true, zero = false)
    assert_eq!(Value::Int(1).to_bool(), Value::Bool(true));
    assert_eq!(Value::Int(0).to_bool(), Value::Bool(false));
    assert_eq!(Value::SmallInt(1).to_bool(), Value::Bool(true));
    assert_eq!(Value::BigInt(1).to_bool(), Value::Bool(true));
    assert_eq!(Value::Float(1.0_f32).to_bool(), Value::Bool(true));
    assert_eq!(Value::Float(0.0_f32).to_bool(), Value::Bool(false));
    assert_eq!(Value::Double(1.0).to_bool(), Value::Bool(true));
    assert_eq!(Value::Double(0.0).to_bool(), Value::Bool(false));

    // Other types return BadData
    assert_eq!(
        Value::List(Box::new(List::new())).to_bool(),
        Value::Null(NullType::BadData)
    );
}

#[test]
fn test_value_integer_conversion() {
    // Integer direct return
    assert_eq!(Value::Int(42).to_int(), Value::Int(42));
    assert_eq!(Value::Int(-100).to_int(), Value::Int(-100));

    // Floating point conversion (truncation)
    assert_eq!(Value::Float(2.7_f32).to_int(), Value::Int(2));
    assert_eq!(Value::Float(-2.9).to_int(), Value::Int(-2));

    // Boundary value processing
    assert_eq!(Value::Float(f32::NAN).to_int(), Value::Null(NullType::Null));
    assert_eq!(
        Value::Float(f32::INFINITY).to_int(),
        Value::Null(NullType::Null)
    );
    assert_eq!(
        Value::Float(f32::NEG_INFINITY).to_int(),
        Value::Null(NullType::Null)
    );

    // string parsing
    assert_eq!(Value::String("42".to_string()).to_int(), Value::Int(42));
    assert_eq!(Value::String("-100".to_string()).to_int(), Value::Int(-100));
    assert_eq!(
        Value::String("invalid".to_string()).to_int(),
        Value::Null(NullType::Null)
    );

    // boolean conversion
    assert_eq!(Value::Bool(true).to_int(), Value::Int(1));
    assert_eq!(Value::Bool(false).to_int(), Value::Int(0));
}

#[test]
fn test_value_float_conversion() {
    // Floating point numbers are returned directly
    assert_eq!(Value::Float(1.5_f32).to_float(), Value::Float(1.5_f32));

    // integer conversion
    assert_eq!(Value::Int(42).to_float(), Value::Float(42.0));

    // string parsing
    assert_eq!(
        Value::String("1.5".to_string()).to_float(),
        Value::Float(1.5_f32)
    );
    assert_eq!(
        Value::String("-2.5".to_string()).to_float(),
        Value::Float(-2.5)
    );
    assert_eq!(
        Value::String("invalid".to_string()).to_float(),
        Value::Null(NullType::Null)
    );

    // boolean conversion
    assert_eq!(Value::Bool(true).to_float(), Value::Float(1.0));
    assert_eq!(Value::Bool(false).to_float(), Value::Float(0.0));
}

#[test]
fn test_value_arithmetic_operations() {
    // addition
    assert_eq!(
        Value::Int(10)
            .add(&Value::Int(5))
            .expect("整数加法应该成功"),
        Value::Int(15)
    );
    assert_eq!(
        Value::Float(3.5)
            .add(&Value::Float(2.5))
            .expect("浮点数加法应该成功"),
        Value::Float(6.0)
    );
    assert_eq!(
        Value::Int(10)
            .add(&Value::Float(2.5))
            .expect("整数与浮点数加法应该成功"),
        Value::Float(12.5)
    );
    assert_eq!(
        Value::String("Hello, ".to_string())
            .add(&Value::String("World".to_string()))
            .expect("字符串连接应该成功"),
        Value::String("Hello, World".to_string())
    );

    // subtractive
    assert_eq!(
        Value::Int(10)
            .sub(&Value::Int(3))
            .expect("整数减法应该成功"),
        Value::Int(7)
    );
    assert_eq!(
        Value::Float(10.5)
            .sub(&Value::Float(3.5))
            .expect("浮点数减法应该成功"),
        Value::Float(7.0)
    );

    // subtraction
    assert_eq!(
        Value::Int(6).mul(&Value::Int(7)).expect("整数乘法应该成功"),
        Value::Int(42)
    );
    assert_eq!(
        Value::Float(3.0)
            .mul(&Value::Float(4.0))
            .expect("浮点数乘法应该成功"),
        Value::Float(12.0)
    );

    // division (math.)
    assert_eq!(
        Value::Int(10)
            .div(&Value::Int(2))
            .expect("整数除法应该成功"),
        Value::Int(5)
    );
    assert_eq!(
        Value::Float(10.0)
            .div(&Value::Float(4.0))
            .expect("浮点数除法应该成功"),
        Value::Float(2.5)
    );

    // division error
    assert!(Value::Int(10).div(&Value::Int(0)).is_err());
    assert!(Value::Float(10.0).div(&Value::Float(0.0)).is_err());

    // take a mold
    assert_eq!(
        Value::Int(10)
            .rem(&Value::Int(3))
            .expect("整数取模应该成功"),
        Value::Int(1)
    );
    assert!(Value::Int(10).rem(&Value::Int(0)).is_err());
}

#[test]
fn test_value_comparison() {
    // integer comparison
    assert!(Value::Int(10) > Value::Int(5));
    assert!(Value::Int(5) < Value::Int(10));
    assert_eq!(Value::Int(10), Value::Int(10));

    // Floating Point Comparison (with NaN Handling)
    assert!(Value::Float(3.5_f32) > Value::Float(2.0));
    assert_eq!(Value::Float(f32::NAN), Value::Float(f32::NAN));

    // string comparison
    assert!(Value::String("b".to_string()) > Value::String("a".to_string()));
    assert_eq!(
        Value::String("test".to_string()),
        Value::String("test".to_string())
    );

    // Boolean comparison
    assert!(Value::Bool(true) > Value::Bool(false));

    // Comparison of different types (based on type priority)
    assert!(Value::Int(1) < Value::String("a".to_string()));
}

#[test]
fn test_value_unary_operations() {
    // retrieve the opposite of what one intended
    assert_eq!(
        Value::Int(42).neg().expect("整数取反应该成功"),
        Value::Int(-42)
    );
    assert_eq!(
        Value::Float(2.5_f32).neg().expect("浮点数取反应该成功"),
        Value::Float(-2.5_f32)
    );
    assert!(Value::String("test".to_string()).neg().is_err());

    // absolute value
    assert_eq!(
        Value::Int(-42).abs().expect("整数绝对值应该成功"),
        Value::Int(42)
    );
    assert_eq!(
        Value::Float(-2.5_f32).abs().expect("浮点数绝对值应该成功"),
        Value::Float(2.5_f32)
    );
    assert!(Value::String("test".to_string()).abs().is_err());

    // lengths
    assert_eq!(
        Value::String("hello".to_string())
            .len()
            .expect("字符串长度计算应该成功"),
        Value::Int(5)
    );
    assert_eq!(
        Value::List(Box::new(graphdb_query::core::List {
            values: vec![Value::Int(1), Value::Int(2)]
        }))
        .len()
        .expect("列表长度计算应该成功"),
        Value::Int(2)
    );
    assert_eq!(
        Value::Map(Box::<std::collections::HashMap<_, _>>::default())
            .len()
            .expect("映射长度计算应该成功"),
        Value::Int(0)
    );
}

#[test]
fn test_value_complex_types() {
    // DateValue
    let date = DateValue {
        year: 2024,
        month: 6,
        day: 15,
    };
    assert_eq!(date.year, 2024);
    assert_eq!(date.month, 6);
    assert_eq!(date.day, 15);

    // TimeValue
    let time = TimeValue {
        hour: 14,
        minute: 30,
        sec: 45,
        microsec: 0,
    };
    assert_eq!(time.hour, 14);
    assert_eq!(time.minute, 30);

    // DateTimeValue
    let datetime = DateTimeValue {
        year: 2024,
        month: 6,
        day: 15,
        hour: 14,
        minute: 30,
        sec: 0,
        microsec: 0,
    };
    assert_eq!(datetime.year, 2024);

    // GeographyValue
    let geo = GeographyValue {
        latitude: 39.9042,
        longitude: 116.4074,
    };
    assert_eq!(geo.latitude, 39.9042);
    assert_eq!(geo.longitude, 116.4074);

    // IntervalValue
    use graphdb_query::core::value::IntervalValue;
    let interval = IntervalValue::new(14, 3, 4_500_000_000);
    assert_eq!(interval.months, 14);
    assert_eq!(interval.days, 3);
    assert_eq!(interval.microseconds, 4_500_000_000);

    // Interval from years
    let iv_years = IntervalValue::from_years(2);
    assert_eq!(iv_years.months, 24);

    // Interval from days
    let iv_days = IntervalValue::from_days(5);
    assert_eq!(iv_days.days, 5);

    // Interval parsing - ISO 8601
    let iv_iso = IntervalValue::parse("P1Y2M3DT4H5M6S").unwrap();
    assert_eq!(iv_iso.months, 14);
    assert_eq!(iv_iso.days, 3);

    // Interval parsing - PostgreSQL format
    let iv_pg = IntervalValue::parse("1 year 2 months 3 days").unwrap();
    assert_eq!(iv_pg.months, 14);
    assert_eq!(iv_pg.days, 3);

    // Interval arithmetic
    let iv1 = IntervalValue::from_days(3);
    let iv2 = IntervalValue::from_hours(12);
    let iv_sum = iv1 + iv2;
    assert_eq!(iv_sum.days, 3);
    assert_eq!(iv_sum.microseconds, 12 * 3_600_000_000);

    // Interval negation
    let iv_neg = -IntervalValue::from_days(5);
    assert_eq!(iv_neg.days, -5);

    // Interval to ISO 8601 string
    let iv_str = IntervalValue::new(14, 3, 4 * 3_600_000_000 + 5 * 60_000_000 + 6 * 1_000_000);
    assert_eq!(iv_str.to_iso8601(), "P1Y2M3DT4H5M6S");

    // Interval to PostgreSQL string
    assert_eq!(iv_str.to_postgresql(), "1 year 2 mons 3 days 04:05:06");

    // DataSet
    let mut dataset = DataSet::new();
    dataset.col_names.push("name".to_string());
    dataset.col_names.push("age".to_string());
    dataset
        .rows
        .push(vec![Value::String("Alice".to_string()), Value::Int(30)]);
    assert_eq!(dataset.col_names.len(), 2);
    assert_eq!(dataset.rows.len(), 1);
}

#[test]
fn test_value_hash_and_equality() {
    use std::collections::HashSet;

    // Testing Hash Consistency
    let value1 = Value::Int(42);
    let value2 = Value::Int(42);
    assert_eq!(value1.hash_value(), value2.hash_value());

    // Testing floating point hashes (including special values)
    let nan1 = Value::Float(f32::NAN);
    let nan2 = Value::Float(f32::NAN);
    assert_eq!(nan1.hash_value(), nan2.hash_value());

    let pos_zero = Value::Float(0.0);
    let neg_zero = Value::Float(-0.0);
    assert_eq!(pos_zero.hash_value(), neg_zero.hash_value());

    // Testing for use in a HashSet
    let mut set = HashSet::new();
    set.insert(Value::Int(1));
    set.insert(Value::Int(2));
    set.insert(Value::Int(1)); // repeatable
    assert_eq!(set.len(), 2);
}

// ==================== DataType 测试 ====================

#[test]
fn test_datatype_variants() {
    // Test all DataType variants
    let types = vec![
        DataType::Empty,
        DataType::Null,
        DataType::Bool,
        DataType::SmallInt,
        DataType::Int,
        DataType::BigInt,
        DataType::Float,
        DataType::Double,
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
        DataType::FixedString(100),
        DataType::VID,
        DataType::Blob,
        DataType::Timestamp,
    ];

    // Ensure that all types can be cloned and compared
    for dtype in &types {
        let cloned = dtype.clone();
        assert_eq!(*dtype, cloned);
    }
}

// ==================== Expression test ====================

#[test]
fn test_expression_literal_creation() {
    // integer literal amount
    let expr = Expression::literal(42i64);
    match &expr {
        Expression::Literal(v) => assert_eq!(*v, Value::Int(42)),
        _ => panic!("Expected Literal Expression"),
    }

    // String literal
    let expr = Expression::literal("hello".to_string());
    match &expr {
        Expression::Literal(v) => assert_eq!(*v, Value::String("hello".to_string())),
        _ => panic!("Expected Literal Expression"),
    }

    // Boolean literals
    let expr = Expression::literal(true);
    match &expr {
        Expression::Literal(v) => assert_eq!(*v, Value::Bool(true)),
        _ => panic!("The “Expected Literal Expression” refers to a situation where the system or a user expects a specific, literal value to be provided as an input. This expectation is based on the context, the rules of the system, or the requirements of the task at hand. If the provided value does not match the expected literal value, it may lead to errors or issues in the system’s functionality."),
    }
}

#[test]
fn test_expression_variable_creation() {
    let expr = Expression::variable("x");
    match &expr {
        Expression::Variable(name) => assert_eq!(name, "x"),
        _ => panic!("Expected Variable Expression"),
    }
}

#[test]
fn test_expression_binary_creation() {
    use graphdb_query::core::types::BinaryOperator;

    let left = Expression::literal(10i64);
    let right = Expression::literal(5i64);
    let expr = Expression::Binary {
        left: Box::new(left),
        op: BinaryOperator::Add,
        right: Box::new(right),
    };

    match &expr {
        Expression::Binary { op, .. } => assert!(matches!(op, BinaryOperator::Add)),
        _ => panic!("Expected Binary expression"),
    }
}

#[test]
fn test_expression_unary_creation() {
    use graphdb_query::core::types::UnaryOperator;

    let operand = Expression::literal(true);
    let expr = Expression::Unary {
        op: UnaryOperator::Not,
        operand: Box::new(operand),
    };

    match &expr {
        Expression::Unary { op, .. } => assert!(matches!(op, UnaryOperator::Not)),
        _ => panic!("Expected a unary expression."),
    }
}

// ==================== ExpressionEvaluator 测试 ====================

#[test]
fn test_evaluator_literal() {
    let mut ctx = DefaultExpressionContext::new();

    // Integer
    let expr = Expression::literal(42i64);
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("整数字面量求值应该成功");
    assert_eq!(result, Value::Int(42));

    // String
    let expr = Expression::literal("test".to_string());
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("字符串字面量求值应该成功");
    assert_eq!(result, Value::String("test".to_string()));

    // Boolean value
    let expr = Expression::literal(true);
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("布尔字面量求值应该成功");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_evaluator_variable() {
    let mut ctx = DefaultExpressionContext::new();
    ctx.set_variable("x".to_string(), Value::Int(100));
    ctx.set_variable("name".to_string(), Value::String("Alice".to_string()));

    // Read the set variables
    let expr = Expression::variable("x");
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("变量求值应该成功");
    assert_eq!(result, Value::Int(100));

    let expr = Expression::variable("name");
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("变量求值应该成功");
    assert_eq!(result, Value::String("Alice".to_string()));
}

#[test]
fn test_evaluator_binary_arithmetic() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // Addition: 10 + 5
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(10i64)),
        op: BinaryOperator::Add,
        right: Box::new(Expression::literal(5i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元加法求值应该成功");
    assert_eq!(result, Value::Int(15));

    // Subtraction: 20 - 8
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(20i64)),
        op: BinaryOperator::Subtract,
        right: Box::new(Expression::literal(8i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元减法求值应该成功");
    assert_eq!(result, Value::Int(12));

    // Multiplication: 6 * 7
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(6i64)),
        op: BinaryOperator::Multiply,
        right: Box::new(Expression::literal(7i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元乘法求值应该成功");
    assert_eq!(result, Value::Int(42));

    // Division: 20 / 4
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(20i64)),
        op: BinaryOperator::Divide,
        right: Box::new(Expression::literal(4i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元除法求值应该成功");
    assert_eq!(result, Value::Int(5));
}

#[test]
fn test_evaluator_binary_comparison() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // Equivalent to: 5 == 5
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(5i64)),
        op: BinaryOperator::Equal,
        right: Box::new(Expression::literal(5i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元相等比较求值应该成功");
    assert_eq!(result, Value::Bool(true));

    // Not equal to: 5 != 3
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(5i64)),
        op: BinaryOperator::NotEqual,
        right: Box::new(Expression::literal(3i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元不等比较求值应该成功");
    assert_eq!(result, Value::Bool(true));

    // Greater than: 10 > 5
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(10i64)),
        op: BinaryOperator::GreaterThan,
        right: Box::new(Expression::literal(5i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元大于比较求值应该成功");
    assert_eq!(result, Value::Bool(true));

    // Less than: 3 < 7
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(3i64)),
        op: BinaryOperator::LessThan,
        right: Box::new(Expression::literal(7i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元小于比较求值应该成功");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_evaluator_binary_logical() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // AND: true && true
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(true)),
        op: BinaryOperator::And,
        right: Box::new(Expression::literal(true)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元AND逻辑求值应该成功");
    assert_eq!(result, Value::Bool(true));

    // AND: true && false
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(true)),
        op: BinaryOperator::And,
        right: Box::new(Expression::literal(false)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元AND逻辑求值应该成功");
    assert_eq!(result, Value::Bool(false));

    // OR: false || true
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(false)),
        op: BinaryOperator::Or,
        right: Box::new(Expression::literal(true)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("二元OR逻辑求值应该成功");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_evaluator_unary() {
    use graphdb_query::core::types::UnaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // NOT: !true
    let expr = Expression::Unary {
        op: UnaryOperator::Not,
        operand: Box::new(Expression::literal(true)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("一元NOT求值应该成功");
    assert_eq!(result, Value::Bool(false));

    // NOT: !false
    let expr = Expression::Unary {
        op: UnaryOperator::Not,
        operand: Box::new(Expression::literal(false)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("一元NOT求值应该成功");
    assert_eq!(result, Value::Bool(true));

    // Negative number: -42
    let expr = Expression::Unary {
        op: UnaryOperator::Minus,
        operand: Box::new(Expression::literal(42i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("一元负号求值应该成功");
    assert_eq!(result, Value::Int(-42));
}

#[test]
fn test_evaluator_nested_expression() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // (10 + 5) * 2 = 30
    let expr = Expression::Binary {
        left: Box::new(Expression::Binary {
            left: Box::new(Expression::literal(10i64)),
            op: BinaryOperator::Add,
            right: Box::new(Expression::literal(5i64)),
        }),
        op: BinaryOperator::Multiply,
        right: Box::new(Expression::literal(2i64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("嵌套表达式求值应该成功");
    assert_eq!(result, Value::Int(30));

    // 10 + (5 * 2) = 20
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(10i64)),
        op: BinaryOperator::Add,
        right: Box::new(Expression::Binary {
            left: Box::new(Expression::literal(5i64)),
            op: BinaryOperator::Multiply,
            right: Box::new(Expression::literal(2i64)),
        }),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("嵌套表达式求值应该成功");
    assert_eq!(result, Value::Int(20));
}

#[test]
fn test_evaluator_batch_evaluation() {
    let mut ctx = DefaultExpressionContext::new();

    let expressions = vec![
        Expression::literal(1i64),
        Expression::literal(2i64),
        Expression::literal(3i64),
    ];

    let results = ExpressionEvaluator::evaluate_batch(&expressions, &mut ctx)
        .expect("批量表达式求值应该成功");
    assert_eq!(results.len(), 3);
    assert_eq!(results[0], Value::Int(1));
    assert_eq!(results[1], Value::Int(2));
    assert_eq!(results[2], Value::Int(3));
}

#[test]
fn test_evaluator_can_evaluate() {
    // Pure constant expressions can be evaluated.
    let const_expr = Expression::Binary {
        left: Box::new(Expression::literal(10i64)),
        op: graphdb_query::core::types::BinaryOperator::Add,
        right: Box::new(Expression::literal(5i64)),
    };
    assert!(ExpressionEvaluator::can_evaluate(&const_expr));

    // Expressions that contain variables require context.
    let var_expr = Expression::variable("x");
    assert!(!ExpressionEvaluator::can_evaluate(&var_expr));
}

// ==================== Function Registry 测试 ====================

#[test]
fn test_function_registry_builtins() {
    let registry = FunctionRegistry::new();

    // Testing mathematical functions
    let result = registry
        .execute("abs", &[Value::Int(-42)])
        .expect("abs函数执行应该成功");
    assert_eq!(result, Value::Int(42));

    let result = registry
        .execute("abs", &[Value::Float(-2.5_f32)])
        .expect("abs函数执行应该成功");
    assert_eq!(result, Value::Float(2.5_f32));

    // Testing the string function
    let result = registry
        .execute("length", &[Value::String("hello".to_string())])
        .expect("length函数执行应该成功");
    assert_eq!(result, Value::Int(5));

    // Test type conversion function
    let result = registry
        .execute("to_int", &[Value::String("42".to_string())])
        .expect("to_int函数执行应该成功");
    assert_eq!(result, Value::Int(42));
}

#[test]
fn test_function_registry_errors() {
    let registry = FunctionRegistry::new();

    // Undefined function
    let result = registry.execute("undefined_func", &[Value::Int(1)]);
    assert!(result.is_err());

    // The number of parameters does not match.
    let result = registry.execute("abs", &[]);
    assert!(result.is_err());
}

// ==================== ExpressionContext 测试 ====================

#[test]
fn test_basic_context_variables() {
    let mut ctx = DefaultExpressionContext::new();

    // Setting variables
    ctx.set_variable("x".to_string(), Value::Int(100));
    ctx.set_variable("y".to_string(), Value::String("test".to_string()));

    // Obtain the variable
    assert_eq!(ctx.get_variable("x"), Some(Value::Int(100)));
    assert_eq!(
        ctx.get_variable("y"),
        Some(Value::String("test".to_string()))
    );
    assert_eq!(ctx.get_variable("z"), None);
}

#[test]
fn test_basic_context_functions() {
    let _ctx = DefaultExpressionContext::new();

    // Testing the existence and execution of functions via the global registry
    let registry = FunctionRegistry::new();

    // Testing the abs function
    let result = registry.execute("abs", &[Value::Int(-42)]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("Failed to execute abs function"),
        Value::Int(42)
    );

    // Testing the length function
    let result = registry.execute("length", &[Value::String("hello".to_string())]);
    assert!(result.is_ok());
    assert_eq!(
        result.expect("Failed to execute length function"),
        Value::Int(5)
    );

    // An undefined function should return an error.
    let result = registry.execute("undefined_func", &[Value::Int(1)]);
    assert!(result.is_err());
}

#[test]
fn test_basic_context_cache() {
    let mut ctx = DefaultExpressionContext::new();

    // Setting and retrieving cache values (simulated using variables)
    ctx.set_variable("cached_key1".to_string(), Value::Int(42));
    assert_eq!(ctx.get_variable("cached_key1"), Some(Value::Int(42)));
    assert_eq!(ctx.get_variable("nonexistent"), None);
}

#[test]
fn test_context_parent_child() {
    let mut parent = DefaultExpressionContext::new();
    parent.set_variable("parent_var".to_string(), Value::Int(100));

    let mut child = DefaultExpressionContext::new();
    child.set_variable("child_var".to_string(), Value::Int(200));

    // The child context should be able to access its own variables.
    assert_eq!(child.get_variable("child_var"), Some(Value::Int(200)));
    // The parent context should be able to access its own variables.
    assert_eq!(parent.get_variable("parent_var"), Some(Value::Int(100)));
}

// ==================== Complex Scenario Testing ====================

#[test]
fn test_complex_arithmetic_expression() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // Complex expression: (100 - 50) * 2 + 10 / 5 = 102
    let expr = Expression::Binary {
        left: Box::new(Expression::Binary {
            left: Box::new(Expression::Binary {
                left: Box::new(Expression::literal(100i64)),
                op: BinaryOperator::Subtract,
                right: Box::new(Expression::literal(50i64)),
            }),
            op: BinaryOperator::Multiply,
            right: Box::new(Expression::literal(2i64)),
        }),
        op: BinaryOperator::Add,
        right: Box::new(Expression::Binary {
            left: Box::new(Expression::literal(10i64)),
            op: BinaryOperator::Divide,
            right: Box::new(Expression::literal(5i64)),
        }),
    };

    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("复杂表达式求值应该成功");
    assert_eq!(result, Value::Int(102));
}

#[test]
fn test_mixed_type_operations() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // Mixed operations of integers and floating-point numbers
    let expr = Expression::Binary {
        left: Box::new(Expression::literal(10i64)),
        op: BinaryOperator::Add,
        right: Box::new(Expression::literal(5.5f64)),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("混合类型操作求值应该成功");
    assert_eq!(result, Value::Float(15.5));
}

#[test]
fn test_string_concatenation() {
    use graphdb_query::core::types::BinaryOperator;
    let mut ctx = DefaultExpressionContext::new();

    // String concatenation
    let expr = Expression::Binary {
        left: Box::new(Expression::literal("Hello, ".to_string())),
        op: BinaryOperator::Add,
        right: Box::new(Expression::literal("World!".to_string())),
    };
    let result = ExpressionEvaluator::evaluate(&expr, &mut ctx).expect("字符串连接求值应该成功");
    assert_eq!(result, Value::String("Hello, World!".to_string()));
}

#[test]
fn test_list_operations() {
    // Create list values
    let list = Value::List(Box::new(graphdb_query::core::List {
        values: vec![Value::Int(1), Value::Int(2), Value::Int(3)],
    }));

    assert_eq!(list.len().expect("列表长度计算应该成功"), Value::Int(3));
    assert_eq!(list.get_type(), DataType::List);

    // Empty list
    let empty_list = Value::List(Box::new(graphdb_query::core::List { values: vec![] }));
    assert_eq!(
        empty_list.len().expect("空列表长度计算应该成功"),
        Value::Int(0)
    );
}

#[test]
fn test_map_operations() {
    use std::collections::HashMap;

    // Create a Map value
    let mut map = HashMap::new();
    map.insert("name".to_string(), Value::String("Alice".to_string()));
    map.insert("age".to_string(), Value::Int(30));
    let map_value = Value::Map(Box::new(map));

    assert_eq!(
        map_value.len().expect("映射长度计算应该成功"),
        Value::Int(2)
    );
    assert_eq!(map_value.get_type(), DataType::Map);
}

#[test]
fn test_value_memory_estimation() {
    // Testing the memory estimation function
    let int_val = Value::Int(42);
    assert!(int_val.estimated_size() > 0);

    let string_val = Value::String("hello world".to_string());
    assert!(string_val.estimated_size() >= std::mem::size_of::<Value>() + "hello world".len());

    let list_val = Value::List(Box::new(graphdb_query::core::List {
        values: vec![Value::Int(1), Value::Int(2)],
    }));
    assert!(list_val.estimated_size() > int_val.estimated_size());
}

#[test]
fn test_null_type_display() {
    assert_eq!(NullType::Null.to_string(), "NULL");
    assert_eq!(NullType::NaN.to_string(), "NaN");
    assert_eq!(NullType::BadData.to_string(), "BAD_DATA");
    assert_eq!(NullType::BadType.to_string(), "BAD_TYPE");
    assert_eq!(NullType::ErrOverflow.to_string(), "ERR_OVERFLOW");
    assert_eq!(NullType::UnknownProp.to_string(), "UNKNOWN_PROP");
    assert_eq!(NullType::DivByZero.to_string(), "DIV_BY_ZERO");
    assert_eq!(NullType::OutOfRange.to_string(), "OUT_OF_RANGE");
}

#[test]
fn test_default_values() {
    // Test the default values.
    let default_null: NullType = Default::default();
    assert_eq!(default_null, NullType::Null);

    let default_date: DateValue = Default::default();
    assert_eq!(default_date.year, 1970);
    assert_eq!(default_date.month, 1);
    assert_eq!(default_date.day, 1);

    let default_time: TimeValue = Default::default();
    assert_eq!(default_time.hour, 0);
    assert_eq!(default_time.minute, 0);

    let default_geo: GeographyValue = Default::default();
    assert_eq!(default_geo.latitude, 0.0);
    assert_eq!(default_geo.longitude, 0.0);
}
