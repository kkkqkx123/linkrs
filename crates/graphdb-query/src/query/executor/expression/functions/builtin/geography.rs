//! Implementation of geospatial functions

use crate::core::value::geography::{Geography, GeographyValue, LineStringValue, PolygonValue};
use crate::core::value::NullType;
use crate::core::Value;
use crate::query::executor::expression::ExpressionError;

define_function_enum! {
    /// Enumeration of geospatial functions
    pub enum GeographyFunction {
        StPoint => {
            name: "st_point",
            arity: 2,
            variadic: false,
            description: "Create Geographic Points (Longitude, Latitude)",
            handler: execute_st_point
        },
        StGeogFromText => {
            name: "st_geogfromtext",
            arity: 1,
            variadic: false,
            description: "Creating geographic objects from WKT text",
            handler: execute_st_geogfromtext
        },
        StAsText => {
            name: "st_astext",
            arity: 1,
            variadic: false,
            description: "Convert geographic objects to WKT text",
            handler: execute_st_astext
        },
        StCentroid => {
            name: "st_centroid",
            arity: 1,
            variadic: false,
            description: "Calculate the center point of a geographic object",
            handler: execute_st_centroid
        },
        StIsValid => {
            name: "st_isvalid",
            arity: 1,
            variadic: false,
            description: "Checking the validity of geographic objects",
            handler: execute_st_isvalid
        },
        StIntersects => {
            name: "st_intersects",
            arity: 2,
            variadic: false,
            description: "Check if two geographic objects intersect",
            handler: execute_st_intersects
        },
        StCovers => {
            name: "st_covers",
            arity: 2,
            variadic: false,
            description: "Check if the first geographic object overrides the second",
            handler: execute_st_covers
        },
        StCoveredBy => {
            name: "st_coveredby",
            arity: 2,
            variadic: false,
            description: "Check if the first geographic object is overwritten by the second",
            handler: execute_st_coveredby
        },
        StDWithin => {
            name: "st_dwithin",
            arity: 3,
            variadic: false,
            description: "Check that two geographic objects are within the specified distance (in kilometers)",
            handler: execute_st_dwithin
        },
        StDistance => {
            name: "st_distance",
            arity: 2,
            variadic: false,
            description: "Calculation of the distance between two geographical objects (in kilometers)",
            handler: execute_st_distance
        },
        StArea => {
            name: "st_area",
            arity: 1,
            variadic: false,
            description: "Calculate the area of a polygon in square kilometers",
            handler: execute_st_area
        },
        StLength => {
            name: "st_length",
            arity: 1,
            variadic: false,
            description: "Calculate the length of a linestring in kilometers",
            handler: execute_st_length
        },
        StPerimeter => {
            name: "st_perimeter",
            arity: 1,
            variadic: false,
            description: "Calculate the perimeter of a polygon in kilometers",
            handler: execute_st_perimeter
        },
        StNPoints => {
            name: "st_npoints",
            arity: 1,
            variadic: false,
            description: "Return the number of points in a geometry",
            handler: execute_st_npoints
        },
        StStartPoint => {
            name: "st_startpoint",
            arity: 1,
            variadic: false,
            description: "Return the start point of a linestring",
            handler: execute_st_startpoint
        },
        StEndPoint => {
            name: "st_endpoint",
            arity: 1,
            variadic: false,
            description: "Return the end point of a linestring",
            handler: execute_st_endpoint
        },
        StIsRing => {
            name: "st_isring",
            arity: 1,
            variadic: false,
            description: "Check if a linestring is a ring",
            handler: execute_st_isring
        },
        StIsClosed => {
            name: "st_isclosed",
            arity: 1,
            variadic: false,
            description: "Check if a linestring is closed",
            handler: execute_st_isclosed
        },
        StGeometryType => {
            name: "st_geometrytype",
            arity: 1,
            variadic: false,
            description: "Return the geometry type name",
            handler: execute_st_geometrytype
        },
        StContains => {
            name: "st_contains",
            arity: 2,
            variadic: false,
            description: "Check if geometry A contains geometry B",
            handler: execute_st_contains
        },
        StWithin => {
            name: "st_within",
            arity: 2,
            variadic: false,
            description: "Check if geometry A is within geometry B",
            handler: execute_st_within
        },
        StEnvelope => {
            name: "st_envelope",
            arity: 1,
            variadic: false,
            description: "Return the bounding box of a geometry as a polygon",
            handler: execute_st_envelope
        },
        StBuffer => {
            name: "st_buffer",
            arity: 2,
            variadic: false,
            description: "Create a buffer around a geometry",
            handler: execute_st_buffer
        },
        StBoundary => {
            name: "st_boundary",
            arity: 1,
            variadic: false,
            description: "Return the boundary of a geometry",
            handler: execute_st_boundary
        },
        StCrosses => {
            name: "st_crosses",
            arity: 2,
            variadic: false,
            description: "Check if geometry A crosses geometry B",
            handler: execute_st_crosses
        },
        StTouches => {
            name: "st_touches",
            arity: 2,
            variadic: false,
            description: "Check if geometry A touches geometry B",
            handler: execute_st_touches
        },
        StOverlaps => {
            name: "st_overlaps",
            arity: 2,
            variadic: false,
            description: "Check if geometry A overlaps geometry B",
            handler: execute_st_overlaps
        },
        StEquals => {
            name: "st_equals",
            arity: 2,
            variadic: false,
            description: "Check if two geometries are spatially equal",
            handler: execute_st_equals
        },
        StAsGeoJson => {
            name: "st_asgeojson",
            arity: 1,
            variadic: false,
            description: "Convert geography to GeoJSON string",
            handler: execute_st_asgeojson
        },
        StGeomFromGeoJson => {
            name: "st_geomfromgeojson",
            arity: 1,
            variadic: false,
            description: "Create geography from GeoJSON string",
            handler: execute_st_geomfromgeojson
        },
    }
}

