# 地理功能集成测试架构设计

## 概述

本文档设计了 GraphDB 地理空间功能的集成测试架构,参考 `tests/fulltext` 目录和 `tests/integration_fulltext.rs` 的组织方式,为地理功能提供完整的测试覆盖。

## 测试架构

### 1. 目录结构

```
tests/
├── geography/                      # 地理功能测试模块目录
│   ├── mod.rs                      # 模块入口,声明所有子模块
│   ├── common.rs                   # 通用测试工具和辅助函数
│   ├── basic.rs                    # 基础功能测试
│   ├── geometry_types.rs           # 几何类型测试
│   ├── spatial_functions.rs        # 空间函数测试
│   ├── format_conversion.rs        # 格式转换测试(WKT/GeoJSON)
│   ├── spatial_relations.rs        # 空间关系测试
│   ├── measurements.rs             # 测量函数测试
│   ├── edge_cases.rs               # 边界情况和异常处理
│   ├── performance.rs              # 性能测试
│   └── concurrent.rs               # 并发操作测试
└── integration_geography.rs        # 地理功能集成测试入口
```

### 2. 主入口文件 (integration_geography.rs)

```rust
//! Geography Integration Tests
//!
//! Test coverage:
//! - Geometry types - Point, LineString, Polygon, MultiPoint, MultiLineString, MultiPolygon
//! - Spatial functions - construction, conversion, properties, measurements, operations
//! - Format support - WKT (Well-Known Text), GeoJSON
//! - Spatial relations - intersects, contains, within, covers, crosses, touches, overlaps
//! - Edge cases - invalid coordinates, empty geometries, degenerate cases
//! - Error handling - type errors, null handling, invalid inputs
//! - Performance - large geometries, batch operations
//! - Concurrent operations - concurrent inserts, queries, mixed operations

mod common;
mod geography;
```

### 3. 模块入口文件 (geography/mod.rs)

```rust
//! Geography Integration Tests Module
//!
//! Test coverage:
//! - Basic operations - create, validate, convert geometries
//! - Geometry types - all 6 geometry types with full operations
//! - Spatial functions - 30+ spatial functions
//! - Format conversion - WKT and GeoJSON support
//! - Spatial relations - all spatial relationship functions
//! - Measurements - distance, area, length, perimeter
//! - Edge cases - boundary conditions and error handling
//! - Performance - large-scale geometry operations
//! - Concurrent operations - thread safety and concurrency

mod common;
mod basic;
mod geometry_types;
mod spatial_functions;
mod format_conversion;
mod spatial_relations;
mod measurements;
mod edge_cases;
mod performance;
mod concurrent;
```

## 测试模块详细设计

### 1. common.rs - 通用测试工具

```rust
//! Common test utilities for geography tests

use graphdb::core::value::geography::{
    Geography, GeographyValue, LineStringValue, PolygonValue,
    MultiPointValue, MultiLineStringValue, MultiPolygonValue,
};
use graphdb::core::Value;

/// Geography Test Context
pub struct GeographyTestContext {
    // 测试上下文字段
}

impl GeographyTestContext {
    pub fn new() -> Self {
        // 初始化测试上下文
    }
}

/// Create test point
pub fn create_test_point(lon: f64, lat: f64) -> Geography {
    Geography::Point(GeographyValue::new(lat, lon))
}

/// Create test linestring
pub fn create_test_linestring(points: Vec<(f64, f64)>) -> Geography {
    let geo_points: Vec<GeographyValue> = points
        .into_iter()
        .map(|(lon, lat)| GeographyValue::new(lat, lon))
        .collect();
    Geography::LineString(LineStringValue::new(geo_points))
}

/// Create test polygon
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

/// Assert geography equals with tolerance
pub fn assert_geography_equals(geo1: &Geography, geo2: &Geography, tolerance: f64) {
    // 比较两个地理对象是否在容差范围内相等
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

/// Generate test geometries
pub fn generate_test_geometries() -> Vec<Geography> {
    // 生成各种测试几何对象
}
```

