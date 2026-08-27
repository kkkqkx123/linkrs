use crate::core::value::geography::{Geography, GeographyValue, LineStringValue, PolygonValue};
use crate::core::value::NullType;
use crate::core::Value;
use crate::executor::expression::ExpressionError;

use super::measurements::{calculate_distance, point_to_segment_distance};

pub fn execute_st_intersects(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_intersects(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_intersects function requires geography arguments",
        )),
    }
}

fn check_intersects(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Point(p1), Geography::Point(p2)) => p1.distance(p2) < 0.001,
        (Geography::Point(p), Geography::Polygon(poly)) => poly.contains_point(p),
        (Geography::Polygon(poly), Geography::Point(p)) => poly.contains_point(p),
        (Geography::Point(p), Geography::MultiPolygon(mp)) => mp.contains_point(p),
        (Geography::MultiPolygon(mp), Geography::Point(p)) => mp.contains_point(p),
        _ => {
            if let (Some(bbox1), Some(bbox2)) = (geo1.bounding_box(), geo2.bounding_box()) {
                bbox_intersect(&bbox1, &bbox2)
            } else {
                false
            }
        }
    }
}

fn bbox_intersect(a: &(f64, f64, f64, f64), b: &(f64, f64, f64, f64)) -> bool {
    a.0 <= b.1 && a.1 >= b.0 && a.2 <= b.3 && a.3 >= b.2
}

pub fn execute_st_covers(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_covers(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_covers function requires geography arguments",
        )),
    }
}

fn check_covers(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Polygon(poly), Geography::Point(p)) => poly.contains_point(p),
        (Geography::MultiPolygon(mp), Geography::Point(p)) => mp.contains_point(p),
        (Geography::Point(p1), Geography::Point(p2)) => p1.distance(p2) < 0.001,
        _ => false,
    }
}

pub fn execute_st_coveredby(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_covers(geo2, geo1);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_coveredby function requires geography arguments",
        )),
    }
}

pub fn execute_st_dwithin(args: &[Value]) -> Result<Value, ExpressionError> {
    let distance = match &args[2] {
        Value::Float(d) => *d as f64,
        Value::Double(d) => *d,
        Value::Int(d) => *d as f64,
        Value::BigInt(d) => *d as f64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The st_dwithin function requires numeric distance parameter",
            ))
        }
    };

    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let actual_distance = calculate_distance(geo1, geo2);
            Ok(Value::Bool(actual_distance <= distance))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_dwithin function requires geography arguments",
        )),
    }
}

pub fn execute_st_contains(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_contains(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_contains function requires geography arguments",
        )),
    }
}

fn check_contains(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Polygon(poly), Geography::Point(p)) => poly.contains_point(p),
        (Geography::MultiPolygon(mp), Geography::Point(p)) => mp.contains_point(p),
        (Geography::Polygon(p1), Geography::Polygon(p2)) => {
            p2.exterior.points.iter().all(|pt| p1.contains_point(pt))
        }
        (Geography::MultiPolygon(mp), Geography::Polygon(p)) => {
            p.exterior.points.iter().all(|pt| mp.contains_point(pt))
        }
        _ => false,
    }
}

pub fn execute_st_within(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_contains(geo2, geo1);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_within function requires geography arguments",
        )),
    }
}

pub fn execute_st_crosses(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_crosses(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_crosses function requires geography arguments",
        )),
    }
}

fn check_crosses(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::LineString(ls), Geography::Polygon(poly)) => {
            linestring_crosses_polygon(ls, poly)
        }
        (Geography::Polygon(poly), Geography::LineString(ls)) => {
            linestring_crosses_polygon(ls, poly)
        }
        (Geography::LineString(ls1), Geography::LineString(ls2)) => linestrings_cross(ls1, ls2),
        _ => false,
    }
}

fn linestring_crosses_polygon(ls: &LineStringValue, poly: &PolygonValue) -> bool {
    if ls.points.len() < 2 {
        return false;
    }
    let mut has_inside = false;
    let mut has_outside = false;
    for pt in &ls.points {
        if poly.contains_point(pt) {
            has_inside = true;
        } else {
            has_outside = true;
        }
        if has_inside && has_outside {
            return true;
        }
    }
    false
}

fn linestrings_cross(ls1: &LineStringValue, ls2: &LineStringValue) -> bool {
    if ls1.points.len() < 2 || ls2.points.len() < 2 {
        return false;
    }
    for i in 0..ls1.points.len() - 1 {
        for j in 0..ls2.points.len() - 1 {
            if segments_intersect(
                &ls1.points[i],
                &ls1.points[i + 1],
                &ls2.points[j],
                &ls2.points[j + 1],
            ) {
                return true;
            }
        }
    }
    false
}

fn segments_intersect(
    p1: &GeographyValue,
    p2: &GeographyValue,
    p3: &GeographyValue,
    p4: &GeographyValue,
) -> bool {
    fn ccw(a: &GeographyValue, b: &GeographyValue, c: &GeographyValue) -> bool {
        (c.latitude - a.latitude) * (b.longitude - a.longitude)
            > (b.latitude - a.latitude) * (c.longitude - a.longitude)
    }
    ccw(p1, p3, p4) != ccw(p2, p3, p4) && ccw(p1, p2, p3) != ccw(p1, p2, p4)
}

