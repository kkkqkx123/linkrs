use crate::core::value::geography::{
    Geography, GeographyValue, LineStringValue, MultiLineStringValue, MultiPointValue, PolygonValue,
};
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

pub fn execute_st_distance(args: &[Value]) -> Result<Value, ExpressionError> {
    match (&args[0], &args[1]) {
        (Value::Geography(geo1), Value::Geography(geo2)) => {
            let distance = calculate_distance(geo1, geo2);
            Ok(Value::Double(distance))
        }
        (Value::Null(_), _) | (_, Value::Null(_)) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_distance function requires geography arguments",
        )),
    }
}

pub fn execute_st_area(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let area = match geo {
                Geography::Polygon(p) => p.area(),
                Geography::MultiPolygon(mp) => mp.area(),
                _ => 0.0,
            };
            Ok(Value::Double(area))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_area function requires polygon or multipolygon type",
        )),
    }
}

pub fn execute_st_length(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let length = match geo {
                Geography::LineString(ls) => ls.length(),
                Geography::MultiLineString(mls) => mls.length(),
                Geography::Polygon(p) => p.perimeter(),
                Geography::MultiPolygon(mp) => mp.polygons.iter().map(|p| p.perimeter()).sum(),
                _ => 0.0,
            };
            Ok(Value::Double(length))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_length function requires linestring or polygon type",
        )),
    }
}

pub fn execute_st_perimeter(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let perimeter = match geo {
                Geography::Polygon(p) => p.perimeter(),
                Geography::MultiPolygon(mp) => mp.polygons.iter().map(|p| p.perimeter()).sum(),
                _ => 0.0,
            };
            Ok(Value::Double(perimeter))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_perimeter function requires polygon type",
        )),
    }
}

pub fn execute_st_npoints(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let count = match geo {
                Geography::Point(_) => 1,
                Geography::LineString(ls) => ls.points.len(),
                Geography::Polygon(p) => {
                    p.exterior.points.len() + p.holes.iter().map(|h| h.points.len()).sum::<usize>()
                }
                Geography::MultiPoint(mp) => mp.points.len(),
                Geography::MultiLineString(mls) => {
                    mls.linestrings.iter().map(|ls| ls.points.len()).sum()
                }
                Geography::MultiPolygon(mp) => mp
                    .polygons
                    .iter()
                    .map(|p| {
                        p.exterior.points.len()
                            + p.holes.iter().map(|h| h.points.len()).sum::<usize>()
                    })
                    .sum(),
            };
            Ok(Value::Int(count as i32))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_npoints function requires geography type",
        )),
    }
}

pub fn execute_st_startpoint(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo {
            Geography::LineString(ls) => match ls.start_point() {
                Some(p) => Ok(Value::Geography(Geography::Point(p.clone()))),
                None => Ok(Value::Null(NullType::Null)),
            },
            _ => Err(ExpressionError::type_error(
                "The st_startpoint function requires linestring type",
            )),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_startpoint function requires geography type",
        )),
    }
}

pub fn execute_st_endpoint(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo {
            Geography::LineString(ls) => match ls.end_point() {
                Some(p) => Ok(Value::Geography(Geography::Point(p.clone()))),
                None => Ok(Value::Null(NullType::Null)),
            },
            _ => Err(ExpressionError::type_error(
                "The st_endpoint function requires linestring type",
            )),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_endpoint function requires geography type",
        )),
    }
}

pub fn execute_st_centroid(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo.centroid() {
            Some(point) => Ok(Value::Geography(Geography::Point(point))),
            None => Ok(Value::Null(NullType::Null)),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_centroid function requires the geography type",
        )),
    }
}

pub fn execute_st_isvalid(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => Ok(Value::Bool(geo.is_valid())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_isvalid function requires the geography type",
        )),
    }
}

pub fn execute_st_geometrytype(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => Ok(Value::string(geo.geometry_type())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geometrytype function requires geography type",
        )),
    }
}

pub fn execute_st_isring(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo {
            Geography::LineString(ls) => Ok(Value::Bool(ls.is_ring())),
            _ => Ok(Value::Bool(false)),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_isring function requires geography type",
        )),
    }
}

pub fn execute_st_isclosed(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo {
            Geography::LineString(ls) => Ok(Value::Bool(ls.is_closed())),
            _ => Ok(Value::Bool(false)),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_isclosed function requires geography type",
        )),
    }
}

