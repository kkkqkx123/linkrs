//! Geospatial Types Module
//!
//! This module defines geographic spatial types and related operations.
//! Supports Point, LineString, Polygon, MultiPoint, MultiLineString, and MultiPolygon.

use regex::Regex;
use serde::{Deserialize, Serialize};

/// Geographic Point type (single coordinate pair)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, PartialOrd)]
pub struct GeographyValue {
    pub latitude: f64,
    pub longitude: f64,
}

impl std::hash::Hash for GeographyValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.latitude.to_bits().hash(state);
        self.longitude.to_bits().hash(state);
    }
}

impl GeographyValue {
    pub fn new(latitude: f64, longitude: f64) -> Self {
        GeographyValue {
            latitude,
            longitude,
        }
    }

    /// Calculate the Haversine distance between two points (unit: kilometers)
    pub fn distance(&self, other: &GeographyValue) -> f64 {
        const EARTH_RADIUS_KM: f64 = 6371.0;
        const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

        let lat1 = self.latitude * DEG_TO_RAD;
        let lat2 = other.latitude * DEG_TO_RAD;
        let delta_lat = (other.latitude - self.latitude) * DEG_TO_RAD;
        let delta_lon = (other.longitude - self.longitude) * DEG_TO_RAD;

        let a = (delta_lat / 2.0).sin().powi(2)
            + lat1.cos() * lat2.cos() * (delta_lon / 2.0).sin().powi(2);
        let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());

        EARTH_RADIUS_KM * c
    }

    /// Calculate the azimuth angle between two points (unit: degrees)
    pub fn bearing(&self, other: &GeographyValue) -> f64 {
        const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;
        const RAD_TO_DEG: f64 = 180.0 / std::f64::consts::PI;

        let lat1 = self.latitude * DEG_TO_RAD;
        let lat2 = other.latitude * DEG_TO_RAD;
        let delta_lon = (other.longitude - self.longitude) * DEG_TO_RAD;

        let y = delta_lon.sin() * lat2.cos();
        let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * delta_lon.cos();

        let bearing = y.atan2(x) * RAD_TO_DEG;
        (bearing + 360.0) % 360.0
    }

    /// Check whether the checkpoint is within the specified rectangular area.
    pub fn in_bbox(&self, min_lat: f64, max_lat: f64, min_lon: f64, max_lon: f64) -> bool {
        self.latitude >= min_lat
            && self.latitude <= max_lat
            && self.longitude >= min_lon
            && self.longitude <= max_lon
    }

    /// Check if the point is valid (within coordinate ranges)
    pub fn is_valid(&self) -> bool {
        self.latitude >= -90.0
            && self.latitude <= 90.0
            && self.longitude >= -180.0
            && self.longitude <= 180.0
    }
}

impl Default for GeographyValue {
    fn default() -> Self {
        GeographyValue {
            latitude: 0.0,
            longitude: 0.0,
        }
    }
}

impl GeographyValue {
    /// Estimate the memory usage of the geography value
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

/// LineString type (ordered sequence of points)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct LineStringValue {
    pub points: Vec<GeographyValue>,
}

impl LineStringValue {
    pub fn new(points: Vec<GeographyValue>) -> Self {
        LineStringValue { points }
    }

    /// Calculate total length in kilometers
    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|w| w[0].distance(&w[1])).sum()
    }

    /// Get the number of points
    pub fn num_points(&self) -> usize {
        self.points.len()
    }

    /// Check if the linestring is closed (first point equals last)
    pub fn is_closed(&self) -> bool {
        if self.points.len() < 2 {
            return false;
        }
        let first = &self.points[0];
        let last = &self.points[self.points.len() - 1];
        (first.latitude - last.latitude).abs() < 1e-9
            && (first.longitude - last.longitude).abs() < 1e-9
    }

    /// Check if the linestring is a ring (closed and has at least 4 points)
    pub fn is_ring(&self) -> bool {
        self.points.len() >= 4 && self.is_closed()
    }

    /// Calculate centroid (average of all points)
    pub fn centroid(&self) -> Option<GeographyValue> {
        if self.points.is_empty() {
            return None;
        }
        let (sum_lat, sum_lon) = self.points.iter().fold((0.0, 0.0), |(lat, lon), p| {
            (lat + p.latitude, lon + p.longitude)
        });
        Some(GeographyValue::new(
            sum_lat / self.points.len() as f64,
            sum_lon / self.points.len() as f64,
        ))
    }

    /// Get start point
    pub fn start_point(&self) -> Option<&GeographyValue> {
        self.points.first()
    }

    /// Get end point
    pub fn end_point(&self) -> Option<&GeographyValue> {
        self.points.last()
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.points.len() * std::mem::size_of::<GeographyValue>()
    }

    /// Check if all points are valid
    pub fn is_valid(&self) -> bool {
        self.points.iter().all(|p| p.is_valid())
    }
}

/// Polygon type (closed ring with optional holes)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct PolygonValue {
    pub exterior: LineStringValue,
    pub holes: Vec<LineStringValue>,
}

impl PolygonValue {
    pub fn new(exterior: LineStringValue, holes: Vec<LineStringValue>) -> Self {
        PolygonValue { exterior, holes }
    }

