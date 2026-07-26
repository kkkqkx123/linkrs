use super::*;
use crate::core::value::geography::{Geography, GeographyValue, LineStringValue, PolygonValue};
use crate::core::value::NullType;
use crate::core::Value;

#[test]
fn test_st_point() {
    let func = GeographyFunction::StPoint;
    let result = func
        .execute(&[Value::Float(116.4074), Value::Float(39.9042)])
        .expect("Implementation should not fail");
    assert!(matches!(result, Value::Geography(_)));
}

#[test]
fn test_st_isvalid() {
    let func = GeographyFunction::StIsValid;
    let geo = Geography::Point(GeographyValue::new(39.9042, 116.4074));
    let result = func
        .execute(&[Value::Geography(geo)])
        .expect("Implementation should not fail");
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_st_distance() {
    let func = GeographyFunction::StDistance;
    let geo1 = Geography::Point(GeographyValue::new(39.9042, 116.4074));
    let geo2 = Geography::Point(GeographyValue::new(31.2304, 121.4737));
    let result = func
        .execute(&[Value::Geography(geo1), Value::Geography(geo2)])
        .expect("Implementation should not fail");
    assert!(matches!(result, Value::Double(_)));
}

#[test]
fn test_null_handling() {
    let func = GeographyFunction::StIsValid;
    let result = func
        .execute(&[Value::Null(NullType::Null)])
        .expect("Implementation should not fail");
    assert_eq!(result, Value::Null(NullType::Null));
}

#[test]
fn test_st_linestring() {
    let wkt = "LINESTRING(116.4 39.9, 121.5 31.2)";
    let result = execute_st_geogfromtext(&[Value::string(wkt.to_string())]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::LineString(_))));
}

#[test]
fn test_st_polygon() {
    let wkt = "POLYGON((116.0 40.0, 117.0 40.0, 117.0 39.0, 116.0 39.0, 116.0 40.0))";
    let result = execute_st_geogfromtext(&[Value::string(wkt.to_string())]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::Polygon(_))));
}

#[test]
fn test_st_length() {
    let ls = LineStringValue::new(vec![
        GeographyValue::new(39.9, 116.4),
        GeographyValue::new(31.2, 121.5),
    ]);
    let result = execute_st_length(&[Value::Geography(Geography::LineString(ls))]).unwrap();
    assert!(matches!(result, Value::Double(d) if d > 1000.0));
}

#[test]
fn test_st_contains() {
    let polygon = PolygonValue::new(
        LineStringValue::new(vec![
            GeographyValue::new(40.0, 116.0),
            GeographyValue::new(40.0, 117.0),
            GeographyValue::new(39.0, 117.0),
            GeographyValue::new(39.0, 116.0),
            GeographyValue::new(40.0, 116.0),
        ]),
        vec![],
    );
    let point_inside = Geography::Point(GeographyValue::new(39.5, 116.5));
    let point_outside = Geography::Point(GeographyValue::new(50.0, 120.0));

    let result_inside = execute_st_contains(&[
        Value::Geography(Geography::Polygon(polygon.clone())),
        Value::Geography(point_inside),
    ])
    .unwrap();
    assert_eq!(result_inside, Value::Bool(true));

    let result_outside = execute_st_contains(&[
        Value::Geography(Geography::Polygon(polygon)),
        Value::Geography(point_outside),
    ])
    .unwrap();
    assert_eq!(result_outside, Value::Bool(false));
}

#[test]
fn test_st_geometrytype() {
    let point = Geography::Point(GeographyValue::new(39.9, 116.4));
    let result = execute_st_geometrytype(&[Value::Geography(point)]).unwrap();
    assert_eq!(result, Value::string("Point".to_string()));

    let ls = Geography::LineString(LineStringValue::new(vec![
        GeographyValue::new(39.9, 116.4),
        GeographyValue::new(31.2, 121.5),
    ]));
    let result = execute_st_geometrytype(&[Value::Geography(ls)]).unwrap();
    assert_eq!(result, Value::string("LineString".to_string()));
}

#[test]
fn test_st_buffer() {
    let point = Geography::Point(GeographyValue::new(39.9, 116.4));
    let result = execute_st_buffer(&[Value::Geography(point), Value::Double(10.0)]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::Polygon(_))));
}

