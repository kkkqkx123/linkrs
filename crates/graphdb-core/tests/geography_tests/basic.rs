//! Geography Integration Tests - Basic Operations
//!
//! Test scope:
//! - Create geometries - Point, LineString, Polygon, MultiPoint, MultiLineString, MultiPolygon
//! - Validate geometries - coordinate ranges, closed rings, valid structures
//! - Serialize geometries - to WKT format
//! - Parse geometries - from WKT format
//! - Memory estimation - estimated_size method
//!
//! Test cases: TC-GEO-001 ~ TC-GEO-015

use super::common::{
    assert_geography_equals, create_test_linestring, create_test_multilinestring,
    create_test_multipoint, create_test_multipolygon, create_test_point, create_test_polygon,
    get_edge_case_geometries, get_standard_test_geometries,
};
use graphdb_core::core::value::geography::Geography;

// ==================== Geometry Creation Tests ====================

/// TC-GEO-001: Create Point Geometry
#[test]
fn test_create_point() {
    let point = create_test_point(116.4074, 39.9042);

    match point {
        Geography::Point(p) => {
            assert_eq!(p.longitude, 116.4074);
            assert_eq!(p.latitude, 39.9042);
            assert!(p.is_valid());
        }
        _ => panic!("Expected Point geometry"),
    }
}

/// TC-GEO-002: Create LineString Geometry
#[test]
fn test_create_linestring() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);

    match linestring {
        Geography::LineString(ls) => {
            assert_eq!(ls.points.len(), 3);
            assert!(ls.is_valid());
        }
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-003: Create Polygon Geometry
#[test]
fn test_create_polygon() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    match polygon {
        Geography::Polygon(p) => {
            assert_eq!(p.exterior.points.len(), 5);
            assert!(p.exterior.is_closed());
            assert!(p.is_valid());
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-004: Create MultiPoint Geometry
#[test]
fn test_create_multipoint() {
    let multipoint = create_test_multipoint(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);

    match multipoint {
        Geography::MultiPoint(mp) => {
            assert_eq!(mp.points.len(), 3);
            assert!(mp.is_valid());
        }
        _ => panic!("Expected MultiPoint geometry"),
    }
}

/// TC-GEO-005: Create MultiLineString Geometry
#[test]
fn test_create_multilinestring() {
    let multilinestring = create_test_multilinestring(vec![
        vec![(0.0, 0.0), (1.0, 1.0)],
        vec![(2.0, 2.0), (3.0, 3.0)],
    ]);

    match multilinestring {
        Geography::MultiLineString(mls) => {
            assert_eq!(mls.linestrings.len(), 2);
            assert!(mls.is_valid());
        }
        _ => panic!("Expected MultiLineString geometry"),
    }
}

/// TC-GEO-006: Create MultiPolygon Geometry
#[test]
fn test_create_multipolygon() {
    let multipolygon = create_test_multipolygon(vec![
        (
            vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
            vec![],
        ),
        (
            vec![(2.0, 2.0), (2.0, 3.0), (3.0, 3.0), (3.0, 2.0), (2.0, 2.0)],
            vec![],
        ),
    ]);

    match multipolygon {
        Geography::MultiPolygon(mp) => {
            assert_eq!(mp.polygons.len(), 2);
            assert!(mp.is_valid());
        }
        _ => panic!("Expected MultiPolygon geometry"),
    }
}

// ==================== Geometry Validation Tests ====================

/// TC-GEO-007: Geometry Validation - Valid Point
#[test]
fn test_valid_point() {
    let valid_point = create_test_point(0.0, 0.0);
    assert!(valid_point.is_valid());

    let valid_point2 = create_test_point(180.0, 90.0);
    assert!(valid_point2.is_valid());

    let valid_point3 = create_test_point(-180.0, -90.0);
    assert!(valid_point3.is_valid());
}

/// TC-GEO-008: Geometry Validation - Invalid Point
#[test]
fn test_invalid_point() {
    let edge_cases = get_edge_case_geometries();

    let invalid_lat = edge_cases
        .get("invalid_point_lat")
        .expect("Should have invalid_point_lat");
    assert!(
        !invalid_lat.is_valid(),
        "Point with latitude > 90 should be invalid"
    );

    let invalid_lon = edge_cases
        .get("invalid_point_lon")
        .expect("Should have invalid_point_lon");
    assert!(
        !invalid_lon.is_valid(),
        "Point with longitude > 180 should be invalid"
    );
}

/// TC-GEO-009: Geometry Validation - Closed LineString
#[test]
fn test_closed_linestring() {
    let closed_linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (0.0, 0.0)]);

    match closed_linestring {
        Geography::LineString(ls) => {
            assert!(ls.is_closed());
            assert!(ls.is_valid());
        }
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-010: Geometry Validation - Open LineString
#[test]
fn test_open_linestring() {
    let open_linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);

    match open_linestring {
        Geography::LineString(ls) => {
            assert!(!ls.is_closed());
            assert!(ls.is_valid());
        }
        _ => panic!("Expected LineString geometry"),
    }
}

// ==================== WKT Format Tests ====================

/// TC-GEO-011: WKT Point Conversion
#[test]
fn test_wkt_point_conversion() {
    let point = create_test_point(116.4074, 39.9042);
    let wkt = point.to_wkt();

    assert!(wkt.contains("POINT"));
    assert!(wkt.contains("116.4074"));
    assert!(wkt.contains("39.9042"));

    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_geography_equals(&point, &parsed, 1e-9);
}

/// TC-GEO-012: WKT LineString Conversion
#[test]
fn test_wkt_linestring_conversion() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);
    let wkt = linestring.to_wkt();

    assert!(wkt.contains("LINESTRING"));

    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_eq!(linestring, parsed);
}

