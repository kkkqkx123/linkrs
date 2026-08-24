use serde_json::Value;

use crate::error::{Result, VectorClientError};
use crate::types::*;

/// Typed representation of a shared [`ConditionType::Match`] value.
///
/// The local engine matches stringified values; remote Qdrant matches by
/// payload type. Classifying here lets every transport pick the matching
/// variant that real Qdrant evaluates against the stored payload type:
/// `"42"` becomes an integer match, `"true"` a boolean match.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassifiedMatch {
    Integer(i64),
    Boolean(bool),
    Keyword(String),
}

/// Classify a shared match value into its wire representation.
///
/// Only canonical spellings are type-promoted (`"42"` → integer match);
/// non-canonical forms such as `"042"` or `"True"` stay keyword matches so
/// remote evaluation matches the local engine's stringified comparison,
/// where `"042"` does not equal the numeric payload `42`.
pub fn classify_match_value(value: &str) -> ClassifiedMatch {
    if let Ok(parsed) = value.parse::<i64>() {
        if parsed.to_string() == value {
            return ClassifiedMatch::Integer(parsed);
        }
    }
    if let Ok(parsed) = value.parse::<bool>() {
        return ClassifiedMatch::Boolean(parsed);
    }
    ClassifiedMatch::Keyword(value.to_string())
}

/// Homogeneous typing of a shared [`ConditionType::MatchAny`] list.
///
/// Mixed-type (or non-scalar) lists have no single Qdrant variant; they
/// degrade to [`ClassifiedMatchAny::Strings`] holding stringified elements,
/// mirroring the local engine's stringified comparison for scalars.
#[derive(Debug, Clone, PartialEq)]
pub enum ClassifiedMatchAny {
    Strings(Vec<String>),
    Integers(Vec<i64>),
    Booleans(Vec<bool>),
}

/// Partition a `match_any` value list by element type.
///
/// A list of pure JSON strings maps to string matching, a list of pure
/// integers to integer matching, and a list of pure booleans to boolean
/// matching; anything mixed (or containing floats, nulls, objects) degrades
/// to the stringified representation.
pub fn classify_match_any(values: &[Value]) -> ClassifiedMatchAny {
    let mut strings = Vec::with_capacity(values.len());
    let mut integers = Vec::with_capacity(values.len());
    let mut booleans = Vec::with_capacity(values.len());
    let mut all_integers = true;
    let mut all_booleans = true;

    for v in values {
        match v {
            Value::String(s) => {
                strings.push(s.clone());
                all_integers = false;
                all_booleans = false;
            }
            Value::Number(n) => {
                strings.push(n.to_string());
                match n.as_i64() {
                    Some(i) => integers.push(i),
                    None => all_integers = false,
                }
                all_booleans = false;
            }
            Value::Bool(b) => {
                strings.push(b.to_string());
                booleans.push(*b);
                all_integers = false;
            }
            _ => {
                return ClassifiedMatchAny::Strings(
                    values.iter().filter_map(stringify_json).collect(),
                )
            }
        }
    }

    if all_booleans && !booleans.is_empty() {
        ClassifiedMatchAny::Booleans(booleans)
    } else if all_integers && !integers.is_empty() {
        ClassifiedMatchAny::Integers(integers)
    } else {
        // Covers pure-string lists and every degraded case alike.
        ClassifiedMatchAny::Strings(strings)
    }
}