### 2. basic.rs - 基础功能测试

**测试范围**:
- TC-GEO-001: 创建点几何
- TC-GEO-002: 创建线串几何
- TC-GEO-003: 创建多边形几何
- TC-GEO-004: 创建多点几何
- TC-GEO-005: 创建多线串几何
- TC-GEO-006: 创建多多边形几何
- TC-GEO-007: 几何对象验证
- TC-GEO-008: 几何对象序列化
- TC-GEO-009: 几何对象反序列化
- TC-GEO-010: 几何对象内存估算

**示例测试**:
```rust
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

/// TC-GEO-007: Geometry Validation
#[test]
fn test_geometry_validation() {
    // Valid point
    let valid_point = create_test_point(0.0, 0.0);
    assert!(valid_point.is_valid());
    
    // Invalid point (out of range)
    let invalid_point = Geography::Point(GeographyValue::new(100.0, 200.0));
    assert!(!invalid_point.is_valid());
}
```

### 3. geometry_types.rs - 几何类型测试

**测试范围**:
- TC-GEO-TYPE-001 ~ TC-GEO-TYPE-006: 每种几何类型的完整操作
- TC-GEO-TYPE-007: 几何类型判断
- TC-GEO-TYPE-008: 几何类型转换
- TC-GEO-TYPE-009: 空几何对象处理
- TC-GEO-TYPE-010: 复杂几何对象

**测试内容**:
- Point: 距离计算、方位角计算、边界框检测
- LineString: 长度计算、闭合检测、中心点计算
- Polygon: 面积计算、周长计算、点包含检测
- MultiPoint: 点数量、中心点、有效性
- MultiLineString: 线串数量、总长度、有效性
- MultiPolygon: 多边形数量、总面积、点包含

### 4. spatial_functions.rs - 空间函数测试

**测试范围**:
- TC-GEO-FUNC-001 ~ TC-GEO-FUNC-010: 构造函数测试
- TC-GEO-FUNC-011 ~ TC-GEO-FUNC-015: 转换函数测试
- TC-GEO-FUNC-016 ~ TC-GEO-FUNC-025: 属性函数测试
- TC-GEO-FUNC-026 ~ TC-GEO-FUNC-030: 操作函数测试

**示例测试**:
```rust
/// TC-GEO-FUNC-001: ST_Point Function
#[test]
fn test_st_point_function() {
    let args = vec![Value::Double(116.4074), Value::Double(39.9042)];
    let result = execute_st_point(&args).expect("ST_Point should succeed");
    
    match result {
        Value::Geography(Geography::Point(p)) => {
            assert_eq!(p.longitude, 116.4074);
            assert_eq!(p.latitude, 39.9042);
        }
        _ => panic!("Expected Geography value"),
    }
}

/// TC-GEO-FUNC-002: ST_GeogFromText Function
#[test]
fn test_st_geogfromtext_function() {
    let wkt = "POINT(116.4074 39.9042)";
    let args = vec![Value::String(wkt.to_string())];
    let result = execute_st_geogfromtext(&args).expect("ST_GeogFromText should succeed");
    
    match result {
        Value::Geography(Geography::Point(p)) => {
            assert_eq!(p.longitude, 116.4074);
            assert_eq!(p.latitude, 39.9042);
        }
        _ => panic!("Expected Geography value"),
    }
}
```

### 5. format_conversion.rs - 格式转换测试

**测试范围**:
- TC-GEO-FMT-001 ~ TC-GEO-FMT-006: WKT 格式转换
- TC-GEO-FMT-007 ~ TC-GEO-FMT-012: GeoJSON 格式转换
- TC-GEO-FMT-013 ~ TC-GEO-FMT-018: 往返转换测试
- TC-GEO-FMT-019 ~ TC-GEO-FMT-024: 错误格式处理