pub fn execute_st_touches(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_touches(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_touches function requires geography arguments",
        )),
    }
}

fn check_touches(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Point(p), Geography::LineString(ls)) => point_touches_linestring(p, ls),
        (Geography::LineString(ls), Geography::Point(p)) => point_touches_linestring(p, ls),
        (Geography::Point(p), Geography::Polygon(poly)) => point_touches_polygon(p, poly),
        (Geography::Polygon(poly), Geography::Point(p)) => point_touches_polygon(p, poly),
        (Geography::LineString(ls1), Geography::LineString(ls2)) => linestrings_touch(ls1, ls2),
        (Geography::Polygon(poly), Geography::LineString(ls)) => {
            linestring_touches_polygon(ls, poly)
        }
        (Geography::LineString(ls), Geography::Polygon(poly)) => {
            linestring_touches_polygon(ls, poly)
        }
        _ => false,
    }
}

fn point_touches_linestring(point: &GeographyValue, ls: &LineStringValue) -> bool {
    for pt in &ls.points {
        if point.distance(pt) < 0.001 {
            return true;
        }
    }
    false
}

fn point_touches_polygon(point: &GeographyValue, poly: &PolygonValue) -> bool {
    if poly.contains_point(point) {
        return false;
    }
    for window in poly.exterior.points.windows(2) {
        let dist = point_to_segment_distance(point, &window[0], &window[1]);
        if dist < 0.001 {
            return true;
        }
    }
    false
}

fn linestrings_touch(ls1: &LineStringValue, ls2: &LineStringValue) -> bool {
    for p1 in &ls1.points {
        for p2 in &ls2.points {
            if p1.distance(p2) < 0.001 {
                return true;
            }
        }
    }
    false
}

fn linestring_touches_polygon(ls: &LineStringValue, poly: &PolygonValue) -> bool {
    for pt in &ls.points {
        if poly.contains_point(pt) {
            return false;
        }
    }
    for pt in &ls.points {
        for window in poly.exterior.points.windows(2) {
            let dist = point_to_segment_distance(pt, &window[0], &window[1]);
            if dist < 0.001 {
                return true;
            }
        }
    }
    false
}

pub fn execute_st_overlaps(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_overlaps(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_overlaps function requires geography arguments",
        )),
    }
}

fn check_overlaps(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Polygon(p1), Geography::Polygon(p2)) => polygons_overlap(p1, p2),
        (Geography::LineString(ls1), Geography::LineString(ls2)) => linestrings_overlap(ls1, ls2),
        _ => false,
    }
}

fn polygons_overlap(p1: &PolygonValue, p2: &PolygonValue) -> bool {
    let has_p1_in_p2 = p2.exterior.points.iter().any(|pt| p1.contains_point(pt));
    let has_p2_in_p1 = p1.exterior.points.iter().any(|pt| p2.contains_point(pt));
    let all_p1_in_p2 = p2.exterior.points.iter().all(|pt| p1.contains_point(pt));
    let all_p2_in_p1 = p1.exterior.points.iter().all(|pt| p2.contains_point(pt));
    (has_p1_in_p2 || has_p2_in_p1) && !all_p1_in_p2 && !all_p2_in_p1
}

fn linestrings_overlap(ls1: &LineStringValue, ls2: &LineStringValue) -> bool {
    if ls1.points.len() < 2 || ls2.points.len() < 2 {
        return false;
    }
    for i in 0..ls1.points.len() - 1 {
        for j in 0..ls2.points.len() - 1 {
            if segments_intersect(
                &ls1.points[i],
                &ls1.points[i + 1],
                &ls2.points[j],
                &ls2.points[j + 1],
            ) {
                return true;
            }
        }
    }
    false
}

pub fn execute_st_equals(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let result = check_equals(geo1, geo2);
            Ok(Value::Bool(result))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_equals function requires geography arguments",
        )),
    }
}

fn check_equals(geo1: &Geography, geo2: &Geography) -> bool {
    match (geo1, geo2) {
        (Geography::Point(p1), Geography::Point(p2)) => p1.distance(p2) < 0.0001,
        (Geography::LineString(ls1), Geography::LineString(ls2)) => {
            if ls1.points.len() != ls2.points.len() {
                return false;
            }
            ls1.points.iter().zip(ls2.points.iter()).all(|(p1, p2)| {
                (p1.latitude - p2.latitude).abs() < 0.0001
                    && (p1.longitude - p2.longitude).abs() < 0.0001
            })
        }
        (Geography::Polygon(p1), Geography::Polygon(p2)) => {
            if p1.exterior.points.len() != p2.exterior.points.len() {
                return false;
            }
            p1.exterior
                .points
                .iter()
                .zip(p2.exterior.points.iter())
                .all(|(pt1, pt2)| {
                    (pt1.latitude - pt2.latitude).abs() < 0.0001
                        && (pt1.longitude - pt2.longitude).abs() < 0.0001
                })
        }
        _ => false,
    }
}