    pub fn from_ring(ring: LineStringValue) -> Self {
        PolygonValue {
            exterior: ring,
            holes: Vec::new(),
        }
    }

    /// Calculate area in square kilometers (approximate using spherical excess)
    pub fn area(&self) -> f64 {
        let exterior_area = self.ring_area(&self.exterior);
        let holes_area: f64 = self.holes.iter().map(|h| self.ring_area(h)).sum();
        (exterior_area - holes_area).abs()
    }

    /// Calculate ring area using spherical excess formula
    fn ring_area(&self, ring: &LineStringValue) -> f64 {
        if ring.points.len() < 4 {
            return 0.0;
        }

        const EARTH_RADIUS_KM: f64 = 6371.0;
        const DEG_TO_RAD: f64 = std::f64::consts::PI / 180.0;

        let mut sum = 0.0;
        let n = ring.points.len() - 1; // Exclude the closing point

        for i in 0..n {
            let j = (i + 1) % n;
            let lat_i = ring.points[i].latitude * DEG_TO_RAD;
            let lon_i = ring.points[i].longitude * DEG_TO_RAD;
            let lon_j = ring.points[j].longitude * DEG_TO_RAD;

            sum += (lon_j - lon_i) * lat_i.sin();
        }

        (sum.abs() * EARTH_RADIUS_KM * EARTH_RADIUS_KM) / 2.0
    }

    /// Calculate perimeter in kilometers
    pub fn perimeter(&self) -> f64 {
        let exterior_perimeter = self.exterior.length();
        let holes_perimeter: f64 = self.holes.iter().map(|h| h.length()).sum();
        exterior_perimeter + holes_perimeter
    }

    /// Check if a point is inside the polygon using ray casting algorithm
    pub fn contains_point(&self, point: &GeographyValue) -> bool {
        self.point_in_ring(point, &self.exterior)
            && !self.holes.iter().any(|h| self.point_in_ring(point, h))
    }

    /// Ray casting algorithm for point-in-ring test
    fn point_in_ring(&self, point: &GeographyValue, ring: &LineStringValue) -> bool {
        if ring.points.len() < 4 {
            return false;
        }

        let mut inside = false;
        let n = ring.points.len() - 1;

        for i in 0..n {
            let j = (i + 1) % n;
            let pi = &ring.points[i];
            let pj = &ring.points[j];

            if ((pi.latitude > point.latitude) != (pj.latitude > point.latitude))
                && (point.longitude
                    < (pj.longitude - pi.longitude) * (point.latitude - pi.latitude)
                        / (pj.latitude - pi.latitude)
                        + pi.longitude)
            {
                inside = !inside;
            }
        }

        inside
    }

    /// Calculate centroid
    pub fn centroid(&self) -> Option<GeographyValue> {
        self.exterior.centroid()
    }

    /// Get bounding box as (min_lat, max_lat, min_lon, max_lon)
    pub fn bounding_box(&self) -> Option<(f64, f64, f64, f64)> {
        if self.exterior.points.is_empty() {
            return None;
        }

        let mut min_lat = f64::MAX;
        let mut max_lat = f64::MIN;
        let mut min_lon = f64::MAX;
        let mut max_lon = f64::MIN;

        for p in &self.exterior.points {
            min_lat = min_lat.min(p.latitude);
            max_lat = max_lat.max(p.latitude);
            min_lon = min_lon.min(p.longitude);
            max_lon = max_lon.max(p.longitude);
        }

        Some((min_lat, max_lat, min_lon, max_lon))
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        let mut size = std::mem::size_of::<Self>();
        size += self.exterior.estimated_size();
        for hole in &self.holes {
            size += hole.estimated_size();
        }
        size
    }

    /// Check if polygon is valid
    pub fn is_valid(&self) -> bool {
        self.exterior.is_valid()
            && self.exterior.is_closed()
            && self.holes.iter().all(|h| h.is_valid() && h.is_closed())
    }
}

/// MultiPoint type (collection of points)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MultiPointValue {
    pub points: Vec<GeographyValue>,
}

impl MultiPointValue {
    pub fn new(points: Vec<GeographyValue>) -> Self {
        MultiPointValue { points }
    }

    /// Get the number of points
    pub fn num_points(&self) -> usize {
        self.points.len()
    }

    /// Calculate centroid
    pub fn centroid(&self) -> Option<GeographyValue> {
        if self.points.is_empty() {
            return None;
        }
        let (sum_lat, sum_lon) = self.points.iter().fold((0.0, 0.0), |(lat, lon), p| {
            (lat + p.latitude, lon + p.longitude)
        });
        Some(GeographyValue::new(
            sum_lat / self.points.len() as f64,
            sum_lon / self.points.len() as f64,
        ))
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>() + self.points.len() * std::mem::size_of::<GeographyValue>()
    }

    /// Check if all points are valid
    pub fn is_valid(&self) -> bool {
        self.points.iter().all(|p| p.is_valid())
    }
}

/// MultiLineString type (collection of linestrings)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MultiLineStringValue {
    pub linestrings: Vec<LineStringValue>,
}

impl MultiLineStringValue {
    pub fn new(linestrings: Vec<LineStringValue>) -> Self {
        MultiLineStringValue { linestrings }
    }

