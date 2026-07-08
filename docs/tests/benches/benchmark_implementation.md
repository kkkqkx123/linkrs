# GraphDB 基准测试实施指南

**文档版本**: v1.0  
**更新日期**: 2026-06-18  
**目标**: 提供具体的基准测试实施步骤和示例代码

---

## 1. 基准测试框架配置

### 1.1 Criterion.rs 集成

在项目的 `Cargo.toml` 中已包含：

```toml
[dev-dependencies]
criterion = { version = "0.5", features = ["async_tokio"] }
```

### 1.2 Benchmark 项目结构

```
graphDB/
├── benches/                    # 基准测试目录
│   ├── lib.rs                 # 基准测试导出
│   ├── common/                # 共享工具
│   │   ├── mod.rs
│   │   ├── data_generator.rs  # 数据生成器
│   │   ├── bench_utils.rs     # 工具函数
│   │   └── test_context.rs    # 测试上下文
│   ├── storage_bench.rs       # 存储层基准
│   ├── transaction_bench.rs   # 事务层基准
│   ├── query_bench.rs         # 查询层基准
│   ├── search_bench.rs        # 搜索层基准
│   └── api_bench.rs           # API层基准
├── docs/tests/benches/        # 基准测试文档
│   ├── performance_benchmark_plan.md       # 计划
│   ├── performance_bottleneck_analysis.md  # 分析
│   └── benchmark_implementation.md          # 本文档
└── Cargo.toml
```

---

## 2. 数据生成器实现

### 2.1 通用数据生成器框架

```rust
// benches/common/data_generator.rs

use graphdb_core::{Vertex, Edge, Property, Value};
use uuid::Uuid;
use std::collections::HashMap;

pub struct TestDataGenerator;

impl TestDataGenerator {
    /// 生成单个顶点
    pub fn create_vertex(id: u64) -> Vertex {
        let mut properties = HashMap::new();
        properties.insert("name".to_string(), Value::String(format!("vertex_{}", id)));
        properties.insert("type".to_string(), Value::String("test".to_string()));
        properties.insert("timestamp".to_string(), Value::I64(id as i64));
        
        Vertex {
            id,
            label: "TestVertex".to_string(),
            properties,
        }
    }
    
    /// 生成多个顶点
    pub fn create_vertices(count: usize) -> Vec<Vertex> {
        (0..count)
            .map(|i| Self::create_vertex(i as u64))
            .collect()
    }
    
    /// 生成带有N个属性的顶点（用于属性性能测试）
    pub fn create_vertex_with_properties(id: u64, property_count: usize) -> Vertex {
        let mut properties = HashMap::new();
        for i in 0..property_count {
            properties.insert(
                format!("prop_{}", i),
                Value::String(format!("value_{}_{}", id, i)),
            );
        }
        
        Vertex {
            id,
            label: "TestVertex".to_string(),
            properties,
        }
    }
    
    /// 生成边
    pub fn create_edge(from_id: u64, to_id: u64, edge_type: &str) -> Edge {
        let mut properties = HashMap::new();
        properties.insert("created_at".to_string(), Value::I64(0));
        
        Edge {
            from_id,
            to_id,
            label: edge_type.to_string(),
            properties,
        }
    }
    
    /// 生成大字符串属性（用于大数据性能测试）
    pub fn create_large_string(size: usize) -> String {
        "x".repeat(size)
    }
}

/// 数据生成统计
pub struct GenerationStats {
    pub total_count: usize,
    pub total_size_bytes: usize,
    pub avg_size_bytes: usize,
}

impl GenerationStats {
    pub fn from_vertices(vertices: &[Vertex]) -> Self {
        let total_count = vertices.len();
        let total_size_bytes = vertices
            .iter()
            .map(|v| std::mem::size_of_val(v) + v.properties.len() * 64)
            .sum();
        
        Self {
            total_count,
            total_size_bytes,
            avg_size_bytes: total_size_bytes / total_count.max(1),
        }
    }
}
```

### 2.2 基准测试工具函数