**示例测试**:
```rust
/// TC-GEO-FMT-001: WKT Point Conversion
#[test]
fn test_wkt_point_conversion() {
    let point = create_test_point(116.4074, 39.9042);
    let wkt = point.to_wkt();
    
    assert_eq!(wkt, "POINT(116.4074 39.9042)");
    
    // Parse back
    let parsed = Geography::from_wkt(&wkt).expect("WKT parsing should succeed");
    assert_eq!(point, parsed);
}

/// TC-GEO-FMT-007: GeoJSON Point Conversion
#[test]
fn test_geojson_point_conversion() {
    let point = create_test_point(116.4074, 39.9042);
    let geojson = point.to_geojson();
    
    match geojson {
        GeoJsonGeometry::Point { coordinates } => {
            assert_eq!(coordinates, vec![116.4074, 39.9042]);
        }
        _ => panic!("Expected Point GeoJSON"),
    }
}
```

### 6. spatial_relations.rs - 空间关系测试

**测试范围**:
- TC-GEO-REL-001 ~ TC-GEO-REL-010: ST_Intersects 测试
- TC-GEO-REL-011 ~ TC-GEO-REL-020: ST_Contains 测试
- TC-GEO-REL-021 ~ TC-GEO-REL-030: ST_Within 测试
- TC-GEO-REL-031 ~ TC-GEO-REL-040: ST_DWithin 测试
- TC-GEO-REL-041 ~ TC-GEO-REL-050: 其他空间关系测试

**示例测试**:
```rust
/// TC-GEO-REL-001: ST_Intersects - Point and Polygon
#[test]
fn test_st_intersects_point_polygon() {
    let point = create_test_point(0.5, 0.5);
    let polygon = create_test_polygon(
        vec![(0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0)],
        vec![],
    );
    
    let args = vec![
        Value::Geography(point),
        Value::Geography(polygon),
    ];
    
    let result = execute_st_intersects(&args).expect("ST_Intersects should succeed");
    assert_eq!(result, Value::Bool(true));
}

/// TC-GEO-REL-031: ST_DWithin - Points within distance
#[test]
fn test_st_dwithin_points() {
    let point1 = create_test_point(0.0, 0.0);
    let point2 = create_test_point(0.0, 0.01); // ~1.11 km apart
    
    let args = vec![
        Value::Geography(point1),
        Value::Geography(point2),
        Value::Double(2.0), // 2 km threshold
    ];
    
    let result = execute_st_dwithin(&args).expect("ST_DWithin should succeed");
    assert_eq!(result, Value::Bool(true));
}
```

### 7. measurements.rs - 测量函数测试

**测试范围**:
- TC-GEO-MEAS-001 ~ TC-GEO-MEAS-010: ST_Distance 测试
- TC-GEO-MEAS-011 ~ TC-GEO-MEAS-020: ST_Area 测试
- TC-GEO-MEAS-021 ~ TC-GEO-MEAS-030: ST_Length 测试
- TC-GEO-MEAS-031 ~ TC-GEO-MEAS-040: ST_Perimeter 测试

**示例测试**:
```rust
/// TC-GEO-MEAS-001: ST_Distance - Point to Point
#[test]
fn test_st_distance_point_to_point() {
    let point1 = create_test_point(0.0, 0.0);
    let point2 = create_test_point(0.0, 1.0); // ~111 km apart
    
    let args = vec![
        Value::Geography(point1),
        Value::Geography(point2),
    ];
    
    let result = execute_st_distance(&args).expect("ST_Distance should succeed");
    
    match result {
        Value::Double(distance) => {
            // Should be approximately 111.32 km
            assert_distance_within(distance, 111.32, 1.0);
        }
        _ => panic!("Expected Double value"),
    }
}

/// TC-GEO-MEAS-011: ST_Area - Polygon Area
#[test]
fn test_st_area_polygon() {
    // Create a 1 degree x 1 degree square
    let polygon = create_test_polygon(
        vec![
            (0.0, 0.0),
            (0.0, 1.0),
            (1.0, 1.0),
            (1.0, 0.0),
            (0.0, 0.0),
        ],
        vec![],
    );
    
    let args = vec![Value::Geography(polygon)];
    let result = execute_st_area(&args).expect("ST_Area should succeed");
    
    match result {
        Value::Double(area) => {
            // Should be approximately 12,360 km²
            assert!(area > 12000.0 && area < 13000.0);
        }
        _ => panic!("Expected Double value"),
    }
}
```