    /// Get the number of linestrings
    pub fn num_linestrings(&self) -> usize {
        self.linestrings.len()
    }

    /// Calculate total length
    pub fn length(&self) -> f64 {
        self.linestrings.iter().map(|ls| ls.length()).sum()
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .linestrings
                .iter()
                .map(|ls| ls.estimated_size())
                .sum::<usize>()
    }

    /// Check if all linestrings are valid
    pub fn is_valid(&self) -> bool {
        self.linestrings.iter().all(|ls| ls.is_valid())
    }
}

/// MultiPolygon type (collection of polygons)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct MultiPolygonValue {
    pub polygons: Vec<PolygonValue>,
}

impl MultiPolygonValue {
    pub fn new(polygons: Vec<PolygonValue>) -> Self {
        MultiPolygonValue { polygons }
    }

    /// Get the number of polygons
    pub fn num_polygons(&self) -> usize {
        self.polygons.len()
    }

    /// Calculate total area
    pub fn area(&self) -> f64 {
        self.polygons.iter().map(|p| p.area()).sum()
    }

    /// Check if a point is inside any polygon
    pub fn contains_point(&self, point: &GeographyValue) -> bool {
        self.polygons.iter().any(|p| p.contains_point(point))
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .polygons
                .iter()
                .map(|p| p.estimated_size())
                .sum::<usize>()
    }

    /// Check if all polygons are valid
    pub fn is_valid(&self) -> bool {
        self.polygons.iter().all(|p| p.is_valid())
    }
}

/// Geographic type enumeration
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Geography {
    Point(GeographyValue),
    LineString(LineStringValue),
    Polygon(PolygonValue),
    MultiPoint(MultiPointValue),
    MultiLineString(MultiLineStringValue),
    MultiPolygon(MultiPolygonValue),
}

impl std::hash::Hash for Geography {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Geography::Point(p) => {
                0u8.hash(state);
                p.hash(state);
            }
            Geography::LineString(ls) => {
                1u8.hash(state);
                ls.points.hash(state);
            }
            Geography::Polygon(p) => {
                2u8.hash(state);
                p.exterior.points.hash(state);
                p.holes.len().hash(state);
            }
            Geography::MultiPoint(mp) => {
                3u8.hash(state);
                mp.points.hash(state);
            }
            Geography::MultiLineString(mls) => {
                4u8.hash(state);
                mls.linestrings.len().hash(state);
                for ls in &mls.linestrings {
                    ls.points.hash(state);
                }
            }
            Geography::MultiPolygon(mp) => {
                5u8.hash(state);
                mp.polygons.len().hash(state);
                for p in &mp.polygons {
                    p.exterior.points.hash(state);
                }
            }
        }
    }
}

impl Geography {
    /// Parse geographic data from the WKT format
    pub fn from_wkt(wkt: &str) -> Result<Self, String> {
        let wkt_upper = wkt.trim().to_uppercase();

        if wkt_upper.starts_with("POINT") {
            Self::parse_point_wkt(wkt.trim())
        } else if wkt_upper.starts_with("LINESTRING") {
            Self::parse_linestring_wkt(wkt.trim())
        } else if wkt_upper.starts_with("POLYGON") {
            Self::parse_polygon_wkt(wkt.trim())
        } else if wkt_upper.starts_with("MULTIPOINT") {
            Self::parse_multipoint_wkt(wkt.trim())
        } else if wkt_upper.starts_with("MULTILINESTRING") {
            Self::parse_multilinestring_wkt(wkt.trim())
        } else if wkt_upper.starts_with("MULTIPOLYGON") {
            Self::parse_multipolygon_wkt(wkt.trim())
        } else {
            Err(format!("Unsupported WKT format: {}", wkt))
        }
    }

    /// Convert to WKT format
    pub fn to_wkt(&self) -> String {
        match self {
            Geography::Point(p) => format!("POINT({} {})", p.longitude, p.latitude),
            Geography::LineString(ls) => {
                let coords: Vec<String> = ls
                    .points
                    .iter()
                    .map(|p| format!("{} {}", p.longitude, p.latitude))
                    .collect();
                format!("LINESTRING({})", coords.join(", "))
            }
            Geography::Polygon(p) => {
                let mut rings: Vec<String> = Vec::new();
                let exterior_coords: Vec<String> = p
                    .exterior
                    .points
                    .iter()
                    .map(|pt| format!("{} {}", pt.longitude, pt.latitude))
                    .collect();
                rings.push(format!("({})", exterior_coords.join(", ")));
                for hole in &p.holes {
                    let hole_coords: Vec<String> = hole
                        .points
                        .iter()
                        .map(|pt| format!("{} {}", pt.longitude, pt.latitude))
                        .collect();
                    rings.push(format!("({})", hole_coords.join(", ")));
                }
                format!("POLYGON({})", rings.join(", "))
            }
            Geography::MultiPoint(mp) => {
                let coords: Vec<String> = mp
                    .points
                    .iter()
                    .map(|p| format!("({} {})", p.longitude, p.latitude))
                    .collect();
                format!("MULTIPOINT({})", coords.join(", "))
            }
            Geography::MultiLineString(mls) => {
                let linestrings: Vec<String> = mls
                    .linestrings
                    .iter()
                    .map(|ls| {
                        let coords: Vec<String> = ls
                            .points
                            .iter()
                            .map(|p| format!("{} {}", p.longitude, p.latitude))
                            .collect();
                        format!("({})", coords.join(", "))
                    })
                    .collect();
                format!("MULTILINESTRING({})", linestrings.join(", "))
            }
            Geography::MultiPolygon(mp) => {
                let polygons: Vec<String> = mp
                    .polygons
                    .iter()
                    .map(|p| {
                        let mut rings: Vec<String> = Vec::new();
                        let exterior_coords: Vec<String> = p
                            .exterior
                            .points
                            .iter()
                            .map(|pt| format!("{} {}", pt.longitude, pt.latitude))
                            .collect();
                        rings.push(format!("({})", exterior_coords.join(", ")));
                        for hole in &p.holes {
                            let hole_coords: Vec<String> = hole
                                .points
                                .iter()
                                .map(|pt| format!("{} {}", pt.longitude, pt.latitude))
                                .collect();
                            rings.push(format!("({})", hole_coords.join(", ")));
                        }
                        format!("({})", rings.join(", "))
                    })
                    .collect();
                format!("MULTIPOLYGON({})", polygons.join(", "))
            }
        }
    }

