//! Common test utilities for geography tests
//!
//! Re-exports common test utilities from the parent directory.

use graphdb_core::core::value::geography::{
    Geography, GeographyValue, LineStringValue, MultiLineStringValue, MultiPointValue,
    MultiPolygonValue, PolygonValue,
};
use std::collections::HashMap;

type Coordinate = (f64, f64);
type PolygonSpec = (Vec<Coordinate>, Vec<Vec<Coordinate>>);

/// Geography Test Context
#[allow(dead_code)]
pub struct GeographyTestContext {
    // Test context fields can be added here
}

#[allow(dead_code)]
impl GeographyTestContext {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for GeographyTestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Create test point
pub fn create_test_point(lon: f64, lat: f64) -> Geography {
    Geography::Point(GeographyValue::new(lat, lon))
}

/// Create test linestring from coordinate pairs
pub fn create_test_linestring(points: Vec<(f64, f64)>) -> Geography {
    let geo_points: Vec<GeographyValue> = points
        .into_iter()
        .map(|(lon, lat)| GeographyValue::new(lat, lon))
        .collect();
    Geography::LineString(LineStringValue::new(geo_points))
}

/// Create test polygon with exterior ring and optional holes
pub fn create_test_polygon(exterior: Vec<(f64, f64)>, holes: Vec<Vec<(f64, f64)>>) -> Geography {
    let exterior_points: Vec<GeographyValue> = exterior
        .into_iter()
        .map(|(lon, lat)| GeographyValue::new(lat, lon))
        .collect();

    let hole_rings: Vec<LineStringValue> = holes
        .into_iter()
        .map(|hole| {
            let points: Vec<GeographyValue> = hole
                .into_iter()
                .map(|(lon, lat)| GeographyValue::new(lat, lon))
                .collect();
            LineStringValue::new(points)
        })
        .collect();

    Geography::Polygon(PolygonValue::new(
        LineStringValue::new(exterior_points),
        hole_rings,
    ))
}

/// Create test multipoint from coordinate pairs
pub fn create_test_multipoint(points: Vec<(f64, f64)>) -> Geography {
    let geo_points: Vec<GeographyValue> = points
        .into_iter()
        .map(|(lon, lat)| GeographyValue::new(lat, lon))
        .collect();
    Geography::MultiPoint(MultiPointValue::new(geo_points))
}

/// Create test multilinestring from multiple linestrings
pub fn create_test_multilinestring(linestrings: Vec<Vec<(f64, f64)>>) -> Geography {
    let geo_linestrings: Vec<LineStringValue> = linestrings
        .into_iter()
        .map(|points| {
            let geo_points: Vec<GeographyValue> = points
                .into_iter()
                .map(|(lon, lat)| GeographyValue::new(lat, lon))
                .collect();
            LineStringValue::new(geo_points)
        })
        .collect();
    Geography::MultiLineString(MultiLineStringValue::new(geo_linestrings))
}

/// Create test multipolygon from multiple polygons
pub fn create_test_multipolygon(polygons: Vec<PolygonSpec>) -> Geography {
    let geo_polygons: Vec<PolygonValue> = polygons
        .into_iter()
        .map(|(exterior, holes)| {
            let exterior_points: Vec<GeographyValue> = exterior
                .into_iter()
                .map(|(lon, lat)| GeographyValue::new(lat, lon))
                .collect();

            let hole_rings: Vec<LineStringValue> = holes
                .into_iter()
                .map(|hole| {
                    let points: Vec<GeographyValue> = hole
                        .into_iter()
                        .map(|(lon, lat)| GeographyValue::new(lat, lon))
                        .collect();
                    LineStringValue::new(points)
                })
                .collect();

            PolygonValue::new(LineStringValue::new(exterior_points), hole_rings)
        })
        .collect();
    Geography::MultiPolygon(MultiPolygonValue::new(geo_polygons))
}

/// Assert geography equals with tolerance
pub fn assert_geography_equals(geo1: &Geography, geo2: &Geography, tolerance: f64) {
    match (geo1, geo2) {
        (Geography::Point(p1), Geography::Point(p2)) => {
            assert!(
                (p1.latitude - p2.latitude).abs() < tolerance,
                "Latitude mismatch: {} vs {}",
                p1.latitude,
                p2.latitude
            );
            assert!(
                (p1.longitude - p2.longitude).abs() < tolerance,
                "Longitude mismatch: {} vs {}",
                p1.longitude,
                p2.longitude
            );
        }
        _ => {
            assert_eq!(geo1, geo2, "Geometries should be equal");
        }
    }
}

/// Assert distance within tolerance
pub fn assert_distance_within(actual: f64, expected: f64, tolerance: f64) {
    assert!(
        (actual - expected).abs() < tolerance,
        "Distance {} not within {} of expected {}",
        actual,
        tolerance,
        expected
    );
}

/// Get standard test geometries
pub fn get_standard_test_geometries() -> HashMap<String, Geography> {
    let mut geometries = HashMap::new();

    geometries.insert("beijing".to_string(), create_test_point(116.4074, 39.9042));
    geometries.insert("shanghai".to_string(), create_test_point(121.4737, 31.2304));
    geometries.insert("newyork".to_string(), create_test_point(-74.0060, 40.7128));
    geometries.insert("origin".to_string(), create_test_point(0.0, 0.0));

    geometries.insert(
        "simple_line".to_string(),
        create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]),
    );

    geometries.insert(
        "unit_square".to_string(),
        create_test_polygon(
            vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
            vec![],
        ),
    );

    geometries.insert(
        "square_with_hole".to_string(),
        create_test_polygon(
            vec![(0.0, 0.0), (0.0, 3.0), (3.0, 3.0), (3.0, 0.0), (0.0, 0.0)],
            vec![vec![
                (1.0, 1.0),
                (1.0, 2.0),
                (2.0, 2.0),
                (2.0, 1.0),
                (1.0, 1.0),
            ]],
        ),
    );

    geometries.insert(
        "multi_points".to_string(),
        create_test_multipoint(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]),
    );

    geometries.insert(
        "multi_lines".to_string(),
        create_test_multilinestring(vec![
            vec![(0.0, 0.0), (1.0, 1.0)],
            vec![(2.0, 2.0), (3.0, 3.0)],
        ]),
    );

    geometries.insert(
        "multi_polygons".to_string(),
        create_test_multipolygon(vec![
            (
                vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
                vec![],
            ),
            (
                vec![(2.0, 2.0), (2.0, 3.0), (3.0, 3.0), (3.0, 2.0), (2.0, 2.0)],
                vec![],
            ),
        ]),
    );

    geometries
}