### 8. edge_cases.rs - 边界情况测试

**测试范围**:
- TC-GEO-EDGE-001 ~ TC-GEO-EDGE-010: 无效坐标处理
- TC-GEO-EDGE-011 ~ TC-GEO-EDGE-020: 空几何对象
- TC-GEO-EDGE-021 ~ TC-GEO-EDGE-030: 退化几何对象
- TC-GEO-EDGE-031 ~ TC-GEO-EDGE-040: Null 值处理
- TC-GEO-EDGE-041 ~ TC-GEO-EDGE-050: 类型错误处理

**示例测试**:
```rust
/// TC-GEO-EDGE-001: Invalid Coordinates
#[test]
fn test_invalid_coordinates() {
    // Latitude out of range
    let invalid_lat = GeographyValue::new(100.0, 0.0);
    assert!(!invalid_lat.is_valid());
    
    // Longitude out of range
    let invalid_lon = GeographyValue::new(0.0, 200.0);
    assert!(!invalid_lon.is_valid());
}

/// TC-GEO-EDGE-031: Null Value Handling
#[test]
fn test_null_value_handling() {
    let args = vec![Value::Null(NullType::Null), Value::Double(39.9042)];
    let result = execute_st_point(&args).expect("ST_Point with null should return null");
    
    assert_eq!(result, Value::Null(NullType::Null));
}

/// TC-GEO-EDGE-041: Type Error Handling
#[test]
fn test_type_error_handling() {
    let args = vec![Value::String("invalid".to_string()), Value::Double(39.9042)];
    let result = execute_st_point(&args);
    
    assert!(result.is_err(), "Should return type error for string argument");
}
```

### 9. performance.rs - 性能测试

**测试范围**:
- TC-GEO-PERF-001: 大型多边形性能
- TC-GEO-PERF-002: 批量距离计算
- TC-GEO-PERF-003: 复杂空间关系计算
- TC-GEO-PERF-004: WKT 解析性能
- TC-GEO-PERF-005: GeoJSON 序列化性能

**示例测试**:
```rust
/// TC-GEO-PERF-001: Large Polygon Performance
#[test]
fn test_large_polygon_performance() {
    // Create a polygon with 10,000 vertices
    let mut points = Vec::with_capacity(10001);
    for i in 0..10000 {
        let angle = 2.0 * std::f64::consts::PI * i as f64 / 10000.0;
        let lon = angle.cos();
        let lat = angle.sin();
        points.push((lon, lat));
    }
    points.push(points[0]); // Close the polygon
    
    let polygon = create_test_polygon(points, vec![]);
    
    let start = std::time::Instant::now();
    let area = match &polygon {
        Geography::Polygon(p) => p.area(),
        _ => 0.0,
    };
    let duration = start.elapsed();
    
    println!("Large polygon area calculation took {:?}", duration);
    assert!(duration.as_millis() < 100, "Should complete within 100ms");
}
```

### 10. concurrent.rs - 并发操作测试

**测试范围**:
- TC-GEO-CONC-001: 并发几何对象创建
- TC-GEO-CONC-002: 并发距离计算
- TC-GEO-CONC-003: 并发空间关系判断
- TC-GEO-CONC-004: 并发格式转换
- TC-GEO-CONC-005: 并发混合操作

