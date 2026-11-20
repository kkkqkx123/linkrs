//! Cross-crate proof that storage pre-filters never disagree with the
//! expression evaluator. The original filter still runs after the scan, so a
//! pushed predicate must accept every row the evaluator would accept.

use linkrs_core::types::operators::BinaryOperator;
use linkrs_core::value::NullType;
use linkrs_core::Value;
use linkrs_query::executor::expression::evaluator::operations::BinaryOperationEvaluator;
use linkrs_storage::ScanPredicate;

fn values() -> Vec<Value> {
    vec![
        Value::SmallInt(-1),
        Value::Int(0),
        Value::BigInt(1),
        Value::Float(0.5),
        Value::Double(0.5),
        Value::Float(f32::NAN),
        Value::Double(f64::NAN),
        Value::Null(NullType::Null),
        Value::Null(NullType::NaN),
        Value::string("a"),
        Value::Bool(false),
    ]
}

fn literals() -> Vec<Value> {
    values()
        .into_iter()
        .filter(|value| !matches!(value, Value::Null(_)))
        .collect()
}

fn predicate(op: BinaryOperator, literal: &Value) -> ScanPredicate {
    let column = "c".into();
    match op {
        BinaryOperator::Equal => ScanPredicate::ColumnEqual {
            column,
            value: literal.clone(),
        },
        BinaryOperator::GreaterThan => ScanPredicate::ColumnRange {
            column,
            lower: Some(literal.clone()),
            upper: None,
            include_lower: false,
            include_upper: false,
        },
        BinaryOperator::GreaterThanOrEqual => ScanPredicate::ColumnRange {
            column,
            lower: Some(literal.clone()),
            upper: None,
            include_lower: true,
            include_upper: false,
        },
        BinaryOperator::LessThan => ScanPredicate::ColumnRange {
            column,
            lower: None,
            upper: Some(literal.clone()),
            include_lower: false,
            include_upper: false,
        },
        BinaryOperator::LessThanOrEqual => ScanPredicate::ColumnRange {
            column,
            lower: None,
            upper: Some(literal.clone()),
            include_lower: false,
            include_upper: true,
        },
        other => panic!("operator {other:?} is not pushable"),
    }
}

#[test]
fn pushed_predicate_matches_evaluator_for_every_scalar_pair() {
    let ops = [
        BinaryOperator::Equal,
        BinaryOperator::GreaterThan,
        BinaryOperator::GreaterThanOrEqual,
        BinaryOperator::LessThan,
        BinaryOperator::LessThanOrEqual,
    ];
    for op in ops {
        for literal in literals() {
            let pushed = predicate(op, &literal);
            for value in values() {
                let evaluated = BinaryOperationEvaluator::evaluate(&value, &op, &literal)
                    .expect("scalar comparison succeeds");
                let expected = matches!(evaluated, Value::Bool(true));
                assert_eq!(
                    pushed.matches_value(&value),
                    expected,
                    "mismatch for {value:?} {op:?} {literal:?}"
                );
            }
        }
    }
}

#[test]
fn missing_property_uses_the_null_comparison_result() {
    let op = BinaryOperator::LessThan;
    let literal = Value::Int(15);
    let evaluated = BinaryOperationEvaluator::evaluate(&Value::Null(NullType::Null), &op, &literal)
        .expect("null comparison succeeds");
    assert_eq!(
        predicate(op, &literal).matches(&[]),
        matches!(evaluated, Value::Bool(true))
    );
}