/// Get edge case test geometries
pub fn get_edge_case_geometries() -> HashMap<String, Geography> {
    let mut geometries = HashMap::new();

    geometries.insert(
        "empty_linestring".to_string(),
        Geography::LineString(LineStringValue::new(vec![])),
    );

    geometries.insert(
        "empty_multipoint".to_string(),
        Geography::MultiPoint(MultiPointValue::new(vec![])),
    );

    geometries.insert(
        "invalid_point_lat".to_string(),
        Geography::Point(GeographyValue::new(100.0, 0.0)),
    );

    geometries.insert(
        "invalid_point_lon".to_string(),
        Geography::Point(GeographyValue::new(0.0, 200.0)),
    );

    geometries
}

/// Create a circular polygon (approximation)
#[allow(dead_code)]
pub fn create_circular_polygon(
    center_lon: f64,
    center_lat: f64,
    radius_deg: f64,
    num_points: usize,
) -> Geography {
    let mut points = Vec::with_capacity(num_points + 1);

    for i in 0..num_points {
        let angle = 2.0 * std::f64::consts::PI * i as f64 / num_points as f64;
        let lon = center_lon + radius_deg * angle.cos();
        let lat = center_lat + radius_deg * angle.sin();
        points.push((lon, lat));
    }
    points.push(points[0]);

    create_test_polygon(points, vec![])
}
