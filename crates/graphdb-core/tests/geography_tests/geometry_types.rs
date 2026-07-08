//! Geography Integration Tests - Geometry Types
//!
//! Test scope:
//! - Point operations - distance, bearing, bounding box check
//! - LineString operations - length, closed/ring check, centroid
//! - Polygon operations - area, perimeter, point containment
//! - MultiPoint operations - count, centroid, validity
//! - MultiLineString operations - count, length, validity
//! - MultiPolygon operations - count, area, point containment
//!
//! Test cases: TC-GEO-TYPE-001 ~ TC-GEO-TYPE-020

use super::common::{
    assert_distance_within, create_test_linestring, create_test_multilinestring,
    create_test_multipoint, create_test_multipolygon, create_test_point, create_test_polygon,
};
use graphdb_core::core::value::geography::{Geography, GeographyValue};

// ==================== Point Operations Tests ====================

/// TC-GEO-TYPE-001: Point Distance Calculation
#[test]
fn test_point_distance() {
    let point1 = create_test_point(0.0, 0.0);
    let point2 = create_test_point(0.0, 1.0);

    match (&point1, &point2) {
        (Geography::Point(p1), Geography::Point(p2)) => {
            let distance = p1.distance(p2);
            assert_distance_within(distance, 111.32, 1.0);
        }
        _ => panic!("Expected Point geometries"),
    }
}

/// TC-GEO-TYPE-002: Point Bearing Calculation
#[test]
fn test_point_bearing() {
    let point1 = create_test_point(0.0, 0.0);
    let point2 = create_test_point(0.0, 1.0);

    match (&point1, &point2) {
        (Geography::Point(p1), Geography::Point(p2)) => {
            let bearing = p1.bearing(p2);
            assert_distance_within(bearing, 0.0, 1.0);
        }
        _ => panic!("Expected Point geometries"),
    }
}

/// TC-GEO-TYPE-003: Point in Bounding Box
#[test]
fn test_point_in_bbox() {
    let point = create_test_point(0.5, 0.5);

    match point {
        Geography::Point(p) => {
            assert!(p.in_bbox(0.0, 1.0, 0.0, 1.0));
            assert!(!p.in_bbox(1.0, 2.0, 1.0, 2.0));
        }
        _ => panic!("Expected Point geometry"),
    }
}

// ==================== LineString Operations Tests ====================

/// TC-GEO-TYPE-004: LineString Length Calculation
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

/// TC-GEO-TYPE-005: LineString Closed Check
#[test]
fn test_linestring_closed() {
    let closed = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (0.0, 0.0)]);
    match closed {
        Geography::LineString(ls) => assert!(ls.is_closed()),
        _ => panic!("Expected LineString geometry"),
    }

    let open = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0)]);
    match open {
        Geography::LineString(ls) => assert!(!ls.is_closed()),
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-TYPE-006: LineString Ring Check
#[test]
fn test_linestring_ring() {
    let ring = create_test_linestring(vec![
        (0.0, 0.0),
        (0.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.0),
        (0.0, 0.0),
    ]);
    match ring {
        Geography::LineString(ls) => assert!(ls.is_ring()),
        _ => panic!("Expected LineString geometry"),
    }

    let not_ring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (0.0, 0.0)]);
    match not_ring {
        Geography::LineString(ls) => assert!(!ls.is_ring()),
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-TYPE-007: LineString Start/End Points
#[test]
fn test_linestring_start_end_points() {
    let linestring = create_test_linestring(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);

    match linestring {
        Geography::LineString(ls) => {
            let start = ls.start_point().expect("Should have start point");
            assert_eq!(start.longitude, 0.0);
            assert_eq!(start.latitude, 0.0);

            let end = ls.end_point().expect("Should have end point");
            assert_eq!(end.longitude, 2.0);
            assert_eq!(end.latitude, 2.0);
        }
        _ => panic!("Expected LineString geometry"),
    }
}

// ==================== Polygon Operations Tests ====================

/// TC-GEO-TYPE-008: Polygon Area Calculation
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

/// TC-GEO-TYPE-009: Polygon Perimeter Calculation
#[test]
fn test_polygon_perimeter() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );

    match polygon {
        Geography::Polygon(p) => {
            let perimeter = p.perimeter();
            assert_distance_within(perimeter, 4.0 * 111.32, 1.0);
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

/// TC-GEO-TYPE-010: Polygon Point Containment
#[test]
fn test_polygon_contains_point() {
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

/// TC-GEO-TYPE-011: Polygon with Hole
#[test]
fn test_polygon_with_hole() {
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 3.0), (3.0, 3.0), (3.0, 0.0), (0.0, 0.0)],
        vec![vec![
            (1.0, 1.0),
            (1.0, 2.0),
            (2.0, 2.0),
            (2.0, 1.0),
            (1.0, 1.0),
        ]],
    );

    match polygon {
        Geography::Polygon(p) => {
            assert_eq!(p.holes.len(), 1);

            let in_hole = GeographyValue::new(1.5, 1.5);
            assert!(
                !p.contains_point(&in_hole),
                "Point in hole should not be contained"
            );

            let in_polygon = GeographyValue::new(0.5, 0.5);
            assert!(
                p.contains_point(&in_polygon),
                "Point in polygon should be contained"
            );
        }
        _ => panic!("Expected Polygon geometry"),
    }
}

