use crate::executor::expression::ExpressionError;
use graphdb_core::value::geography::{Geography, GeographyValue, LineStringValue, PolygonValue};
use graphdb_core::value::NullType;
use graphdb_core::Value;

pub fn execute_st_buffer(args: &[Value]) -> Result<Value, ExpressionError> {
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