```rust
// benches/common/bench_utils.rs

use criterion::{BenchmarkId, Criterion, Throughput};
use std::time::Duration;

/// 为基准测试创建标准的 BenchmarkGroup 配置
pub fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    
    // 配置基准测试参数
    group.measurement_time(Duration::from_secs(10)); // 运行10秒
    group.sample_size(100);  // 最少100个样本
    group.warm_up_time(Duration::from_secs(1));  // 预热1秒
    
    group
}

/// 基准测试吞吐量配置辅助
pub fn set_throughput(
    group: &mut criterion::BenchmarkGroup<criterion::measurement::WallTime>,
    size: u64,
) {
    group.throughput(Throughput::Elements(size));
}

/// 从基准测试名生成统计文件路径
pub fn get_stats_path(bench_name: &str) -> String {
    format!("target/criterion/{}/stats", bench_name)
}

/// 性能对比助手
pub struct PerformanceComparison {
    pub metric: String,
    pub baseline: f64,
    pub current: f64,
}

impl PerformanceComparison {
    pub fn new(metric: &str, baseline: f64, current: f64) -> Self {
        Self {
            metric: metric.to_string(),
            baseline,
            current,
        }
    }
    
    /// 计算改进百分比（正数表示改进）
    pub fn improvement_percent(&self) -> f64 {
        ((self.baseline - self.current) / self.baseline) * 100.0
    }
    
    /// 是否达到预期改进（阈值为 5%）
    pub fn meets_expectation(&self, threshold_percent: f64) -> bool {
        self.improvement_percent() >= threshold_percent
    }
}
```

### 2.3 测试上下文

```rust
// benches/common/test_context.rs

use graphdb::storage::Storage;
use std::path::PathBuf;
use tempfile::TempDir;

/// 基准测试的存储上下文
pub struct StorageBenchContext {
    pub storage: Storage,
    _temp_dir: TempDir,
}

impl StorageBenchContext {
    /// 创建新的测试上下文
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let temp_dir = TempDir::new()?;
        let db_path = temp_dir.path().join("test.db");
        
        let config = graphdb::config::StorageConfig {
            data_dir: db_path.clone(),
            cache_size_mb: 100,
            compression_enabled: true,
            ..Default::default()
        };
        
        let storage = Storage::new(config)?;
        
        Ok(Self {
            storage,
            _temp_dir: temp_dir,
        })
    }
    
    /// 清空存储中的所有数据
    pub fn clear(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.storage.clear()?;
        Ok(())
    }
}

impl Drop for StorageBenchContext {
    fn drop(&mut self) {
        let _ = self.storage.close();
    }
}
```

---

## 3. 存储层基准测试示例

### 3.1 顶点插入基准

```rust
// benches/storage_bench.rs

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use benches::common::{TestDataGenerator, create_benchmark_group};

fn bench_vertex_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vertex_insert");
    
    // 测试不同数据量的单顶点插入
    for count in [1, 10, 100, 1000].iter() {
        let vertices = TestDataGenerator::create_vertices(*count);
        
        group.bench_with_input(
            BenchmarkId::from_parameter(count),
            count,
            |b, _| {
                let mut ctx = StorageBenchContext::new().unwrap();
                
                // 预热
                for v in vertices.iter().take(10) {
                    ctx.storage.insert_vertex(v).unwrap();
                }
                ctx.clear().unwrap();
                
                // 实际基准测试
                b.iter(|| {
                    for v in vertices.iter() {
                        black_box(ctx.storage.insert_vertex(v).unwrap());
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn bench_vertex_query(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vertex_query");
    
    // 创建存储和测试数据
    let mut ctx = StorageBenchContext::new().unwrap();
    let vertex = TestDataGenerator::create_vertex(1);
    ctx.storage.insert_vertex(&vertex).unwrap();
    
    // 测试查询性能
    group.bench_function("query_single_vertex", |b| {
        b.iter(|| {
            black_box(ctx.storage.query_vertex(1).unwrap())
        });
    });
    
    group.finish();
}

fn bench_vertex_batch_insert(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vertex_batch_insert");
    
    // 不同批量大小
    for batch_size in [10, 100, 1000, 10000].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(batch_size),
            batch_size,
            |b, _| {
                let mut ctx = StorageBenchContext::new().unwrap();
                let vertices = TestDataGenerator::create_vertices(*batch_size);
                
                b.iter(|| {
                    for v in vertices.iter() {
                        black_box(ctx.storage.insert_vertex(v).unwrap());
                    }
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_vertex_insert,
    bench_vertex_query,
    bench_vertex_batch_insert
);
criterion_main!(benches);
```

