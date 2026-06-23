//! Geography Integration Tests - Spatial Functions
//!
//! Test scope:
//! - Construction functions - ST_Point, ST_GeogFromText
//! - Conversion functions - to_wkt, to_geojson
//! - Property functions - centroid, is_valid, geometry_type
//! - Measurement functions - distance, area, length, perimeter
//!
//! Test cases: TC-GEO-FUNC-001 ~ TC-GEO-FUNC-020

use super::common::{
    assert_distance_within, create_test_linestring, create_test_point, create_test_polygon,
};
use graphdb_core::core::value::geography::{Geography, GeographyValue};

/// TC-GEO-FUNC-001: Point Creation and Validation
#[test]
fn test_point_creation() {
    let point = create_test_point(116.4074, 39.9042);

    match point {
        Geography::Point(p) => {
            assert_distance_within(p.longitude, 116.4074, 1e-9);
            assert_distance_within(p.latitude, 39.9042, 1e-9);
            assert!(p.is_valid());
        }
        _ => panic!("Expected Point geometry"),
    }
}

/// TC-GEO-FUNC-002: WKT Parsing - Point
#[test]
fn test_wkt_parsing_point() {
    let wkt = "POINT(116.4074 39.9042)";
    let result = Geography::from_wkt(wkt).expect("WKT parsing should succeed");

    match result {
        Geography::Point(p) => {
            assert_distance_within(p.longitude, 116.4074, 1e-9);
            assert_distance_within(p.latitude, 39.9042, 1e-9);
        }
        _ => panic!("Expected Point geometry"),
    }
}

/// TC-GEO-FUNC-003: WKT Parsing - LineString
#[test]
fn test_wkt_parsing_linestring() {
    let wkt = "LINESTRING(0 0, 1 1, 2 2)";
    let result = Geography::from_wkt(wkt).expect("WKT parsing should succeed");

    match result {
        Geography::LineString(ls) => {
            assert_eq!(ls.points.len(), 3);
        }
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-FUNC-004: WKT Parsing - Polygon
#[test]
fn test_wkt_parsing_polygon() {
    let wkt = "POLYGON((0 0, 0 1, 1 1, 1 0, 0 0))";
    let result = Geography::from_wkt(wkt).expect("WKT parsing should succeed");

    match result {
        Geography::Polygon(p) => {
            assert_eq!(p.exterior.points.len(), 5);
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-FUNC-005: WKT Serialization - Point
#[test]
fn test_wkt_serialization_point() {
    let point = create_test_point(116.4074, 39.9042);
    let wkt = point.to_wkt();

    assert!(wkt.contains("POINT"));
    assert!(wkt.contains("116.4074"));
    assert!(wkt.contains("39.9042"));
}

/// TC-GEO-FUNC-006: WKT Round-trip
#[test]
fn test_wkt_roundtrip() {
    let original = create_test_point(116.4074, 39.9042);
    let wkt = original.to_wkt();
    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");

    match (original, parsed) {
        (Geography::Point(p1), Geography::Point(p2)) => {
            assert_distance_within(p1.longitude, p2.longitude, 1e-9);
            assert_distance_within(p1.latitude, p2.latitude, 1e-9);
        }
        _ => panic!("Geometries should match"),
    }
}

/// TC-GEO-FUNC-007: GeoJSON Conversion - Point
#[test]
fn test_geojson_point() {
    let point = create_test_point(116.4074, 39.9042);
    let geojson = point.to_geojson();

    match geojson {
        graphdb_core::core::value::geography::GeoJsonGeometry::Point { coordinates } => {
            assert_eq!(coordinates.len(), 2);
            assert_distance_within(coordinates[0], 116.4074, 1e-9);
            assert_distance_within(coordinates[1], 39.9042, 1e-9);
        }
        _ => panic!("Expected Point GeoJSON"),
    }
}

/// TC-GEO-FUNC-008: GeoJSON Conversion - LineString
#[test]
fn test_geojson_linestring() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0)]);
    let geojson = linestring.to_geojson();

    match geojson {
        graphdb_core::core::value::geography::GeoJsonGeometry::LineString { coordinates } => {
            assert_eq!(coordinates.len(), 2);
        }
        _ => panic!("Expected LineString GeoJSON"),
    }
}

/// TC-GEO-FUNC-009: Centroid Calculation
#[test]
fn test_centroid_calculation() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    let centroid = polygon.centroid().expect("Should have centroid");
    assert_distance_within(centroid.latitude, 1.0, 0.5);
    assert_distance_within(centroid.longitude, 1.0, 0.5);
}

