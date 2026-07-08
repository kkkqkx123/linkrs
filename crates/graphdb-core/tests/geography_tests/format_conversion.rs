//! Geography Integration Tests - Format Conversion
//!
//! Test scope:
//! - WKT format - parse and serialize all geometry types
//! - GeoJSON format - parse and serialize all geometry types
//! - Round-trip conversion - WKT -> Geography -> WKT
//! - Error handling - invalid formats

use super::common::{create_test_linestring, create_test_point, create_test_polygon};
use graphdb_core::core::value::geography::{GeoJsonGeometry, Geography};

/// TC-GEO-FMT-001: WKT Point Round-trip
#[test]
fn test_wkt_point_roundtrip() {
    let point = create_test_point(116.4074, 39.9042);
    let wkt = point.to_wkt();
    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");

    match (point, parsed) {
        (Geography::Point(p1), Geography::Point(p2)) => {
            assert!((p1.longitude - p2.longitude).abs() < 1e-9);
            assert!((p1.latitude - p2.latitude).abs() < 1e-9);
        }
        _ => panic!("Geometries should match"),
    }
}

/// TC-GEO-FMT-002: WKT LineString Round-trip
#[test]
fn test_wkt_linestring_roundtrip() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);
    let wkt = linestring.to_wkt();
    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_eq!(linestring, parsed);
}

/// TC-GEO-FMT-003: WKT Polygon Round-trip
#[test]
fn test_wkt_polygon_roundtrip() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    let wkt = polygon.to_wkt();
    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_eq!(polygon, parsed);
}

/// TC-GEO-FMT-004: GeoJSON Point Conversion
#[test]
fn test_geojson_point_conversion() {
    let point = create_test_point(116.4074, 39.9042);
    let geojson = point.to_geojson();

    match geojson {
        GeoJsonGeometry::Point { coordinates } => {
            assert_eq!(coordinates.len(), 2);
            assert!((coordinates[0] - 116.4074).abs() < 1e-9);
            assert!((coordinates[1] - 39.9042).abs() < 1e-9);
        }
        _ => panic!("Expected Point GeoJSON"),
    }
}

/// TC-GEO-FMT-005: GeoJSON LineString Conversion
#[test]
fn test_geojson_linestring_conversion() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0)]);
    let geojson = linestring.to_geojson();

    match geojson {
        GeoJsonGeometry::LineString { coordinates } => {
            assert_eq!(coordinates.len(), 2);
        }
        _ => panic!("Expected LineString GeoJSON"),
    }
}

/// TC-GEO-FMT-006: GeoJSON Polygon Conversion
#[test]
fn test_geojson_polygon_conversion() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    let geojson = polygon.to_geojson();

    match geojson {
        GeoJsonGeometry::Polygon { coordinates } => {
            assert_eq!(coordinates.len(), 1);
            assert_eq!(coordinates[0].len(), 5);
        }
        _ => panic!("Expected Polygon GeoJSON"),
    }
}

/// TC-GEO-FMT-007: Invalid WKT Format
#[test]
fn test_invalid_wkt_format() {
    let invalid_wkt = "INVALID(0 0)";
    let result = Geography::from_wkt(invalid_wkt);
    assert!(result.is_err(), "Invalid WKT should return error");
}

/// TC-GEO-FMT-008: Empty WKT
#[test]
fn test_empty_wkt() {
    let empty_wkt = "";
    let result = Geography::from_wkt(empty_wkt);
    assert!(result.is_err(), "Empty WKT should return error");
}