pub fn execute_st_envelope(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => match geo.bounding_box() {
            Some((min_lat, max_lat, min_lon, max_lon)) => {
                let envelope = PolygonValue::new(
                    LineStringValue::new(vec![
                        GeographyValue::new(min_lat, min_lon),
                        GeographyValue::new(max_lat, min_lon),
                        GeographyValue::new(max_lat, max_lon),
                        GeographyValue::new(min_lat, max_lon),
                        GeographyValue::new(min_lat, min_lon),
                    ]),
                    vec![],
                );
                Ok(Value::Geography(Geography::Polygon(envelope)))
            }
            None => Ok(Value::Null(NullType::Null)),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_envelope function requires geography type",
        )),
    }
}

pub fn execute_st_boundary(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let boundary = get_boundary(geo);
            match boundary {
                Some(b) => Ok(Value::Geography(b)),
                None => Ok(Value::Null(NullType::Null)),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_boundary function requires geography type",
        )),
    }
}

pub fn calculate_distance(geo1: &Geography, geo2: &Geography) -> f64 {
    match (geo1, geo2) {
        (Geography::Point(p1), Geography::Point(p2)) => p1.distance(p2),
        (Geography::Point(p), Geography::LineString(ls)) => point_to_linestring_distance(p, ls),
        (Geography::LineString(ls), Geography::Point(p)) => point_to_linestring_distance(p, ls),
        (Geography::Point(p), Geography::Polygon(poly)) => {
            if poly.contains_point(p) {
                0.0
            } else {
                point_to_polygon_distance(p, poly)
            }
        }
        (Geography::Polygon(poly), Geography::Point(p)) => {
            if poly.contains_point(p) {
                0.0
            } else {
                point_to_polygon_distance(p, poly)
            }
        }
        _ => {
            let c1 = geo1.centroid();
            let c2 = geo2.centroid();
            match (c1, c2) {
                (Some(p1), Some(p2)) => p1.distance(&p2),
                _ => f64::MAX,
            }
        }
    }
}

fn point_to_linestring_distance(point: &GeographyValue, ls: &LineStringValue) -> f64 {
    if ls.points.is_empty() {
        return f64::MAX;
    }

    let mut min_dist = f64::MAX;
    for window in ls.points.windows(2) {
        let dist = point_to_segment_distance(point, &window[0], &window[1]);
        min_dist = min_dist.min(dist);
    }
    min_dist
}

fn point_to_polygon_distance(point: &GeographyValue, poly: &PolygonValue) -> f64 {
    let mut min_dist = point_to_linestring_distance(point, &poly.exterior);
    for hole in &poly.holes {
        let dist = point_to_linestring_distance(point, hole);
        min_dist = min_dist.min(dist);
    }
    min_dist
}

pub fn point_to_segment_distance(
    point: &GeographyValue,
    seg_start: &GeographyValue,
    seg_end: &GeographyValue,
) -> f64 {
    let d1 = point.distance(seg_start);
    let d2 = point.distance(seg_end);
    let seg_len = seg_start.distance(seg_end);

    if seg_len < 1e-9 {
        return d1;
    }

    let t = ((point.latitude - seg_start.latitude) * (seg_end.latitude - seg_start.latitude)
        + (point.longitude - seg_start.longitude) * (seg_end.longitude - seg_start.longitude))
        / ((seg_end.latitude - seg_start.latitude).powi(2)
            + (seg_end.longitude - seg_start.longitude).powi(2));

    if t <= 0.0 {
        d1
    } else if t >= 1.0 {
        d2
    } else {
        let proj = GeographyValue::new(
            seg_start.latitude + t * (seg_end.latitude - seg_start.latitude),
            seg_start.longitude + t * (seg_end.longitude - seg_start.longitude),
        );
        point.distance(&proj)
    }
}

fn get_boundary(geo: &Geography) -> Option<Geography> {
    match geo {
        Geography::LineString(ls) => {
            if ls.points.len() < 2 {
                return None;
            }
            let start = ls.start_point()?;
            let end = ls.end_point()?;
            if ls.is_closed() {
                return None;
            }
            Some(Geography::MultiPoint(MultiPointValue::new(vec![
                start.clone(),
                end.clone(),
            ])))
        }
        Geography::Polygon(p) => Some(Geography::LineString(p.exterior.clone())),
        Geography::MultiPolygon(mp) => {
            let mut all_boundaries = Vec::new();
            for p in &mp.polygons {
                all_boundaries.push(p.exterior.clone());
            }
            Some(Geography::MultiLineString(MultiLineStringValue::new(
                all_boundaries,
            )))
        }
        _ => None,
    }
}