fn execute_st_point(args: &[Value]) -> Result<Value, ExpressionError> {
    let (lon, lat) = match (&args[0], &args[1]) {
        (Value::Float(lon), Value::Float(lat)) => (*lon as f64, *lat as f64),
        (Value::Double(lon), Value::Double(lat)) => (*lon, *lat),
        (Value::SmallInt(lon), Value::SmallInt(lat)) => (*lon as f64, *lat as f64),
        (Value::Int(lon), Value::Int(lat)) => (*lon as f64, *lat as f64),
        (Value::BigInt(lon), Value::BigInt(lat)) => (*lon as f64, *lat as f64),
        (Value::Float(lon), Value::Double(lat)) => (*lon as f64, *lat),
        (Value::Double(lon), Value::Float(lat)) => (*lon, *lat as f64),
        (Value::Null(_), _) | (_, Value::Null(_)) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The st_point function takes numeric arguments",
            ))
        }
    };

    let geo = Geography::Point(GeographyValue::new(lat, lon));
    Ok(Value::Geography(geo))
}

fn execute_st_geogfromtext(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(wkt) => match Geography::from_wkt(wkt) {
            Ok(geo) => Ok(Value::Geography(geo)),
            Err(e) => Err(ExpressionError::type_error(format!(
                "Failed to parse WKT: {}",
                e
            ))),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geogfromtext function takes string arguments",
        )),
    }
}

fn execute_st_astext(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let wkt = geo.to_wkt();
            Ok(Value::String(wkt))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_astext function requires the geographic type",
        )),
    }
}

fn execute_st_centroid(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_isvalid(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => Ok(Value::Bool(geo.is_valid())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_isvalid function requires the geography type",
        )),
    }
}