    fn parse_point_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)POINT\s*\(\s*([-\d.]+)\s+([-\d.]+)\s*\)")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let lon = caps
                .get(1)
                .ok_or("Missing longitude coordinate")?
                .as_str()
                .parse::<f64>()
                .map_err(|_| "Invalid longitude format")?;
            let lat = caps
                .get(2)
                .ok_or("Missing latitude coordinate")?
                .as_str()
                .parse::<f64>()
                .map_err(|_| "Invalid latitude format")?;
            return Ok(Geography::Point(GeographyValue::new(lat, lon)));
        }

        Err("Invalid POINT WKT format".to_string())
    }

    fn parse_linestring_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)LINESTRING\s*\(([^)]+)\)")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let coords_str = caps.get(1).ok_or("Missing coordinates")?.as_str();
            let points = Self::parse_coordinate_pairs(coords_str)?;
            if points.len() < 2 {
                return Err("LineString must have at least 2 points".to_string());
            }
            return Ok(Geography::LineString(LineStringValue::new(points)));
        }

        Err("Invalid LINESTRING WKT format".to_string())
    }

    fn parse_polygon_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)POLYGON\s*\((.+)\)\s*$")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let content = caps.get(1).ok_or("Missing polygon content")?.as_str();
            let rings = Self::parse_rings(content)?;

            if rings.is_empty() {
                return Err("Polygon must have at least one ring".to_string());
            }

            let exterior = rings.into_iter().next().unwrap();
            return Ok(Geography::Polygon(PolygonValue::from_ring(exterior)));
        }

        Err("Invalid POLYGON WKT format".to_string())
    }

    fn parse_multipoint_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)MULTIPOINT\s*\(([^)]*)\)")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let coords_str = caps.get(1).ok_or("Missing coordinates")?.as_str();
            if coords_str.trim().is_empty() {
                return Ok(Geography::MultiPoint(MultiPointValue::default()));
            }
            let points = Self::parse_coordinate_pairs(coords_str)?;
            return Ok(Geography::MultiPoint(MultiPointValue::new(points)));
        }

        Err("Invalid MULTIPOINT WKT format".to_string())
    }

    fn parse_multilinestring_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)MULTILINESTRING\s*\((.+)\)\s*$")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let content = caps
                .get(1)
                .ok_or("Missing multilinestring content")?
                .as_str();
            let rings = Self::parse_rings(content)?;

            let linestrings: Vec<LineStringValue> =
                rings.into_iter().filter(|r| r.points.len() >= 2).collect();

            return Ok(Geography::MultiLineString(MultiLineStringValue::new(
                linestrings,
            )));
        }

        Err("Invalid MULTILINESTRING WKT format".to_string())
    }

    fn parse_multipolygon_wkt(wkt: &str) -> Result<Self, String> {
        let re = Regex::new(r"(?i)MULTIPOLYGON\s*\((.+)\)\s*$")
            .map_err(|_| "Invalid regular expression".to_string())?;

        if let Some(caps) = re.captures(wkt) {
            let content = caps.get(1).ok_or("Missing multipolygon content")?.as_str();
            let polygons = Self::parse_multipolygon_content(content)?;

            if polygons.is_empty() {
                return Ok(Geography::MultiPolygon(MultiPolygonValue::default()));
            }

            return Ok(Geography::MultiPolygon(MultiPolygonValue::new(polygons)));
        }

        Err("Invalid MULTIPOLYGON WKT format".to_string())
    }

    fn parse_coordinate_pairs(s: &str) -> Result<Vec<GeographyValue>, String> {
        let mut points = Vec::new();
        let pairs: Vec<&str> = s.split(',').collect();

        for pair in pairs {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }

            let coords: Vec<&str> = pair.split_whitespace().collect();
            if coords.len() < 2 {
                return Err(format!("Invalid coordinate pair: {}", pair));
            }

            let lon = coords[0]
                .parse::<f64>()
                .map_err(|_| format!("Invalid longitude: {}", coords[0]))?;
            let lat = coords[1]
                .parse::<f64>()
                .map_err(|_| format!("Invalid latitude: {}", coords[1]))?;

            points.push(GeographyValue::new(lat, lon));
        }

        Ok(points)
    }

    fn parse_rings(s: &str) -> Result<Vec<LineStringValue>, String> {
        let mut rings = Vec::new();
        let mut depth = 0;
        let mut current = String::new();

        for c in s.chars() {
            match c {
                '(' => {
                    depth += 1;
                    if depth > 1 {
                        current.push(c);
                    }
                }
                ')' => {
                    depth -= 1;
                    if depth > 0 {
                        current.push(c);
                    } else if !current.trim().is_empty() {
                        let points = Self::parse_coordinate_pairs(&current)?;
                        if !points.is_empty() {
                            rings.push(LineStringValue::new(points));
                        }
                        current.clear();
                    }
                }
                _ => {
                    if depth > 0 {
                        current.push(c);
                    }
                }
            }
        }

        Ok(rings)
    }

    fn parse_multipolygon_content(s: &str) -> Result<Vec<PolygonValue>, String> {
        let mut polygons = Vec::new();
        let mut depth = 0;
        let mut polygon_content = String::new();
        let mut in_polygon = false;

        for c in s.chars() {
            match c {
                '(' => {
                    depth += 1;
                    if depth == 2 {
                        in_polygon = true;
                    }
                    if in_polygon {
                        polygon_content.push(c);
                    }
                }
                ')' => {
                    if in_polygon {
                        polygon_content.push(c);
                    }
                    depth -= 1;
                    if depth == 1 && in_polygon {
                        let rings = Self::parse_rings(&polygon_content)?;
                        if !rings.is_empty() {
                            let exterior = rings.into_iter().next().unwrap();
                            polygons.push(PolygonValue::from_ring(exterior));
                        }
                        polygon_content.clear();
                        in_polygon = false;
                    }
                }
                _ => {
                    if in_polygon {
                        polygon_content.push(c);
                    }
                }
            }
        }

        Ok(polygons)
    }

    /// Get geometry type name
    pub fn geometry_type(&self) -> &'static str {
        match self {
            Geography::Point(_) => "Point",
            Geography::LineString(_) => "LineString",
            Geography::Polygon(_) => "Polygon",
            Geography::MultiPoint(_) => "MultiPoint",
            Geography::MultiLineString(_) => "MultiLineString",
            Geography::MultiPolygon(_) => "MultiPolygon",
        }
    }

    /// Calculate centroid of the geometry
    pub fn centroid(&self) -> Option<GeographyValue> {
        match self {
            Geography::Point(p) => Some(p.clone()),
            Geography::LineString(ls) => ls.centroid(),
            Geography::Polygon(p) => p.centroid(),
            Geography::MultiPoint(mp) => mp.centroid(),
            Geography::MultiLineString(mls) => {
                let all_points: Vec<&GeographyValue> =
                    mls.linestrings.iter().flat_map(|ls| &ls.points).collect();
                if all_points.is_empty() {
                    return None;
                }
                let (sum_lat, sum_lon) = all_points.iter().fold((0.0, 0.0), |(lat, lon), p| {
                    (lat + p.latitude, lon + p.longitude)
                });
                Some(GeographyValue::new(
                    sum_lat / all_points.len() as f64,
                    sum_lon / all_points.len() as f64,
                ))
            }
            Geography::MultiPolygon(mp) => {
                let centroids: Vec<GeographyValue> =
                    mp.polygons.iter().filter_map(|p| p.centroid()).collect();
                if centroids.is_empty() {
                    return None;
                }
                let (sum_lat, sum_lon) = centroids.iter().fold((0.0, 0.0), |(lat, lon), p| {
                    (lat + p.latitude, lon + p.longitude)
                });
                Some(GeographyValue::new(
                    sum_lat / centroids.len() as f64,
                    sum_lon / centroids.len() as f64,
                ))
            }
        }
    }

    /// Check if geometry is valid
    pub fn is_valid(&self) -> bool {
        match self {
            Geography::Point(p) => p.is_valid(),
            Geography::LineString(ls) => ls.is_valid(),
            Geography::Polygon(p) => p.is_valid(),
            Geography::MultiPoint(mp) => mp.is_valid(),
            Geography::MultiLineString(mls) => mls.is_valid(),
            Geography::MultiPolygon(mp) => mp.is_valid(),
        }
    }

    /// Estimate memory usage
    pub fn estimated_size(&self) -> usize {
        match self {
            Geography::Point(p) => p.estimated_size(),
            Geography::LineString(ls) => ls.estimated_size(),
            Geography::Polygon(p) => p.estimated_size(),
            Geography::MultiPoint(mp) => mp.estimated_size(),
            Geography::MultiLineString(mls) => mls.estimated_size(),
            Geography::MultiPolygon(mp) => mp.estimated_size(),
        }
    }

    /// Get bounding box as (min_lat, max_lat, min_lon, max_lon)
    pub fn bounding_box(&self) -> Option<(f64, f64, f64, f64)> {
        match self {
            Geography::Point(p) => Some((p.latitude, p.latitude, p.longitude, p.longitude)),
            Geography::LineString(ls) => {
                if ls.points.is_empty() {
                    return None;
                }
                let mut min_lat = f64::MAX;
                let mut max_lat = f64::MIN;
                let mut min_lon = f64::MAX;
                let mut max_lon = f64::MIN;
                for p in &ls.points {
                    min_lat = min_lat.min(p.latitude);
                    max_lat = max_lat.max(p.latitude);
                    min_lon = min_lon.min(p.longitude);
                    max_lon = max_lon.max(p.longitude);
                }
                Some((min_lat, max_lat, min_lon, max_lon))
            }
            Geography::Polygon(p) => p.bounding_box(),
            Geography::MultiPoint(mp) => {
                if mp.points.is_empty() {
                    return None;
                }
                let mut min_lat = f64::MAX;
                let mut max_lat = f64::MIN;
                let mut min_lon = f64::MAX;
                let mut max_lon = f64::MIN;
                for p in &mp.points {
                    min_lat = min_lat.min(p.latitude);
                    max_lat = max_lat.max(p.latitude);
                    min_lon = min_lon.min(p.longitude);
                    max_lon = max_lon.max(p.longitude);
                }
                Some((min_lat, max_lat, min_lon, max_lon))
            }
            Geography::MultiLineString(mls) => {
                let all_points: Vec<&GeographyValue> =
                    mls.linestrings.iter().flat_map(|ls| &ls.points).collect();
                if all_points.is_empty() {
                    return None;
                }
                let mut min_lat = f64::MAX;
                let mut max_lat = f64::MIN;
                let mut min_lon = f64::MAX;
                let mut max_lon = f64::MIN;
                for p in all_points {
                    min_lat = min_lat.min(p.latitude);
                    max_lat = max_lat.max(p.latitude);
                    min_lon = min_lon.min(p.longitude);
                    max_lon = max_lon.max(p.longitude);
                }
                Some((min_lat, max_lat, min_lon, max_lon))
            }
            Geography::MultiPolygon(mp) => {
                let all_points: Vec<&GeographyValue> = mp
                    .polygons
                    .iter()
                    .flat_map(|p| p.exterior.points.iter())
                    .collect();
                if all_points.is_empty() {
                    return None;
                }
                let mut min_lat = f64::MAX;
                let mut max_lat = f64::MIN;
                let mut min_lon = f64::MAX;
                let mut max_lon = f64::MIN;
                for p in all_points {
                    min_lat = min_lat.min(p.latitude);
                    max_lat = max_lat.max(p.latitude);
                    min_lon = min_lon.min(p.longitude);
                    max_lon = max_lon.max(p.longitude);
                }
                Some((min_lat, max_lat, min_lon, max_lon))
            }
        }
    }
}