/// TC-GEO-013: WKT Polygon Conversion
#[test]
fn test_wkt_polygon_conversion() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    let wkt = polygon.to_wkt();

    assert!(wkt.contains("POLYGON"));

    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_eq!(polygon, parsed);
}

/// TC-GEO-014: WKT Parsing - Invalid Format
#[test]
fn test_wkt_invalid_format() {
    let invalid_wkt = "INVALID(0 0)";
    let result = Geography::from_wkt(invalid_wkt);

    assert!(result.is_err(), "Invalid WKT format should return error");
}

// ==================== Memory Estimation Tests ====================

/// TC-GEO-015: Memory Estimation
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

    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    let polygon_size = polygon.estimated_size();
    assert!(
        polygon_size > linestring_size,
        "Polygon should have larger memory size than linestring"
    );
}

// ==================== Standard Test Geometries Tests ====================

/// TC-GEO-016: Standard Test Geometries
#[test]
fn test_standard_geometries() {
    let geometries = get_standard_test_geometries();

    assert!(
        geometries.len() >= 10,
        "Should have at least 10 standard test geometries"
    );

    for (name, geo) in &geometries {
        assert!(geo.is_valid(), "Geometry '{}' should be valid", name);
    }
}

/// TC-GEO-017: Edge Case Geometries
#[test]
fn test_edge_case_geometries() {
    let edge_cases = get_edge_case_geometries();

    let empty_linestring = edge_cases
        .get("empty_linestring")
        .expect("Should have empty_linestring");
    match empty_linestring {
        Geography::LineString(ls) => {
            assert_eq!(ls.points.len(), 0);
        }
        _ => panic!("Expected LineString geometry"),
    }

    let empty_multipoint = edge_cases
        .get("empty_multipoint")
        .expect("Should have empty_multipoint");
    match empty_multipoint {
        Geography::MultiPoint(mp) => {
            assert_eq!(mp.points.len(), 0);
        }
        _ => panic!("Expected MultiPoint geometry"),
    }
}

/// TC-GEO-018: Geometry Type Name
#[test]
fn test_geometry_type_name() {
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

/// TC-GEO-019: Bounding Box
#[test]
fn test_bounding_box() {
    let point = create_test_point(0.0, 0.0);
    let bbox = point
        .bounding_box()
        .expect("Point should have bounding box");
    assert_eq!(bbox, (0.0, 0.0, 0.0, 0.0));

    let linestring = create_test_linestring(vec![(0.0, 0.0), (2.0, 2.0)]);
    let bbox = linestring
        .bounding_box()
        .expect("LineString should have bounding box");
    assert_eq!(bbox, (0.0, 2.0, 0.0, 2.0));

    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    let bbox = polygon
        .bounding_box()
        .expect("Polygon should have bounding box");
    assert_eq!(bbox, (0.0, 2.0, 0.0, 2.0));
}

/// TC-GEO-020: Centroid Calculation
#[test]
fn test_centroid() {
    let point = create_test_point(0.0, 0.0);
    let centroid = point.centroid().expect("Point should have centroid");
    assert_eq!(centroid.latitude, 0.0);
    assert_eq!(centroid.longitude, 0.0);

    let linestring = create_test_linestring(vec![(0.0, 0.0), (2.0, 2.0)]);
    let centroid = linestring
        .centroid()
        .expect("LineString should have centroid");
    assert_eq!(centroid.latitude, 1.0);
    assert_eq!(centroid.longitude, 1.0);
}