fn execute_st_intersects(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_covers(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_coveredby(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_distance(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn calculate_distance(geo1: &Geography, geo2: &Geography) -> f64 {
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

fn point_to_segment_distance(
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

fn point_to_polygon_distance(point: &GeographyValue, poly: &PolygonValue) -> f64 {
    let mut min_dist = point_to_linestring_distance(point, &poly.exterior);
    for hole in &poly.holes {
        let dist = point_to_linestring_distance(point, hole);
        min_dist = min_dist.min(dist);
    }
    min_dist
}

fn execute_st_dwithin(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_area(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_length(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_perimeter(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_npoints(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_startpoint(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_endpoint(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_isring(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_isclosed(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_geometrytype(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => Ok(Value::String(geo.geometry_type().to_string())),
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geometrytype function requires geography type",
        )),
    }
}

fn execute_st_contains(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_within(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_envelope(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_buffer(args: &[Value]) -> Result<Value, ExpressionError> {
    let distance_km = match &args[1] {
        Value::Double(d) => *d,
        Value::Float(d) => *d as f64,
        Value::Int(d) => *d as f64,
        Value::BigInt(d) => *d as f64,
        Value::Null(_) => return Ok(Value::Null(NullType::Null)),
        _ => {
            return Err(ExpressionError::type_error(
                "The st_buffer function requires numeric distance parameter",
            ))
        }
    };

    match &args[0] {
        Value::Geography(geo) => {
            let buffer = create_buffer(geo, distance_km);
            match buffer {
                Some(polygon) => Ok(Value::Geography(Geography::Polygon(polygon))),
                None => Ok(Value::Null(NullType::Null)),
            }
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_buffer function requires geography type",
        )),
    }
}

fn create_buffer(geo: &Geography, radius_km: f64) -> Option<PolygonValue> {
    const NUM_SEGMENTS: usize = 32;
    match geo {
        Geography::Point(p) => {
            let mut points = Vec::with_capacity(NUM_SEGMENTS + 1);
            for i in 0..NUM_SEGMENTS {
                let angle = 2.0 * std::f64::consts::PI * i as f64 / NUM_SEGMENTS as f64;
                let (lat, lon) = destination_point(p.latitude, p.longitude, radius_km, angle);
                points.push(GeographyValue::new(lat, lon));
            }
            points.push(points[0].clone());
            Some(PolygonValue::new(LineStringValue::new(points), vec![]))
        }
        Geography::LineString(ls) => {
            let mut all_points = Vec::new();
            for window in ls.points.windows(2) {
                let buffer_points = create_segment_buffer(&window[0], &window[1], radius_km);
                all_points.extend(buffer_points);
            }
            if all_points.is_empty() {
                return None;
            }
            all_points.push(all_points[0].clone());
            Some(PolygonValue::new(LineStringValue::new(all_points), vec![]))
        }
        _ => None,
    }
}

fn create_segment_buffer(
    start: &GeographyValue,
    end: &GeographyValue,
    radius_km: f64,
) -> Vec<GeographyValue> {
    const NUM_SEGMENTS_PER_END: usize = 8;
    let mut points = Vec::new();

    let dx = end.longitude - start.longitude;
    let dy = end.latitude - start.latitude;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 1e-9 {
        return points;
    }

    let perp_x = -dy / length;
    let perp_y = dx / length;

    let offset_lat = perp_y * radius_km / 111.0;
    let offset_lon = perp_x * radius_km / 111.0;

    points.push(GeographyValue::new(
        start.latitude + offset_lat,
        start.longitude + offset_lon,
    ));

    for i in 1..NUM_SEGMENTS_PER_END {
        let angle = std::f64::consts::PI * (0.5 + i as f64 / NUM_SEGMENTS_PER_END as f64);
        let (lat, lon) = destination_point(start.latitude, start.longitude, radius_km, angle);
        points.push(GeographyValue::new(lat, lon));
    }

    points.push(GeographyValue::new(
        end.latitude + offset_lat,
        end.longitude + offset_lon,
    ));

    for i in 1..NUM_SEGMENTS_PER_END {
        let angle = std::f64::consts::PI * (1.5 + i as f64 / NUM_SEGMENTS_PER_END as f64);
        let (lat, lon) = destination_point(end.latitude, end.longitude, radius_km, angle);
        points.push(GeographyValue::new(lat, lon));
    }

    points
}

fn destination_point(lat: f64, lon: f64, distance_km: f64, bearing: f64) -> (f64, f64) {
    const EARTH_RADIUS_KM: f64 = 6371.0;
    let lat_rad = lat.to_radians();
    let lon_rad = lon.to_radians();
    let bearing_rad = bearing;
    let angular_dist = distance_km / EARTH_RADIUS_KM;

    let new_lat = (lat_rad.cos() * angular_dist.cos()
        - lat_rad.sin() * angular_dist.sin() * bearing_rad.cos())
    .asin();
    let new_lon = lon_rad
        + (bearing_rad.sin() * angular_dist.sin() * lat_rad.cos())
            .atan2(angular_dist.cos() - lat_rad.sin() * new_lat.sin());

    (new_lat.to_degrees(), new_lon.to_degrees())
}

fn execute_st_boundary(args: &[Value]) -> Result<Value, ExpressionError> {
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
            Some(Geography::MultiPoint(
                crate::core::value::geography::MultiPointValue::new(vec![
                    start.clone(),
                    end.clone(),
                ]),
            ))
        }
        Geography::Polygon(p) => Some(Geography::LineString(p.exterior.clone())),
        Geography::MultiPolygon(mp) => {
            let mut all_boundaries = Vec::new();
            for p in &mp.polygons {
                all_boundaries.push(p.exterior.clone());
            }
            Some(Geography::MultiLineString(
                crate::core::value::geography::MultiLineStringValue::new(all_boundaries),
            ))
        }
        _ => None,
    }
}

fn execute_st_crosses(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_touches(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_overlaps(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_equals(args: &[Value]) -> Result<Value, ExpressionError> {
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

fn execute_st_asgeojson(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::Geography(geo) => {
            let json_str = geo.to_geojson_string();
            Ok(Value::String(json_str))
        }
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_asgeojson function requires geography type",
        )),
    }
}

fn execute_st_geomfromgeojson(args: &[Value]) -> Result<Value, ExpressionError> {
    match &args[0] {
        Value::String(json_str) => match Geography::from_geojson_string(json_str) {
            Ok(geo) => Ok(Value::Geography(geo)),
            Err(e) => Err(ExpressionError::type_error(format!(
                "Invalid GeoJSON: {}",
                e
            ))),
        },
        Value::Null(_) => Ok(Value::Null(NullType::Null)),
        _ => Err(ExpressionError::type_error(
            "The st_geomfromgeojson function requires string argument",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = execute_st_geogfromtext(&[Value::String(wkt.to_string())]).unwrap();
        assert!(matches!(result, Value::Geography(Geography::LineString(_))));
    }

    #[test]
    fn test_st_polygon() {
        let wkt = "POLYGON((116.0 40.0, 117.0 40.0, 117.0 39.0, 116.0 39.0, 116.0 40.0))";
        let result = execute_st_geogfromtext(&[Value::String(wkt.to_string())]).unwrap();
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
        assert_eq!(result, Value::String("Point".to_string()));

        let ls = Geography::LineString(LineStringValue::new(vec![
            GeographyValue::new(39.9, 116.4),
            GeographyValue::new(31.2, 121.5),
        ]));
        let result = execute_st_geometrytype(&[Value::Geography(ls)]).unwrap();
        assert_eq!(result, Value::String("LineString".to_string()));
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
        let result = execute_st_geomfromgeojson(&[Value::String(json.to_string())]).unwrap();
        assert!(matches!(result, Value::Geography(Geography::Point(_))));
    }

    #[test]
    fn test_geojson_roundtrip() {
        let point = Geography::Point(GeographyValue::new(39.9, 116.4));
        let json = execute_st_asgeojson(&[Value::Geography(point.clone())]).unwrap();
        let parsed = execute_st_geomfromgeojson(&[json]).unwrap();
        assert_eq!(Value::Geography(point), parsed);
    }
}
