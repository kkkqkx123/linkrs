//! Mechanical conversions between the transport-side vector types
//! (`graphdb_core::vector`) and the index-side copies owned by `simvec`
//! (`simvec::filter_cond`).
//!
//! The two type families are deliberately duplicated (index semantics live
//! in `simvec`, transport semantics in `graphdb-core`); this module is the
//! single conversion point at the sync-side backend enumeration boundary.
//! The copies must stay structurally identical — the conversions below are
//! field-for-field moves with no semantic translation.
//!
//! Free functions are used instead of `From` impls because both sides are
//! foreign types (orphan rule).

use graphdb_core::vector as tx;
use simvec::filter_cond as ix;

pub fn point_id_to_ix(v: tx::PointId) -> ix::PointId {
    match v {
        tx::PointId::Num(n) => ix::PointId::Num(n),
        tx::PointId::Uuid(s) => ix::PointId::Uuid(s),
    }
}

pub fn point_id_to_tx(v: ix::PointId) -> tx::PointId {
    match v {
        ix::PointId::Num(n) => tx::PointId::Num(n),
        ix::PointId::Uuid(s) => tx::PointId::Uuid(s),
    }
}

pub fn geo_point_to_ix(v: tx::GeoPoint) -> ix::GeoPoint {
    ix::GeoPoint {
        lat: v.lat,
        lon: v.lon,
    }
}

pub fn geo_point_to_tx(v: ix::GeoPoint) -> tx::GeoPoint {
    tx::GeoPoint {
        lat: v.lat,
        lon: v.lon,
    }
}

pub fn geo_radius_to_ix(v: tx::GeoRadius) -> ix::GeoRadius {
    ix::GeoRadius {
        center: geo_point_to_ix(v.center),
        radius: v.radius,
    }
}

pub fn geo_radius_to_tx(v: ix::GeoRadius) -> tx::GeoRadius {
    tx::GeoRadius {
        center: geo_point_to_tx(v.center),
        radius: v.radius,
    }
}

pub fn geo_bounding_box_to_ix(v: tx::GeoBoundingBox) -> ix::GeoBoundingBox {
    ix::GeoBoundingBox {
        top_left: geo_point_to_ix(v.top_left),
        bottom_right: geo_point_to_ix(v.bottom_right),
    }
}

pub fn geo_bounding_box_to_tx(v: ix::GeoBoundingBox) -> tx::GeoBoundingBox {
    tx::GeoBoundingBox {
        top_left: geo_point_to_tx(v.top_left),
        bottom_right: geo_point_to_tx(v.bottom_right),
    }
}

pub fn range_to_ix(v: tx::RangeCondition) -> ix::RangeCondition {
    ix::RangeCondition {
        gt: v.gt,
        gte: v.gte,
        lt: v.lt,
        lte: v.lte,
    }
}

pub fn range_to_tx(v: ix::RangeCondition) -> tx::RangeCondition {
    tx::RangeCondition {
        gt: v.gt,
        gte: v.gte,
        lt: v.lt,
        lte: v.lte,
    }
}

pub fn values_count_to_ix(v: tx::ValuesCountCondition) -> ix::ValuesCountCondition {
    ix::ValuesCountCondition {
        gt: v.gt,
        gte: v.gte,
        lt: v.lt,
        lte: v.lte,
    }
}

pub fn values_count_to_tx(v: ix::ValuesCountCondition) -> tx::ValuesCountCondition {
    tx::ValuesCountCondition {
        gt: v.gt,
        gte: v.gte,
        lt: v.lt,
        lte: v.lte,
    }
}