#[test]
fn test_st_boundary() {
    let ls = LineStringValue::new(vec![
        GeographyValue::new(39.9, 116.4),
        GeographyValue::new(31.2, 121.5),
    ]);
    let result = execute_st_boundary(&[Value::Geography(Geography::LineString(ls))]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::MultiPoint(_))));

    let polygon = PolygonValue::new(
        LineStringValue::new(vec![
            GeographyValue::new(40.0, 116.0),
            GeographyValue::new(40.0, 117.0),
            GeographyValue::new(39.0, 117.0),
            GeographyValue::new(39.0, 116.0),
            GeographyValue::new(40.0, 116.0),
        ]),
        vec![],
    );
    let result = execute_st_boundary(&[Value::Geography(Geography::Polygon(polygon))]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::LineString(_))));
}

#[test]
fn test_st_crosses() {
    let polygon = PolygonValue::new(
        LineStringValue::new(vec![
            GeographyValue::new(40.0, 116.0),
            GeographyValue::new(40.0, 117.0),
            GeographyValue::new(39.0, 117.0),
            GeographyValue::new(39.0, 116.0),
            GeographyValue::new(40.0, 116.0),
        ]),
        vec![],
    );
    let ls = LineStringValue::new(vec![
        GeographyValue::new(39.5, 116.5),
        GeographyValue::new(40.5, 116.5),
    ]);
    let result = execute_st_crosses(&[
        Value::Geography(Geography::LineString(ls)),
        Value::Geography(Geography::Polygon(polygon)),
    ])
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_st_touches() {
    let point = Geography::Point(GeographyValue::new(40.0, 116.0));
    let ls = LineStringValue::new(vec![
        GeographyValue::new(40.0, 116.0),
        GeographyValue::new(40.0, 117.0),
    ]);
    let result = execute_st_touches(&[
        Value::Geography(point),
        Value::Geography(Geography::LineString(ls)),
    ])
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_st_overlaps() {
    let p1 = PolygonValue::new(
        LineStringValue::new(vec![
            GeographyValue::new(40.0, 116.0),
            GeographyValue::new(40.0, 117.0),
            GeographyValue::new(39.0, 117.0),
            GeographyValue::new(39.0, 116.0),
            GeographyValue::new(40.0, 116.0),
        ]),
        vec![],
    );
    let p2 = PolygonValue::new(
        LineStringValue::new(vec![
            GeographyValue::new(39.5, 116.5),
            GeographyValue::new(39.5, 117.5),
            GeographyValue::new(38.5, 117.5),
            GeographyValue::new(38.5, 116.5),
            GeographyValue::new(39.5, 116.5),
        ]),
        vec![],
    );
    let result = execute_st_overlaps(&[
        Value::Geography(Geography::Polygon(p1)),
        Value::Geography(Geography::Polygon(p2)),
    ])
    .unwrap();
    assert_eq!(result, Value::Bool(true));
}

#[test]
fn test_st_equals() {
    let p1 = Geography::Point(GeographyValue::new(39.9, 116.4));
    let p2 = Geography::Point(GeographyValue::new(39.9, 116.4));
    let result = execute_st_equals(&[Value::Geography(p1), Value::Geography(p2)]).unwrap();
    assert_eq!(result, Value::Bool(true));

    let p3 = Geography::Point(GeographyValue::new(31.2, 121.5));
    let result = execute_st_equals(&[
        Value::Geography(Geography::Point(GeographyValue::new(39.9, 116.4))),
        Value::Geography(p3),
    ])
    .unwrap();
    assert_eq!(result, Value::Bool(false));
}

#[test]
fn test_st_asgeojson() {
    let point = Geography::Point(GeographyValue::new(39.9, 116.4));
    let result = execute_st_asgeojson(&[Value::Geography(point)]).unwrap();
    if let Value::String(json) = result {
        assert!(json.contains("\"type\":\"Point\""));
        assert!(json.contains("\"coordinates\""));
    } else {
        panic!("Expected String value");
    }
}

#[test]
fn test_st_geomfromgeojson() {
    let json = r#"{"type":"Point","coordinates":[116.4,39.9]}"#;
    let result = execute_st_geomfromgeojson(&[Value::string(json.to_string())]).unwrap();
    assert!(matches!(result, Value::Geography(Geography::Point(_))));
}

#[test]
fn test_geojson_roundtrip() {
    let point = Geography::Point(GeographyValue::new(39.9, 116.4));
    let json = execute_st_asgeojson(&[Value::Geography(point.clone())]).unwrap();
    let parsed = execute_st_geomfromgeojson(&[json]).unwrap();
    assert_eq!(Value::Geography(point), parsed);
}