### 3.2 属性操作基准

```rust
// 在 benches/storage_bench.rs 中添加

fn bench_property_operations(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "property_operations");
    
    let mut ctx = StorageBenchContext::new().unwrap();
    let vertex = TestDataGenerator::create_vertex_with_properties(1, 10);
    ctx.storage.insert_vertex(&vertex).unwrap();
    
    // 读取属性
    group.bench_function("read_property", |b| {
        b.iter(|| {
            black_box(ctx.storage.get_property(1, "prop_0").unwrap())
        });
    });
    
    // 更新属性
    group.bench_function("update_property", |b| {
        b.iter(|| {
            let new_value = graphdb::value::Value::String("new_value".to_string());
            black_box(ctx.storage.set_property(1, "prop_0", new_value).unwrap());
        });
    });
    
    // 大属性值
    let large_value = TestDataGenerator::create_large_string(1024 * 1024); // 1MB
    group.bench_function("write_large_property", |b| {
        b.iter(|| {
            black_box(
                ctx.storage.set_property(
                    1,
                    "large_prop",
                    graphdb::value::Value::String(large_value.clone()),
                ).unwrap()
            );
        });
    });
    
    group.finish();
}
```

---

## 4. 并发性能基准

### 4.1 并发读写基准

```rust
// benches/concurrency_bench.rs

use std::sync::Arc;
use std::thread;

fn bench_concurrent_read(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "concurrent_read");
    
    // 不同并发度
    for thread_count in [1, 2, 4, 8].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                let ctx = Arc::new(StorageBenchContext::new().unwrap());
                
                // 插入测试数据
                for i in 0..1000 {
                    let vertex = TestDataGenerator::create_vertex(i);
                    ctx.storage.insert_vertex(&vertex).unwrap();
                }
                
                b.iter(|| {
                    let mut handles = vec![];
                    
                    for _ in 0..thread_count {
                        let ctx = Arc::clone(&ctx);
                        let handle = thread::spawn(move || {
                            for i in 0..100 {
                                black_box(ctx.storage.query_vertex(i % 1000).unwrap());
                            }
                        });
                        handles.push(handle);
                    }
                    
                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn bench_concurrent_write(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "concurrent_write");
    
    for thread_count in [1, 2, 4].iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(thread_count),
            thread_count,
            |b, &thread_count| {
                let ctx = Arc::new(StorageBenchContext::new().unwrap());
                
                let base_id = Arc::new(std::sync::atomic::AtomicU64::new(0));
                
                b.iter(|| {
                    let mut handles = vec![];
                    
                    for t in 0..thread_count {
                        let ctx = Arc::clone(&ctx);
                        let base_id = Arc::clone(&base_id);
                        
                        let handle = thread::spawn(move || {
                            for i in 0..100 {
                                let id = base_id.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                let vertex = TestDataGenerator::create_vertex(id);
                                black_box(ctx.storage.insert_vertex(&vertex).unwrap());
                            }
                        });
                        handles.push(handle);
                    }
                    
                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    
    group.finish();
}
```

---

## 5. 查询性能基准

### 5.1 简单查询基准