/// TC-GEO-FUNC-010: Geometry Type
#[test]
fn test_geometry_type() {
    let point = create_test_point(0.0, 0.0);
    assert_eq!(point.geometry_type(), "Point");

    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0)]);
    assert_eq!(linestring.geometry_type(), "LineString");

    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    assert_eq!(polygon.geometry_type(), "Polygon");
}

/// TC-GEO-FUNC-011: Point Distance
#[test]
fn test_point_distance() {
    let point1 = GeographyValue::new(0.0, 0.0);
    let point2 = GeographyValue::new(1.0, 0.0);

    let distance = point1.distance(&point2);
    assert_distance_within(distance, 111.32, 1.0);
}

/// TC-GEO-FUNC-012: Point Bearing
#[test]
fn test_point_bearing() {
    let point1 = GeographyValue::new(0.0, 0.0);
    let point2 = GeographyValue::new(1.0, 0.0);

    let bearing = point1.bearing(&point2);
    assert_distance_within(bearing, 0.0, 5.0);
}

/// TC-GEO-FUNC-013: Polygon Area
#[test]
fn test_polygon_area() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    match polygon {
        Geography::Polygon(p) => {
            let area = p.area();
            assert!(area > 5000.0, "Area should be positive, got {}", area);
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-FUNC-014: LineString Length
#[test]
fn test_linestring_length() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (0.0, 1.0)]);

    match linestring {
        Geography::LineString(ls) => {
            let length = ls.length();
            assert_distance_within(length, 111.32, 1.0);
        }
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-FUNC-015: Polygon Perimeter
#[test]
fn test_polygon_perimeter() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    match polygon {
        Geography::Polygon(p) => {
            let perimeter = p.perimeter();
            assert_distance_within(perimeter, 4.0 * 111.32, 2.0);
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-FUNC-016: Point in Polygon
#[test]
fn test_point_in_polygon() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    match polygon {
        Geography::Polygon(p) => {
            let inside_point = GeographyValue::new(1.0, 1.0);
            assert!(p.contains_point(&inside_point));

            let outside_point = GeographyValue::new(3.0, 3.0);
            assert!(!p.contains_point(&outside_point));
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-FUNC-017: Bounding Box
#[test]
fn test_bounding_box() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (2.0, 2.0)]);
    let bbox = linestring.bounding_box().expect("Should have bounding box");

    assert_eq!(bbox, (0.0, 2.0, 0.0, 2.0));
}

/// TC-GEO-FUNC-018: Invalid WKT
#[test]
fn test_invalid_wkt() {
    let invalid_wkt = "INVALID(0 0)";
    let result = Geography::from_wkt(invalid_wkt);
    assert!(result.is_err(), "Invalid WKT should return error");
}

/// TC-GEO-FUNC-019: Empty WKT
#[test]
fn test_empty_wkt() {
    let empty_wkt = "";
    let result = Geography::from_wkt(empty_wkt);
    assert!(result.is_err(), "Empty WKT should return error");
}

/// TC-GEO-FUNC-020: Memory Estimation
#[test]
fn test_memory_estimation() {
    let point = create_test_point(0.0, 0.0);
    let point_size = point.estimated_size();
    assert!(point_size > 0, "Point should have non-zero memory size");

    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);
    let linestring_size = linestring.estimated_size();
    assert!(
        linestring_size > point_size,
        "LineString should have larger memory size than point"
    );
}
