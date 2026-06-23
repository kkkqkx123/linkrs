use serde::{Deserialize, Serialize};

pub type PayloadValue = serde_json::Value;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct GeoPoint {
    pub lat: f64,
    pub lon: f64,
}

impl GeoPoint {
    pub fn new(lat: f64, lon: f64) -> Self {
        Self { lat, lon }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoRadius {
    pub center: GeoPoint,
    pub radius: f64,
}

impl GeoRadius {
    pub fn new(center: GeoPoint, radius: f64) -> Self {
        Self { center, radius }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoBoundingBox {
    pub top_left: GeoPoint,
    pub bottom_right: GeoPoint,
}

impl GeoBoundingBox {
    pub fn new(top_left: GeoPoint, bottom_right: GeoPoint) -> Self {
        Self {
            top_left,
            bottom_right,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValuesCountCondition {
    pub gt: Option<u64>,
    pub gte: Option<u64>,
    pub lt: Option<u64>,
    pub lte: Option<u64>,
}

impl ValuesCountCondition {
    pub fn new() -> Self {
        Self {
            gt: None,
            gte: None,
            lt: None,
            lte: None,
        }
    }

    pub fn gt(mut self, value: u64) -> Self {
        self.gt = Some(value);
        self
    }

    pub fn gte(mut self, value: u64) -> Self {
        self.gte = Some(value);
        self
    }

    pub fn lt(mut self, value: u64) -> Self {
        self.lt = Some(value);
        self
    }

    pub fn lte(mut self, value: u64) -> Self {
        self.lte = Some(value);
        self
    }
}

impl Default for ValuesCountCondition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorFilter {
    pub must: Option<Vec<FilterCondition>>,
    pub must_not: Option<Vec<FilterCondition>>,
    pub should: Option<Vec<FilterCondition>>,
    pub min_should: Option<MinShouldCondition>,
}

impl VectorFilter {
    pub fn new() -> Self {
        Self {
            must: None,
            must_not: None,
            should: None,
            min_should: None,
        }
    }

    pub fn must(mut self, condition: FilterCondition) -> Self {
        self.must.get_or_insert_with(Vec::new).push(condition);
        self
    }

    pub fn must_not(mut self, condition: FilterCondition) -> Self {
        self.must_not.get_or_insert_with(Vec::new).push(condition);
        self
    }

    pub fn should(mut self, condition: FilterCondition) -> Self {
        self.should.get_or_insert_with(Vec::new).push(condition);
        self
    }
}

impl Default for VectorFilter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinShouldCondition {
    pub conditions: Vec<FilterCondition>,
    pub min_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilterCondition {
    pub field: String,
    pub condition: ConditionType,
}

impl FilterCondition {
    pub fn new(field: impl Into<String>, condition: ConditionType) -> Self {
        Self {
            field: field.into(),
            condition,
        }
    }

    pub fn match_value(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            field,
            ConditionType::Match {
                value: value.into(),
            },
        )
    }

    pub fn match_any(field: impl Into<String>, values: Vec<serde_json::Value>) -> Self {
        Self::new(field, ConditionType::MatchAny { values })
    }

    pub fn range(field: impl Into<String>, range: RangeCondition) -> Self {
        Self::new(field, ConditionType::Range(range))
    }

    pub fn is_empty(field: impl Into<String>) -> Self {
        Self::new(field, ConditionType::IsEmpty)
    }

    pub fn is_null(field: impl Into<String>) -> Self {
        Self::new(field, ConditionType::IsNull)
    }

    pub fn has_id(ids: Vec<String>) -> Self {
        Self::new("_id", ConditionType::HasId { ids })
    }

    pub fn geo_radius(field: impl Into<String>, radius: GeoRadius) -> Self {
        Self::new(field, ConditionType::GeoRadius(radius))
    }

    pub fn geo_bounding_box(field: impl Into<String>, bbox: GeoBoundingBox) -> Self {
        Self::new(field, ConditionType::GeoBoundingBox(bbox))
    }

    pub fn values_count(field: impl Into<String>, count: ValuesCountCondition) -> Self {
        Self::new(field, ConditionType::ValuesCount(count))
    }

    pub fn contains(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::new(
            field,
            ConditionType::Contains {
                value: value.into(),
            },
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConditionType {
    Match { value: String },
    MatchAny { values: Vec<PayloadValue> },
    Range(RangeCondition),
    IsEmpty,
    IsNull,
    HasId { ids: Vec<String> },
    Nested { filter: Box<VectorFilter> },
    GeoRadius(GeoRadius),
    GeoBoundingBox(GeoBoundingBox),
    ValuesCount(ValuesCountCondition),
    Contains { value: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RangeCondition {
    pub gt: Option<f64>,
    pub gte: Option<f64>,
    pub lt: Option<f64>,
    pub lte: Option<f64>,
}

impl RangeCondition {
    pub fn new() -> Self {
        Self {
            gt: None,
            gte: None,
            lt: None,
            lte: None,
        }
    }

    pub fn gt(mut self, value: f64) -> Self {
        self.gt = Some(value);
        self
    }

    pub fn gte(mut self, value: f64) -> Self {
        self.gte = Some(value);
        self
    }

    pub fn lt(mut self, value: f64) -> Self {
        self.lt = Some(value);
        self
    }

    pub fn lte(mut self, value: f64) -> Self {
        self.lte = Some(value);
        self
    }
}

impl Default for RangeCondition {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadSelector {
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

impl PayloadSelector {
    pub fn include(fields: Vec<String>) -> Self {
        Self {
            include: Some(fields),
            exclude: None,
        }
    }

    pub fn exclude(fields: Vec<String>) -> Self {
        Self {
            include: None,
            exclude: Some(fields),
        }
    }

    pub fn all() -> Self {
        Self {
            include: None,
            exclude: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_geo_point_new() {
        let p = GeoPoint::new(1.0, 2.0);
        assert!((p.lat - 1.0).abs() < f64::EPSILON);
        assert!((p.lon - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_geo_radius_new() {
        let center = GeoPoint::new(0.0, 0.0);
        let r = GeoRadius::new(center, 100.0);
        assert!((r.radius - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_geo_bounding_box_new() {
        let tl = GeoPoint::new(1.0, 2.0);
        let br = GeoPoint::new(3.0, 4.0);
        let bbox = GeoBoundingBox::new(tl, br);
        assert!((bbox.top_left.lat - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_values_count_condition_default() {
        let v = ValuesCountCondition::default();
        assert!(v.gt.is_none());
        assert!(v.gte.is_none());
        assert!(v.lt.is_none());
        assert!(v.lte.is_none());
    }

    #[test]
    fn test_values_count_condition_builder() {
        let v = ValuesCountCondition::new().gt(5).lt(10);
        assert_eq!(v.gt, Some(5));
        assert_eq!(v.lt, Some(10));
        assert!(v.gte.is_none());
        assert!(v.lte.is_none());
    }

    #[test]
    fn test_range_condition_default() {
        let r = RangeCondition::default();
        assert!(r.gt.is_none());
        assert!(r.gte.is_none());
        assert!(r.lt.is_none());
        assert!(r.lte.is_none());
    }

    #[test]
    fn test_range_condition_builder() {
        let r = RangeCondition::new().gte(1.5).lt(10.0);
        assert_eq!(r.gte, Some(1.5));
        assert_eq!(r.lt, Some(10.0));
        assert!(r.gt.is_none());
        assert!(r.lte.is_none());
    }

    #[test]
    fn test_vector_filter_default() {
        let f = VectorFilter::default();
        assert!(f.must.is_none());
        assert!(f.must_not.is_none());
        assert!(f.should.is_none());
        assert!(f.min_should.is_none());
    }

    #[test]
    fn test_vector_filter_must() {
        let f = VectorFilter::new()
            .must(FilterCondition::match_value("color", "red"))
            .must(FilterCondition::match_value("size", "large"));
        assert_eq!(f.must.as_ref().map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_vector_filter_must_not() {
        let f = VectorFilter::new().must_not(FilterCondition::is_null("deleted"));
        assert_eq!(f.must_not.as_ref().map(|v| v.len()), Some(1));
    }

    #[test]
    fn test_vector_filter_should() {
        let f = VectorFilter::new()
            .should(FilterCondition::match_value("tag", "a"))
            .should(FilterCondition::match_value("tag", "b"));
        assert_eq!(f.should.as_ref().map(|v| v.len()), Some(2));
    }

    #[test]
    fn test_filter_condition_match_value() {
        let c = FilterCondition::match_value("color", "blue");
        assert_eq!(c.field, "color");
        match c.condition {
            ConditionType::Match { value } => assert_eq!(value, "blue"),
            _ => panic!("expected Match"),
        }
    }

    #[test]
    fn test_filter_condition_match_any() {
        let values = vec![serde_json::json!("a"), serde_json::json!("b")];
        let c = FilterCondition::match_any("tags", values);
        assert_eq!(c.field, "tags");
    }

    #[test]
    fn test_filter_condition_range() {
        let range = RangeCondition::new().gt(10.0);
        let c = FilterCondition::range("price", range);
        assert_eq!(c.field, "price");
    }

    #[test]
    fn test_filter_condition_is_empty() {
        let c = FilterCondition::is_empty("description");
        assert_eq!(c.field, "description");
        assert!(matches!(c.condition, ConditionType::IsEmpty));
    }

    #[test]
    fn test_filter_condition_is_null() {
        let c = FilterCondition::is_null("deleted_at");
        assert!(matches!(c.condition, ConditionType::IsNull));
    }

    #[test]
    fn test_filter_condition_has_id() {
        let ids = vec!["1".to_string(), "2".to_string()];
        let c = FilterCondition::has_id(ids);
        assert_eq!(c.field, "_id");
    }

    #[test]
    fn test_filter_condition_geo_radius() {
        let center = GeoPoint::new(1.0, 2.0);
        let radius = GeoRadius::new(center, 500.0);
        let c = FilterCondition::geo_radius("location", radius);
        assert_eq!(c.field, "location");
    }

    #[test]
    fn test_filter_condition_geo_bounding_box() {
        let tl = GeoPoint::new(1.0, 2.0);
        let br = GeoPoint::new(3.0, 4.0);
        let bbox = GeoBoundingBox::new(tl, br);
        let c = FilterCondition::geo_bounding_box("location", bbox);
        assert!(matches!(c.condition, ConditionType::GeoBoundingBox(_)));
    }

    #[test]
    fn test_filter_condition_values_count() {
        let count = ValuesCountCondition::new().gt(2);
        let c = FilterCondition::values_count("tags", count);
        assert!(matches!(c.condition, ConditionType::ValuesCount(_)));
    }

    #[test]
    fn test_filter_condition_contains() {
        let c = FilterCondition::contains("title", "rust");
        assert_eq!(c.field, "title");
        match c.condition {
            ConditionType::Contains { value } => assert_eq!(value, "rust"),
            _ => panic!("expected Contains"),
        }
    }

    #[test]
    fn test_payload_selector_include() {
        let sel = PayloadSelector::include(vec!["a".into(), "b".into()]);
        assert_eq!(sel.include.as_ref().map(|v| v.len()), Some(2));
        assert!(sel.exclude.is_none());
    }

    #[test]
    fn test_payload_selector_exclude() {
        let sel = PayloadSelector::exclude(vec!["c".into()]);
        assert_eq!(sel.exclude.as_ref().map(|v| v.len()), Some(1));
        assert!(sel.include.is_none());
    }

    #[test]
    fn test_payload_selector_all() {
        let sel = PayloadSelector::all();
        assert!(sel.include.is_none());
        assert!(sel.exclude.is_none());
    }

    #[test]
    fn test_min_should_condition() {
        let condition = FilterCondition::match_value("field", "val");
        let ms = MinShouldCondition {
            conditions: vec![condition],
            min_count: 1,
        };
        assert_eq!(ms.min_count, 1);
        assert_eq!(ms.conditions.len(), 1);
    }
}