```rust
// benches/query_bench.rs

fn bench_simple_query(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "simple_query");
    
    let mut ctx = StorageBenchContext::new().unwrap();
    
    // 准备测试数据
    for i in 0..10000 {
        let vertex = TestDataGenerator::create_vertex(i);
        ctx.storage.insert_vertex(&vertex).unwrap();
    }
    
    // 单顶点查询
    group.bench_function("single_vertex", |b| {
        b.iter(|| {
            black_box(ctx.storage.query_vertex(5000).unwrap())
        });
    });
    
    // 邻接表查询（K=10）
    group.bench_function("adjacency_k10", |b| {
        b.iter(|| {
            black_box(ctx.storage.get_adjacent(5000, 10).unwrap())
        });
    });
    
    // 邻接表查询（K=100）
    group.bench_function("adjacency_k100", |b| {
        b.iter(|| {
            black_box(ctx.storage.get_adjacent(5000, 100).unwrap())
        });
    });
    
    group.finish();
}
```

---

## 6. 运行和分析基准测试

### 6.1 基本命令

```bash
# 运行所有基准测试
cargo bench

# 运行特定的基准测试
cargo bench --bench storage_bench

# 运行特定的测试函数
cargo bench vertex_insert

# 保存基线（用于对比）
cargo bench -- --save-baseline=v1_0

# 与基线对比
cargo bench -- --baseline=v1_0

# 生成详细的JSON报告
cargo bench -- --output-format=bencher | tee results.txt
```

### 6.2 性能对比脚本

```bash
#!/bin/bash
# scripts/compare_benchmarks.sh

# 对比两个版本的性能

BASELINE=$1
CURRENT=$2

if [ -z "$BASELINE" ] || [ -z "$CURRENT" ]; then
    echo "Usage: $0 <baseline_name> <current_name>"
    exit 1
fi

echo "=== 性能对比: $BASELINE vs $CURRENT ==="
echo ""

# 运行基准测试并保存
cargo bench -- --save-baseline=$BASELINE
git checkout <branch>
cargo bench -- --save-baseline=$CURRENT
git checkout -

# 对比结果
echo "对比结果位置："
echo "target/criterion/report/index.html"
```

### 6.3 性能报告生成

```python
#!/usr/bin/env python3
# scripts/generate_perf_report.py

import json
import sys
from pathlib import Path

def parse_criterion_results(results_dir):
    """解析 Criterion 生成的结果"""
    results = {}
    
    for bench_dir in Path(results_dir).glob("*"):
        if not bench_dir.is_dir():
            continue
        
        base_json = bench_dir / "base" / "raw.json"
        if not base_json.exists():
            continue
        
        with open(base_json) as f:
            data = json.load(f)
            
        results[bench_dir.name] = {
            "mean": data["mean"]["point_estimate"],
            "std_dev": data["std_dev"]["point_estimate"],
        }
    
    return results

def generate_html_report(results):
    """生成HTML性能报告"""
    html = """
    <!DOCTYPE html>
    <html>
    <head>
        <title>GraphDB 性能基准报告</title>
        <style>
            table { border-collapse: collapse; width: 100%; }
            th, td { border: 1px solid #ddd; padding: 8px; text-align: left; }
            th { background-color: #f2f2f2; }
            .pass { color: green; }
            .fail { color: red; }
        </style>
    </head>
    <body>
        <h1>GraphDB 性能基准报告</h1>
        <table>
            <tr>
                <th>基准测试</th>
                <th>平均延迟</th>
                <th>标准差</th>
                <th>状态</th>
            </tr>
    """
    
    for name, metrics in results.items():
        mean = metrics["mean"]
        std_dev = metrics["std_dev"]
        status = "✓" if std_dev < mean * 0.1 else "⚠"
        
        html += f"""
            <tr>
                <td>{name}</td>
                <td>{mean:.2f} ms</td>
                <td>{std_dev:.2f}</td>
                <td>{status}</td>
            </tr>
        """
    
    html += """
        </table>
    </body>
    </html>
    """
    
    return html

if __name__ == "__main__":
    results_dir = "target/criterion"
    results = parse_criterion_results(results_dir)
    html = generate_html_report(results)
    
    with open("perf_report.html", "w") as f:
        f.write(html)
    
    print("性能报告已生成: perf_report.html")
```