impl std::fmt::Display for Geography {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_wkt())
    }
}

// ============================================================================
// GeoJSON Support
// ============================================================================

/// GeoJSON Geometry Object
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum GeoJsonGeometry {
    Point {
        coordinates: Vec<f64>,
    },
    LineString {
        coordinates: Vec<Vec<f64>>,
    },
    Polygon {
        coordinates: Vec<Vec<Vec<f64>>>,
    },
    MultiPoint {
        coordinates: Vec<Vec<f64>>,
    },
    MultiLineString {
        coordinates: Vec<Vec<Vec<f64>>>,
    },
    MultiPolygon {
        coordinates: Vec<Vec<Vec<Vec<f64>>>>,
    },
}

/// GeoJSON Feature Object
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeoJsonFeature {
    #[serde(rename = "type")]
    pub type_: String,
    pub geometry: Option<GeoJsonGeometry>,
    pub properties: serde_json::Map<String, serde_json::Value>,
}

/// GeoJSON FeatureCollection Object
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GeoJsonFeatureCollection {
    #[serde(rename = "type")]
    pub type_: String,
    pub features: Vec<GeoJsonFeature>,
}

impl Geography {
    /// Convert to GeoJSON Geometry object
    pub fn to_geojson(&self) -> GeoJsonGeometry {
        match self {
            Geography::Point(p) => GeoJsonGeometry::Point {
                coordinates: vec![p.longitude, p.latitude],
            },
            Geography::LineString(ls) => GeoJsonGeometry::LineString {
                coordinates: ls
                    .points
                    .iter()
                    .map(|p| vec![p.longitude, p.latitude])
                    .collect(),
            },
            Geography::Polygon(poly) => GeoJsonGeometry::Polygon {
                coordinates: {
                    let mut rings = Vec::new();
                    rings.push(
                        poly.exterior
                            .points
                            .iter()
                            .map(|p| vec![p.longitude, p.latitude])
                            .collect(),
                    );
                    for hole in &poly.holes {
                        rings.push(
                            hole.points
                                .iter()
                                .map(|p| vec![p.longitude, p.latitude])
                                .collect(),
                        );
                    }
                    rings
                },
            },
            Geography::MultiPoint(mp) => GeoJsonGeometry::MultiPoint {
                coordinates: mp
                    .points
                    .iter()
                    .map(|p| vec![p.longitude, p.latitude])
                    .collect(),
            },
            Geography::MultiLineString(mls) => GeoJsonGeometry::MultiLineString {
                coordinates: mls
                    .linestrings
                    .iter()
                    .map(|ls| {
                        ls.points
                            .iter()
                            .map(|p| vec![p.longitude, p.latitude])
                            .collect()
                    })
                    .collect(),
            },
            Geography::MultiPolygon(mp) => GeoJsonGeometry::MultiPolygon {
                coordinates: mp
                    .polygons
                    .iter()
                    .map(|poly| {
                        let mut rings: Vec<Vec<Vec<f64>>> = Vec::new();
                        rings.push(
                            poly.exterior
                                .points
                                .iter()
                                .map(|p| vec![p.longitude, p.latitude])
                                .collect(),
                        );
                        for hole in &poly.holes {
                            rings.push(
                                hole.points
                                    .iter()
                                    .map(|p| vec![p.longitude, p.latitude])
                                    .collect(),
                            );
                        }
                        rings
                    })
                    .collect(),
            },
        }
    }

