pub mod constructors;
pub mod measurements;
pub mod predicates;
pub mod serialization;
pub mod transform;

use self::constructors::*;
use self::measurements::*;
use self::predicates::*;
use self::serialization::*;
use self::transform::*;

#[path = "test.rs"]
mod tests;

define_function_enum! {
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