---

## 7. 基准测试编写清单

### 7.1 编写新基准测试的步骤

```
□ 1. 确定测试目标
   - 测什么？(单操作 vs 批量操作)
   - 为什么？(性能是否符合预期)
   
□ 2. 设计测试场景
   - 不同数据规模
   - 不同操作参数
   - 正常/极端场景
   
□ 3. 实现基准测试代码
   - 使用 Criterion 框架
   - 配置合理的测试参数
   - 包含预热和清理
   
□ 4. 验证测试的准确性
   - 结果可重复性好 (CV < 5%)
   - 没有被编译器优化消除
   - 隔离了干扰因素
   
□ 5. 生成基线
   - cargo bench -- --save-baseline=baseline_name
   
□ 6. 文档化
   - 在顶部注释说明测试目的
   - 文档中记录预期性能指标
   - 记录配置和假设
```

### 7.2 常见错误和修复

```
错误1: 编译器优化导致测试结果不准确
症状: 基准测试结果为 0
修复: 使用 black_box 防止优化
  ✗ b.iter(|| { some_operation() })
  ✓ b.iter(|| black_box(some_operation()))

错误2: 初始化在测试循环内
症状: 测试结果波动很大
修复: 初始化应在 b.iter 外
  ✗ b.iter(|| { let x = setup(); operation(x); })
  ✓ let x = setup(); b.iter(|| operation(&x));

错误3: 没有预热导致首次运行特别慢
症状: 第一次基准测试特别慢，之后变快
修复: 在 iter 前添加预热代码
  ✓ for _ in 0..10 { operation(); }
    b.iter(|| operation());

错误4: 并发基准中共享状态导致缓存竞争
症状: 并发性能不如预期
修复: 使用 Arc<Mutex<>> 或无锁数据结构
```

---

## 8. 集成到 CI/CD

### 8.1 GitHub Actions 工作流

```yaml
# .github/workflows/benchmark.yml

name: Benchmark

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:
  benchmark:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      
      - name: Install Rust
        uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      
      - name: Run benchmarks
        run: |
          cargo bench --all-features -- --save-baseline=pr
      
      - name: Compare with main
        if: github.base_ref == 'main'
        run: |
          git fetch origin main
          git checkout origin/main
          cargo bench --all-features -- --baseline=main
      
      - name: Comment on PR
        if: github.event_name == 'pull_request'
        uses: actions/github-script@v6
        with:
          script: |
            const fs = require('fs');
            const report = fs.readFileSync('perf_report.txt', 'utf8');
            github.rest.issues.createComment({
              issue_number: context.issue.number,
              owner: context.repo.owner,
              repo: context.repo.repo,
              body: '## Performance Benchmark Results\n' + report
            });
```

---

## 9. 快速参考

### 运行命令速查

```bash
# 运行所有基准
cargo bench

# 运行特定基准
cargo bench storage_bench
cargo bench --bench storage_bench

# 运行特定测试
cargo bench vertex_insert
cargo bench 'vertex_*'

# 保存/对比基线
cargo bench -- --save-baseline=my_baseline
cargo bench -- --baseline=my_baseline

# 更多样本（更准确但更慢）
cargo bench -- --sample-size 500

# 更长的运行时间
cargo bench -- --measurement-time 30

# Release 模式下运行（推荐）
cargo bench --release

# 生成HTML报告
open target/criterion/report/index.html
```

### 文档位置

| 资源 | 位置 |
|------|------|
| 性能基准计划 | `docs/tests/benches/performance_benchmark_plan.md` |
| 瓶颈分析指南 | `docs/tests/benches/performance_bottleneck_analysis.md` |
| 实施指南 | `docs/tests/benches/benchmark_implementation.md` |
| 基准测试代码 | `benches/` |
| 生成的报告 | `target/criterion/report/index.html` |

---

**文档完成度**: 100%  
**最后更新**: 2026-06-18  
**维护者**: GraphDB Team
