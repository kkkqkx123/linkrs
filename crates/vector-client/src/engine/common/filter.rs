use serde_json::Value;

use crate::error::{Result, VectorClientError};
use crate::types::*;

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
    use serde_json::Value;

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