fn stringify_json(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub trait ConditionHandler {
    type Condition;
    type Filter;

    fn handle_match(&self, field: &str, value: &str) -> Self::Condition;
    fn handle_match_any(&self, field: &str, values: &[Value]) -> Self::Condition;
    fn handle_range(&self, field: &str, range: &RangeCondition) -> Self::Condition;
    fn handle_is_empty(&self, field: &str) -> Self::Condition;
    fn handle_is_null(&self, field: &str) -> Self::Condition;
    fn handle_has_id(&self, ids: &[String]) -> Self::Condition;
    fn handle_geo_radius(&self, field: &str, radius: &GeoRadius) -> Self::Condition;
    fn handle_geo_bounding_box(&self, field: &str, bbox: &GeoBoundingBox) -> Self::Condition;
    fn handle_values_count(&self, field: &str, count: &ValuesCountCondition) -> Self::Condition;
    fn handle_contains(&self, field: &str, value: &str) -> Self::Condition;
    fn handle_nested(&self, field: &str, filter: Self::Filter) -> Self::Condition;
    fn build_filter(
        &self,
        must: Vec<Self::Condition>,
        must_not: Vec<Self::Condition>,
        should: Vec<Self::Condition>,
        min_should: Option<(Vec<Self::Condition>, usize)>,
    ) -> Option<Self::Filter>;
}

pub fn process_filter<H: ConditionHandler>(
    filter: &VectorFilter,
    handler: &H,
) -> Result<Option<H::Filter>> {
    let mut should: Vec<H::Condition> = Vec::new();
    let mut must: Vec<H::Condition> = Vec::new();
    let mut must_not: Vec<H::Condition> = Vec::new();

    if let Some(ref conditions) = filter.must {
        for c in conditions {
            must.push(handle_condition(c, handler)?);
        }
    }

    if let Some(ref conditions) = filter.must_not {
        for c in conditions {
            must_not.push(handle_condition(c, handler)?);
        }
    }

    if let Some(ref conditions) = filter.should {
        for c in conditions {
            should.push(handle_condition(c, handler)?);
        }
    }

    let min_should = if let Some(ref ms) = filter.min_should {
        let mut conditions = Vec::new();
        for c in &ms.conditions {
            conditions.push(handle_condition(c, handler)?);
        }
        Some((conditions, ms.min_count))
    } else {
        None
    };

    if should.is_empty() && must.is_empty() && must_not.is_empty() && min_should.is_none() {
        return Ok(None);
    }

    Ok(handler.build_filter(must, must_not, should, min_should))
}

fn handle_condition<H: ConditionHandler>(c: &FilterCondition, handler: &H) -> Result<H::Condition> {
    match &c.condition {
        ConditionType::Match { value } => Ok(handler.handle_match(&c.field, value)),
        ConditionType::MatchAny { values } => Ok(handler.handle_match_any(&c.field, values)),
        ConditionType::Range(range) => Ok(handler.handle_range(&c.field, range)),
        ConditionType::IsEmpty => Ok(handler.handle_is_empty(&c.field)),
        ConditionType::IsNull => Ok(handler.handle_is_null(&c.field)),
        ConditionType::HasId { ids } => Ok(handler.handle_has_id(ids)),
        ConditionType::Nested { filter } => {
            let nested = process_filter(filter, handler)?
                .ok_or_else(|| VectorClientError::FilterError("Empty nested filter".to_string()))?;
            Ok(handler.handle_nested(&c.field, nested))
        }
        ConditionType::GeoRadius(radius) => Ok(handler.handle_geo_radius(&c.field, radius)),
        ConditionType::GeoBoundingBox(bbox) => Ok(handler.handle_geo_bounding_box(&c.field, bbox)),
        ConditionType::ValuesCount(count) => Ok(handler.handle_values_count(&c.field, count)),
        ConditionType::Contains { value } => Ok(handler.handle_contains(&c.field, value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_classify_match_value_integer() {
        assert_eq!(classify_match_value("42"), ClassifiedMatch::Integer(42));
        assert_eq!(classify_match_value("-7"), ClassifiedMatch::Integer(-7));
    }

    #[test]
    fn test_classify_match_value_boolean() {
        assert_eq!(classify_match_value("true"), ClassifiedMatch::Boolean(true));
        assert_eq!(
            classify_match_value("false"),
            ClassifiedMatch::Boolean(false)
        );
    }

    #[test]
    fn test_classify_match_value_keyword() {
        // Non-canonical spellings must stay keyword matches.
        assert_eq!(
            classify_match_value("red"),
            ClassifiedMatch::Keyword("red".to_string())
        );
        assert_eq!(
            classify_match_value("042"),
            ClassifiedMatch::Keyword("042".to_string())
        );
        assert_eq!(
            classify_match_value("True"),
            ClassifiedMatch::Keyword("True".to_string())
        );
        assert_eq!(
            classify_match_value("3.5"),
            ClassifiedMatch::Keyword("3.5".to_string())
        );
    }

    #[test]
    fn test_classify_match_any_strings() {
        let values = vec![json!("a"), json!("b")];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Strings(vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn test_classify_match_any_integers() {
        let values = vec![json!(1), json!(-2)];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Integers(vec![1, -2])
        );
    }

    #[test]
    fn test_classify_match_any_booleans() {
        let values = vec![json!(true), json!(false)];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Booleans(vec![true, false])
        );
    }

    #[test]
    fn test_classify_match_any_mixed_degrades_to_stringified() {
        let values = vec![json!("a"), json!(1), json!(true)];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Strings(vec!["a".to_string(), "1".to_string(), "true".to_string()])
        );
    }

    #[test]
    fn test_classify_match_any_bool_with_string_degrades_to_stringified() {
        let values = vec![json!(true), json!("on")];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Strings(vec!["true".to_string(), "on".to_string()])
        );
    }

    #[test]
    fn test_classify_match_any_floats_degrade_to_stringified() {
        let values = vec![json!(1.5), json!(2.5)];
        assert_eq!(
            classify_match_any(&values),
            ClassifiedMatchAny::Strings(vec!["1.5".to_string(), "2.5".to_string()])
        );
    }

    struct TestCondition {
        field: String,
        _cond_type: String,
    }

    struct TestFilter {
        must: Vec<TestCondition>,
        must_not: Vec<TestCondition>,
        should: Vec<TestCondition>,
        min_should: Option<(Vec<TestCondition>, usize)>,
    }

    struct TestHandler;

    impl ConditionHandler for TestHandler {
        type Condition = TestCondition;
        type Filter = TestFilter;

        fn handle_match(&self, field: &str, value: &str) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: format!("match:{}", value),
            }
        }

        fn handle_match_any(&self, field: &str, values: &[Value]) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: format!("match_any:{}", values.len()),
            }
        }

        fn handle_range(&self, field: &str, _range: &RangeCondition) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "range".into(),
            }
        }

        fn handle_is_empty(&self, field: &str) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "is_empty".into(),
            }
        }

        fn handle_is_null(&self, field: &str) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "is_null".into(),
            }
        }

        fn handle_has_id(&self, ids: &[String]) -> TestCondition {
            TestCondition {
                field: "_id".into(),
                _cond_type: format!("has_id:{}", ids.len()),
            }
        }

        fn handle_geo_radius(&self, field: &str, _radius: &GeoRadius) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "geo_radius".into(),
            }
        }

        fn handle_geo_bounding_box(&self, field: &str, _bbox: &GeoBoundingBox) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "geo_bbox".into(),
            }
        }

        fn handle_values_count(&self, field: &str, _count: &ValuesCountCondition) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "values_count".into(),
            }
        }

        fn handle_contains(&self, field: &str, _value: &str) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: "contains".into(),
            }
        }

        fn handle_nested(&self, field: &str, filter: TestFilter) -> TestCondition {
            TestCondition {
                field: field.into(),
                _cond_type: format!("nested:{}_must_cond", filter.must.len()),
            }
        }

        fn build_filter(
            &self,
            must: Vec<TestCondition>,
            must_not: Vec<TestCondition>,
            should: Vec<TestCondition>,
            min_should: Option<(Vec<TestCondition>, usize)>,
        ) -> Option<TestFilter> {
            if must.is_empty() && must_not.is_empty() && should.is_empty() && min_should.is_none() {
                return None;
            }

            Some(TestFilter {
                must,
                must_not,
                should,
                min_should,
            })
        }
    }

    #[test]
    fn test_process_filter_empty() {
        let filter = VectorFilter::new();
        let result = process_filter(&filter, &TestHandler).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_process_filter_must() {
        let filter = VectorFilter::new().must(FilterCondition::match_value("color", "red"));
        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        assert_eq!(result.must.len(), 1);
        assert_eq!(result.must[0].field, "color");
    }

    #[test]
    fn test_process_filter_must_not() {
        let filter = VectorFilter::new().must_not(FilterCondition::is_null("deleted"));
        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        assert_eq!(result.must_not.len(), 1);
        assert_eq!(result.must_not[0].field, "deleted");
    }

    #[test]
    fn test_process_filter_should() {
        let filter = VectorFilter::new()
            .should(FilterCondition::match_value("tag", "a"))
            .should(FilterCondition::contains("title", "rust"));
        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        assert_eq!(result.should.len(), 2);
    }

    #[test]
    fn test_process_filter_min_should() {
        let filter = VectorFilter {
            must: None,
            must_not: None,
            should: None,
            min_should: Some(MinShouldCondition {
                conditions: vec![FilterCondition::match_value("a", "b")],
                min_count: 1,
            }),
        };
        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        let (conds, min) = result.min_should.unwrap();
        assert_eq!(conds.len(), 1);
        assert_eq!(min, 1);
    }

    #[test]
    fn test_process_filter_all_types() {
        let filter = VectorFilter::new()
            .must(FilterCondition::match_value("f1", "v1"))
            .must(FilterCondition::match_any(
                "f2",
                vec![serde_json::json!("a")],
            ))
            .must(FilterCondition::range("f3", RangeCondition::new().gt(10.0)))
            .must(FilterCondition::is_empty("f4"))
            .must(FilterCondition::is_null("f5"))
            .must(FilterCondition::has_id(vec!["1".into()]))
            .must(FilterCondition::geo_radius(
                "f6",
                GeoRadius::new(GeoPoint::new(1.0, 2.0), 100.0),
            ))
            .must(FilterCondition::values_count(
                "f7",
                ValuesCountCondition::new().gt(1),
            ))
            .must(FilterCondition::contains("f8", "needle"));

        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        assert_eq!(result.must.len(), 9);
    }

    #[test]
    fn test_process_filter_nested() {
        let inner = VectorFilter::new().must(FilterCondition::match_value("inner_field", "val"));
        let nested = FilterCondition {
            field: "nested".into(),
            condition: ConditionType::Nested {
                filter: Box::new(inner),
            },
        };
        let filter = VectorFilter::new().must(nested);
        let result = process_filter(&filter, &TestHandler).unwrap().unwrap();
        assert_eq!(result.must.len(), 1);
    }

    #[test]
    fn test_handle_nested_empty_returns_error() {
        let nested = FilterCondition {
            field: "nested".into(),
            condition: ConditionType::Nested {
                filter: Box::new(VectorFilter::new()),
            },
        };
        let filter = VectorFilter::new().must(nested);
        let result = process_filter(&filter, &TestHandler);
        assert!(result.is_err());
    }
}