pub fn condition_to_ix(v: tx::ConditionType) -> ix::ConditionType {
    match v {
        tx::ConditionType::Match { value } => ix::ConditionType::Match { value },
        tx::ConditionType::MatchAny { values } => ix::ConditionType::MatchAny { values },
        tx::ConditionType::Range(r) => ix::ConditionType::Range(range_to_ix(r)),
        tx::ConditionType::IsEmpty => ix::ConditionType::IsEmpty,
        tx::ConditionType::IsNull => ix::ConditionType::IsNull,
        tx::ConditionType::HasId { ids } => ix::ConditionType::HasId { ids },
        tx::ConditionType::Nested { filter } => ix::ConditionType::Nested {
            filter: Box::new(vector_filter_to_ix(*filter)),
        },
        tx::ConditionType::GeoRadius(r) => ix::ConditionType::GeoRadius(geo_radius_to_ix(r)),
        tx::ConditionType::GeoBoundingBox(b) => {
            ix::ConditionType::GeoBoundingBox(geo_bounding_box_to_ix(b))
        }
        tx::ConditionType::ValuesCount(c) => ix::ConditionType::ValuesCount(values_count_to_ix(c)),
        tx::ConditionType::Contains { value } => ix::ConditionType::Contains { value },
    }
}

pub fn condition_to_tx(v: ix::ConditionType) -> tx::ConditionType {
    match v {
        ix::ConditionType::Match { value } => tx::ConditionType::Match { value },
        ix::ConditionType::MatchAny { values } => tx::ConditionType::MatchAny { values },
        ix::ConditionType::Range(r) => tx::ConditionType::Range(range_to_tx(r)),
        ix::ConditionType::IsEmpty => tx::ConditionType::IsEmpty,
        ix::ConditionType::IsNull => tx::ConditionType::IsNull,
        ix::ConditionType::HasId { ids } => tx::ConditionType::HasId { ids },
        ix::ConditionType::Nested { filter } => tx::ConditionType::Nested {
            filter: Box::new(vector_filter_to_tx(*filter)),
        },
        ix::ConditionType::GeoRadius(r) => tx::ConditionType::GeoRadius(geo_radius_to_tx(r)),
        ix::ConditionType::GeoBoundingBox(b) => {
            tx::ConditionType::GeoBoundingBox(geo_bounding_box_to_tx(b))
        }
        ix::ConditionType::ValuesCount(c) => tx::ConditionType::ValuesCount(values_count_to_tx(c)),
        ix::ConditionType::Contains { value } => tx::ConditionType::Contains { value },
    }
}

pub fn filter_condition_to_ix(v: tx::FilterCondition) -> ix::FilterCondition {
    ix::FilterCondition {
        field: v.field,
        condition: condition_to_ix(v.condition),
    }
}

pub fn filter_condition_to_tx(v: ix::FilterCondition) -> tx::FilterCondition {
    tx::FilterCondition {
        field: v.field,
        condition: condition_to_tx(v.condition),
    }
}

pub fn min_should_to_ix(v: tx::MinShouldCondition) -> ix::MinShouldCondition {
    ix::MinShouldCondition {
        conditions: v
            .conditions
            .into_iter()
            .map(filter_condition_to_ix)
            .collect(),
        min_count: v.min_count,
    }
}

pub fn min_should_to_tx(v: ix::MinShouldCondition) -> tx::MinShouldCondition {
    tx::MinShouldCondition {
        conditions: v
            .conditions
            .into_iter()
            .map(filter_condition_to_tx)
            .collect(),
        min_count: v.min_count,
    }
}

pub fn vector_filter_to_ix(v: tx::VectorFilter) -> ix::VectorFilter {
    ix::VectorFilter {
        must: v
            .must
            .map(|list| list.into_iter().map(filter_condition_to_ix).collect()),
        must_not: v
            .must_not
            .map(|list| list.into_iter().map(filter_condition_to_ix).collect()),
        should: v
            .should
            .map(|list| list.into_iter().map(filter_condition_to_ix).collect()),
        min_should: v.min_should.map(min_should_to_ix),
    }
}