    /// Parse from GeoJSON Geometry object
    pub fn from_geojson(geojson: &GeoJsonGeometry) -> Result<Self, String> {
        match geojson {
            GeoJsonGeometry::Point { coordinates } => {
                if coordinates.len() < 2 {
                    return Err("Point must have at least 2 coordinates".to_string());
                }
                Ok(Geography::Point(GeographyValue::new(
                    coordinates[1],
                    coordinates[0],
                )))
            }
            GeoJsonGeometry::LineString { coordinates } => {
                let points: Result<Vec<_>, _> = coordinates
                    .iter()
                    .map(|c| {
                        if c.len() < 2 {
                            Err("LineString coordinate must have at least 2 values".to_string())
                        } else {
                            Ok(GeographyValue::new(c[1], c[0]))
                        }
                    })
                    .collect();
                Ok(Geography::LineString(LineStringValue::new(points?)))
            }
            GeoJsonGeometry::Polygon { coordinates } => {
                if coordinates.is_empty() {
                    return Err("Polygon must have at least one ring".to_string());
                }
                let exterior = parse_ring(&coordinates[0])?;
                let holes: Result<Vec<_>, _> = coordinates[1..]
                    .iter()
                    .map(|ring| parse_ring(ring))
                    .collect();
                Ok(Geography::Polygon(PolygonValue::new(exterior, holes?)))
            }
            GeoJsonGeometry::MultiPoint { coordinates } => {
                let points: Result<Vec<_>, _> = coordinates
                    .iter()
                    .map(|c| {
                        if c.len() < 2 {
                            Err("MultiPoint coordinate must have at least 2 values".to_string())
                        } else {
                            Ok(GeographyValue::new(c[1], c[0]))
                        }
                    })
                    .collect();
                Ok(Geography::MultiPoint(MultiPointValue::new(points?)))
            }
            GeoJsonGeometry::MultiLineString { coordinates } => {
                let linestrings: Result<Vec<_>, _> =
                    coordinates.iter().map(|ring| parse_ring(ring)).collect();
                Ok(Geography::MultiLineString(MultiLineStringValue::new(
                    linestrings?,
                )))
            }
            GeoJsonGeometry::MultiPolygon { coordinates } => {
                let polygons: Result<Vec<_>, _> = coordinates
                    .iter()
                    .map(|poly_coords| {
                        if poly_coords.is_empty() {
                            return Err(
                                "MultiPolygon polygon must have at least one ring".to_string()
                            );
                        }
                        let exterior = parse_ring(&poly_coords[0])?;
                        let holes: Result<Vec<_>, _> = poly_coords[1..]
                            .iter()
                            .map(|ring| parse_ring(ring))
                            .collect();
                        Ok(PolygonValue::new(exterior, holes?))
                    })
                    .collect();
                Ok(Geography::MultiPolygon(MultiPolygonValue::new(polygons?)))
            }
        }
    }