// ==================== MultiPoint Operations Tests ====================

/// TC-GEO-TYPE-012: MultiPoint Count
#[test]
fn test_multipoint_count() {
    let multipoint = create_test_multipoint(vec![(0.0, 0.0), (1.0, 1.0), (2.0, 2.0)]);

    match multipoint {
        Geography::MultiPoint(mp) => {
            assert_eq!(mp.num_points(), 3);
        }
        _ => panic!("Expected MultiPoint geometry"),
    }
}

/// TC-GEO-TYPE-013: MultiPoint Centroid
#[test]
fn test_multipoint_centroid() {
    let multipoint = create_test_multipoint(vec![(0.0, 0.0), (2.0, 2.0)]);

    match multipoint {
        Geography::MultiPoint(mp) => {
            let centroid = mp.centroid().expect("Should have centroid");
            assert_distance_within(centroid.latitude, 1.0, 1e-9);
            assert_distance_within(centroid.longitude, 1.0, 1e-9);
        }
        _ => panic!("Expected MultiPoint geometry"),
    }
}

// ==================== MultiLineString Operations Tests ====================

/// TC-GEO-TYPE-014: MultiLineString Count
#[test]
fn test_multilinestring_count() {
    let multilinestring = create_test_multilinestring(vec![
        vec![(0.0, 0.0), (1.0, 1.0)],
        vec![(2.0, 2.0), (3.0, 3.0)],
    ]);

    match multilinestring {
        Geography::MultiLineString(mls) => {
            assert_eq!(mls.num_linestrings(), 2);
        }
        _ => panic!("Expected MultiLineString geometry"),
    }
}

/// TC-GEO-TYPE-015: MultiLineString Length
#[test]
fn test_multilinestring_length() {
    let multilinestring = create_test_multilinestring(vec![
        vec![(0.0, 0.0), (0.0, 1.0)],
        vec![(0.0, 0.0), (0.0, 1.0)],
    ]);

    match multilinestring {
        Geography::MultiLineString(mls) => {
            let length = mls.length();
            assert_distance_within(length, 2.0 * 111.32, 2.0);
        }
        _ => panic!("Expected MultiLineString geometry"),
    }
}

// ==================== MultiPolygon Operations Tests ====================

/// TC-GEO-TYPE-016: MultiPolygon Count
#[test]
fn test_multipolygon_count() {
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
            assert_eq!(mp.num_polygons(), 2);
        }
        _ => panic!("Expected MultiPolygon geometry"),
    }
}

/// TC-GEO-TYPE-017: MultiPolygon Area
#[test]
fn test_multipolygon_area() {
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
            let area = mp.area();
            assert!(area > 10000.0, "Area should be positive, got {}", area);
        }
        _ => panic!("Expected MultiPolygon geometry"),
    }
}

/// TC-GEO-TYPE-018: MultiPolygon Point Containment
#[test]
fn test_multipolygon_contains_point() {
    let multipolygon = create_test_multipolygon(vec![
        (
            vec![(0.0, 0.0), (0.0, 2.0), (2.0, 2.0), (2.0, 0.0), (0.0, 0.0)],
            vec![],
        ),
        (
            vec![(5.0, 5.0), (5.0, 7.0), (7.0, 7.0), (7.0, 5.0), (5.0, 5.0)],
            vec![],
        ),
    ]);

    match multipolygon {
        Geography::MultiPolygon(mp) => {
            let point1 = GeographyValue::new(1.0, 1.0);
            assert!(
                mp.contains_point(&point1),
                "Point should be in first polygon"
            );

            let point2 = GeographyValue::new(6.0, 6.0);
            assert!(
                mp.contains_point(&point2),
                "Point should be in second polygon"
            );

            let point3 = GeographyValue::new(10.0, 10.0);
            assert!(
                !mp.contains_point(&point3),
                "Point should not be in any polygon"
            );
        }
        _ => panic!("Expected MultiPolygon geometry"),
    }
}

// ==================== Empty Geometry Tests ====================

/// TC-GEO-TYPE-019: Empty LineString Operations
#[test]
fn test_empty_linestring_operations() {
    let empty = create_test_linestring(vec![]);

    match empty {
        Geography::LineString(ls) => {
            assert_eq!(ls.points.len(), 0);
            assert_eq!(ls.length(), 0.0);
            assert!(!ls.is_closed());
            assert!(ls.centroid().is_none());
        }
        _ => panic!("Expected LineString geometry"),
    }
}

/// TC-GEO-TYPE-020: Empty MultiPoint Operations
#[test]
fn test_empty_multipoint_operations() {
    let empty = create_test_multipoint(vec![]);

    match empty {
        Geography::MultiPoint(mp) => {
            assert_eq!(mp.points.len(), 0);
            assert_eq!(mp.num_points(), 0);
            assert!(mp.centroid().is_none());
        }
        _ => panic!("Expected MultiPoint geometry"),
    }
}