pub fn vector_filter_to_tx(v: ix::VectorFilter) -> tx::VectorFilter {
    tx::VectorFilter {
        must: v
            .must
            .map(|list| list.into_iter().map(filter_condition_to_tx).collect()),
        must_not: v
            .must_not
            .map(|list| list.into_iter().map(filter_condition_to_tx).collect()),
        should: v
            .should
            .map(|list| list.into_iter().map(filter_condition_to_tx).collect()),
        min_should: v.min_should.map(min_should_to_tx),
    }
}

pub fn payload_selector_to_ix(v: tx::PayloadSelector) -> ix::PayloadSelector {
    ix::PayloadSelector {
        include: v.include,
        exclude: v.exclude,
    }
}

pub fn payload_selector_to_tx(v: ix::PayloadSelector) -> tx::PayloadSelector {
    tx::PayloadSelector {
        include: v.include,
        exclude: v.exclude,
    }
}

pub fn payload_schema_type_to_ix(v: tx::PayloadSchemaType) -> ix::PayloadSchemaType {
    match v {
        tx::PayloadSchemaType::Keyword => ix::PayloadSchemaType::Keyword,
        tx::PayloadSchemaType::Integer => ix::PayloadSchemaType::Integer,
        tx::PayloadSchemaType::Float => ix::PayloadSchemaType::Float,
        tx::PayloadSchemaType::Text => ix::PayloadSchemaType::Text,
        tx::PayloadSchemaType::Bool => ix::PayloadSchemaType::Bool,
        tx::PayloadSchemaType::Geo => ix::PayloadSchemaType::Geo,
        tx::PayloadSchemaType::Datetime => ix::PayloadSchemaType::Datetime,
    }
}

pub fn payload_schema_type_to_tx(v: ix::PayloadSchemaType) -> tx::PayloadSchemaType {
    match v {
        ix::PayloadSchemaType::Keyword => tx::PayloadSchemaType::Keyword,
        ix::PayloadSchemaType::Integer => tx::PayloadSchemaType::Integer,
        ix::PayloadSchemaType::Float => tx::PayloadSchemaType::Float,
        ix::PayloadSchemaType::Text => tx::PayloadSchemaType::Text,
        ix::PayloadSchemaType::Bool => tx::PayloadSchemaType::Bool,
        ix::PayloadSchemaType::Geo => tx::PayloadSchemaType::Geo,
        ix::PayloadSchemaType::Datetime => tx::PayloadSchemaType::Datetime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_round_trip() {
        let tx_filter = tx::VectorFilter::new()
            .must(tx::FilterCondition::match_value("kind", "person"))
            .must(tx::FilterCondition::range(
                "age",
                tx::RangeCondition::new().gte(18.0),
            ))
            .must_not(tx::FilterCondition::geo_radius(
                "home",
                tx::GeoRadius::new(tx::GeoPoint::new(39.9, 116.4), 1000.0),
            ));

        let ix_filter = vector_filter_to_ix(tx_filter.clone());
        let back = vector_filter_to_tx(ix_filter);
        assert_eq!(
            serde_json::to_string(&tx_filter).unwrap(),
            serde_json::to_string(&back).unwrap()
        );
    }

    #[test]
    fn nested_filter_round_trip() {
        let tx_filter = tx::VectorFilter::new().must(tx::FilterCondition::new(
            "meta",
            tx::ConditionType::Nested {
                filter: Box::new(
                    tx::VectorFilter::new().should(tx::FilterCondition::match_any(
                        "tag",
                        vec![serde_json::json!("a"), serde_json::json!("b")],
                    )),
                ),
            },
        ));

        let ix_filter = vector_filter_to_ix(tx_filter.clone());
        let back = vector_filter_to_tx(ix_filter);
        assert_eq!(
            serde_json::to_string(&tx_filter).unwrap(),
            serde_json::to_string(&back).unwrap()
        );
    }

    #[test]
    fn point_id_round_trip() {
        for id in [tx::PointId::Num(7), tx::PointId::Uuid("abc".into())] {
            let back = point_id_to_tx(point_id_to_ix(id.clone()));
            assert_eq!(id, back);
        }
    }
}