    /// Convert to GeoJSON string
    pub fn to_geojson_string(&self) -> String {
        serde_json::to_string(&self.to_geojson()).unwrap_or_default()
    }

    /// Parse from GeoJSON string
    pub fn from_geojson_string(json: &str) -> Result<Self, String> {
        let geojson: GeoJsonGeometry =
            serde_json::from_str(json).map_err(|e| format!("Invalid GeoJSON: {}", e))?;
        Self::from_geojson(&geojson)
    }
}

fn parse_ring(coords: &[Vec<f64>]) -> Result<LineStringValue, String> {
    let points: Result<Vec<_>, _> = coords
        .iter()
        .map(|c| {
            if c.len() < 2 {
                Err("Ring coordinate must have at least 2 values".to_string())
            } else {
                Ok(GeographyValue::new(c[1], c[0]))
            }
        })
        .collect();
    Ok(LineStringValue::new(points?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point_wkt_parsing() {
        let wkt = "POINT(116.4 39.9)";
        let geo = Geography::from_wkt(wkt).unwrap();
        assert!(matches!(geo, Geography::Point(_)));
        if let Geography::Point(p) = geo {
            assert!((p.longitude - 116.4).abs() < 1e-6);
            assert!((p.latitude - 39.9).abs() < 1e-6);
        }
    }

    #[test]
    fn test_linestring_wkt_parsing() {
        let wkt = "LINESTRING(116.4 39.9, 121.5 31.2, 113.3 23.1)";
        let geo = Geography::from_wkt(wkt).unwrap();
        assert!(matches!(geo, Geography::LineString(_)));
        if let Geography::LineString(ls) = geo {
            assert_eq!(ls.points.len(), 3);
            assert!(ls.length() > 1000.0);
        }
    }

    #[test]
    fn test_polygon_wkt_parsing() {
        let wkt = "POLYGON((116.0 40.0, 117.0 40.0, 117.0 39.0, 116.0 39.0, 116.0 40.0))";
        let geo = Geography::from_wkt(wkt).unwrap();
        assert!(matches!(geo, Geography::Polygon(_)));
    }

    #[test]
    fn test_linestring_length() {
        let ls = LineStringValue::new(vec![
            GeographyValue::new(39.9, 116.4),
            GeographyValue::new(31.2, 121.5),
        ]);
        let length = ls.length();
        assert!(length > 1000.0);
        assert!(length < 1500.0);
    }

    #[test]
    fn test_polygon_contains_point() {
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
        let point_inside = GeographyValue::new(39.5, 116.5);
        let point_outside = GeographyValue::new(50.0, 120.0);

        assert!(polygon.contains_point(&point_inside));
        assert!(!polygon.contains_point(&point_outside));
    }

    #[test]
    fn test_wkt_roundtrip() {
        let wkt = "POINT(116.4 39.9)";
        let geo = Geography::from_wkt(wkt).unwrap();
        let roundtrip = geo.to_wkt();
        let geo2 = Geography::from_wkt(&roundtrip).unwrap();
        assert_eq!(geo, geo2);
    }

    #[test]
    fn test_centroid() {
        let ls = LineStringValue::new(vec![
            GeographyValue::new(0.0, 0.0),
            GeographyValue::new(10.0, 10.0),
        ]);
        let centroid = ls.centroid().unwrap();
        assert!((centroid.latitude - 5.0).abs() < 1e-6);
        assert!((centroid.longitude - 5.0).abs() < 1e-6);
    }

    #[test]
    fn test_is_valid() {
        let valid_point = GeographyValue::new(45.0, 90.0);
        assert!(valid_point.is_valid());

        let invalid_point = GeographyValue::new(100.0, 200.0);
        assert!(!invalid_point.is_valid());
    }

    #[test]
    fn test_multipoint_wkt_parsing() {
        let wkt = "MULTIPOINT(116.4 39.9, 121.5 31.2)";
        let geo = Geography::from_wkt(wkt).unwrap();
        assert!(matches!(geo, Geography::MultiPoint(_)));
        if let Geography::MultiPoint(mp) = geo {
            assert_eq!(mp.points.len(), 2);
        }
    }

    #[test]
    fn test_multilinestring_wkt_parsing() {
        let wkt = "MULTILINESTRING((116.4 39.9, 121.5 31.2), (113.3 23.1, 114.1 22.5))";
        let geo = Geography::from_wkt(wkt).unwrap();
        assert!(matches!(geo, Geography::MultiLineString(_)));
        if let Geography::MultiLineString(mls) = geo {
            assert_eq!(mls.linestrings.len(), 2);
        }
    }

    #[test]
    fn test_geojson_point() {
        let point = Geography::Point(GeographyValue::new(39.9, 116.4));
        let geojson = point.to_geojson();
        assert!(matches!(geojson, GeoJsonGeometry::Point { .. }));
        if let GeoJsonGeometry::Point { coordinates } = geojson {
            assert_eq!(coordinates, vec![116.4, 39.9]);
        }
    }

    #[test]
    fn test_geojson_linestring() {
        let ls = Geography::LineString(LineStringValue::new(vec![
            GeographyValue::new(39.9, 116.4),
            GeographyValue::new(31.2, 121.5),
        ]));
        let geojson = ls.to_geojson();
        assert!(matches!(geojson, GeoJsonGeometry::LineString { .. }));
        if let GeoJsonGeometry::LineString { coordinates } = geojson {
            assert_eq!(coordinates.len(), 2);
            assert_eq!(coordinates[0], vec![116.4, 39.9]);
        }
    }

    #[test]
    fn test_geojson_polygon() {
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
        let geo = Geography::Polygon(polygon);
        let geojson = geo.to_geojson();
        assert!(matches!(geojson, GeoJsonGeometry::Polygon { .. }));
    }

    #[test]
    fn test_geojson_roundtrip() {
        let point = Geography::Point(GeographyValue::new(39.9, 116.4));
        let geojson = point.to_geojson();
        let parsed = Geography::from_geojson(&geojson).unwrap();
        assert_eq!(point, parsed);

        let ls = Geography::LineString(LineStringValue::new(vec![
            GeographyValue::new(39.9, 116.4),
            GeographyValue::new(31.2, 121.5),
        ]));
        let geojson = ls.to_geojson();
        let parsed = Geography::from_geojson(&geojson).unwrap();
        assert_eq!(ls, parsed);
    }

    #[test]
    fn test_geojson_string() {
        let point = Geography::Point(GeographyValue::new(39.9, 116.4));
        let json_str = point.to_geojson_string();
        assert!(json_str.contains("\"type\":\"Point\""));
        assert!(json_str.contains("\"coordinates\""));

        let parsed = Geography::from_geojson_string(&json_str).unwrap();
        assert_eq!(point, parsed);
    }
}