**示例测试**:
```rust
/// TC-GEO-CONC-001: Concurrent Geometry Creation
#[tokio::test]
async fn test_concurrent_geometry_creation() {
    let num_tasks = 100;
    let mut handles = vec![];
    
    for i in 0..num_tasks {
        let handle = tokio::spawn(async move {
            let lon = i as f64 * 0.1;
            let lat = i as f64 * 0.1;
            create_test_point(lon, lat)
        });
        handles.push(handle);
    }
    
    let results: Vec<_> = futures::future::join_all(handles).await;
    
    assert_eq!(results.len(), num_tasks);
    for result in results {
        let geo = result.expect("Task should succeed");
        assert!(geo.is_valid());
    }
}
```

## 测试数据设计

### 1. 标准测试数据集

```rust
/// Standard test geometries
pub fn get_standard_test_geometries() -> HashMap<String, Geography> {
    let mut geometries = HashMap::new();
    
    // Points
    geometries.insert("beijing".to_string(), create_test_point(116.4074, 39.9042));
    geometries.insert("shanghai".to_string(), create_test_point(121.4737, 31.2304));
    geometries.insert("newyork".to_string(), create_test_point(-74.0060, 40.7128));
    
    // LineStrings
    geometries.insert("simple_line".to_string(), create_test_linestring(vec![
        (0.0, 0.0), (1.0, 1.0), (2.0, 2.0),
    ]));
    
    // Polygons
    geometries.insert("unit_square".to_string(), create_test_polygon(vec![
        (0.0, 0.0), (0.0, 1.0), (1.0, 1.0), (1.0, 0.0), (0.0, 0.0),
    ], vec![]));
    
    geometries
}
```

### 2. 边界测试数据

```rust
/// Edge case test geometries
pub fn get_edge_case_geometries() -> HashMap<String, Geography> {
    let mut geometries = HashMap::new();
    
    // Empty geometries
    geometries.insert("empty_linestring".to_string(), 
        Geography::LineString(LineStringValue::new(vec![])));
    
    // Degenerate geometries
    geometries.insert("degenerate_polygon".to_string(), 
        create_test_polygon(vec![(0.0, 0.0)], vec![]));
    
    // Invalid coordinates
    geometries.insert("invalid_point".to_string(), 
        Geography::Point(GeographyValue::new(100.0, 200.0)));
    
    geometries
}
```

## 测试执行策略

### 1. 单元测试
- 每个测试函数独立运行
- 使用 `#[test]` 属性
- 快速反馈,适合开发阶段

### 2. 集成测试
- 测试多个组件协同工作
- 使用 `#[tokio::test]` 进行异步测试
- 验证端到端功能

### 3. 性能测试
- 使用基准测试框架
- 记录执行时间
- 监控内存使用

### 4. 测试覆盖率
- 目标覆盖率: 80% 以上
- 关键路径: 100% 覆盖
- 边界情况: 全面覆盖

## 测试命令

```bash
# 运行所有地理功能测试
cargo test --test integration_geography

# 运行特定测试模块
cargo test --test integration_geography -- mod::test_name

# 运行性能测试
cargo test --test integration_geography -- --nocapture perf

# 生成测试覆盖率报告
cargo tarpaulin --test integration_geography
```

## 持续集成

### GitHub Actions 配置

```yaml
name: Geography Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run geography tests
        run: cargo test --test integration_geography --all-features
      - name: Generate coverage
        run: cargo tarpaulin --test integration_geography
```

## 测试报告

### 1. 测试摘要
- 总测试数量: ~150 个测试用例
- 覆盖功能: 所有地理空间功能
- 预期通过率: 100%

### 2. 测试分类
- 基础功能: 30 个测试
- 几何类型: 20 个测试
- 空间函数: 40 个测试
- 格式转换: 20 个测试
- 空间关系: 20 个测试
- 测量函数: 10 个测试
- 边界情况: 10 个测试

## 总结

本测试架构设计提供了完整的地理空间功能测试覆盖,包括:
- 10 个测试模块,覆盖所有功能点
- ~150 个测试用例,确保功能正确性
- 完整的测试工具和辅助函数
- 性能测试和并发测试支持
- 持续集成配置

该架构参考了 `tests/fulltext` 的组织方式,确保测试代码的可维护性和可扩展性。
