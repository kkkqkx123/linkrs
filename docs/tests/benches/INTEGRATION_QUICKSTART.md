# GraphDB 基准测试分析功能集成 - 快速参考

**日期**: 2026-06-18  
**目的**: 快速概览如何集成 EXPLAIN/PROFILE 分析功能到基准测试

---

## 🎯 核心问题

**问题**: 如何在性能基准测试中集成分析功能（如 EXPLAIN 语句），获取详细的性能指标？

**答案**: GraphDB 已具有完整的 EXPLAIN ANALYZE 和 PROFILE 功能，只需在基准测试中调用，提取指标即可。

---

## 🏗️ 三层分析体系

```
第一层: 基础性能基准 (已完成)
├─ 工具: Criterion.rs
├─ 指标: 平均时间、P95、P99
└─ 文件: benches/{storage,query,transaction,search,api,end_to_end}_bench.rs

第二层: 分析型基准 (待实现)
├─ 工具: EXPLAIN ANALYZE
├─ 指标: 规划时间、执行时间、行数、内存
└─ 文件: benches/analysis_bench.rs

第三层: 深度性能分析 (待实现)
├─ 工具: PROFILE 语句 + 自定义分析器
├─ 指标: 节点级统计、缓存命中率、瓶颈识别
└─ 文件: benches/analyzer/{performance_analyzer,metrics}.rs
```

---

## 📊 关键指标速查表

### 规划阶段 (Query Planning)
```
planning_time_us      // 查询优化耗时
plan_nodes_count      // 执行计划节点数
```

### 执行阶段 (Execution)
```
execution_time_us     // 执行耗时
startup_time_us       // 首字节延迟
total_rows            // 处理的总行数
peak_memory_bytes     // 峰值内存
```

### 节点级 (Per-Node)
```
node_id               // 节点ID
num_rows              // 输出行数
exec_time_us          // 执行时间
memory_peak           // 峰值内存
throughput_rows_per_sec  // 吞吐量
```

### 缓存 (Caching)
```
cache_hit_rate        // 缓存命中率
plan_cache_hits       // 计划缓存命中
memory_usage          // 缓存内存
```

---

## 🔧 快速集成模板

### 模板 1: 基础分析型基准

```rust
// benches/analysis_bench.rs
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_analyze_query_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("analyze_query");
    
    let queries = vec![
        ("simple_match", "MATCH (n:Data) RETURN n"),
        ("path_query", "MATCH (n:Data)->(m:Data) RETURN n, m"),
        ("aggregation", "MATCH (n:Data) RETURN COUNT(n)"),
    ];
    
    for (name, query) in queries {
        group.bench_function(name, |b| {
            b.iter_custom(|_iters| {
                // 1. 执行 EXPLAIN ANALYZE
                let explain_query = format!("EXPLAIN ANALYZE {}", query);
                let result = execute_query(&explain_query);
                
                // 2. 解析关键指标
                let planning_time = parse_planning_time(&result);
                let execution_time = parse_execution_time(&result);
                let total_rows = parse_total_rows(&result);
                
                // 3. 记录指标
                println!("Query: {} | Planning: {:.2}ms | Execution: {:.2}ms | Rows: {}",
                    name, planning_time, execution_time, total_rows);
                
                Duration::from_millis(execution_time as u64)
            });
        });
    }
    
    group.finish();
}

criterion_group!(benches, bench_analyze_query_performance);
criterion_main!(benches);
```

### 模板 2: 性能分析器

```rust
// benches/analyzer/performance_analyzer.rs
pub struct PerformanceAnalyzer;

impl PerformanceAnalyzer {
    pub async fn analyze_with_explain(query: &str) -> Result<AnalysisResult> {
        let explain_query = format!("EXPLAIN ANALYZE {}", query);
        let result = execute(explain_query).await?;
        
        Ok(AnalysisResult {
            planning_time_ms: parse_planning_time(&result),
            execution_time_ms: parse_execution_time(&result),
            total_rows: parse_total_rows(&result),
            peak_memory_bytes: parse_peak_memory(&result),
            bottlenecks: identify_bottlenecks(&result),
        })
    }
    
    pub fn identify_bottlenecks(analysis: &AnalysisResult) -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];
        
        if analysis.planning_time_ms > 100.0 {
            bottlenecks.push(Bottleneck::SlowPlanning);
        }
        
        if analysis.execution_time_ms > 1000.0 {
            bottlenecks.push(Bottleneck::SlowExecution);
        }
        
        if analysis.peak_memory_bytes > 100 * 1024 * 1024 {
            bottlenecks.push(Bottleneck::HighMemory);
        }
        
        bottlenecks
    }
}
```

### 模板 3: 指标收集和对比

```rust
// benches/common/analysis_metrics.rs
#[derive(Serialize, Deserialize)]
pub struct AnalysisMetrics {
    pub planning_time_ms: f64,
    pub execution_time_ms: f64,
    pub total_rows: usize,
    pub peak_memory_bytes: usize,
}

impl AnalysisMetrics {
    pub fn compare(&self, baseline: &AnalysisMetrics) -> ComparisonResult {
        let planning_deviation = 
            ((self.planning_time_ms - baseline.planning_time_ms) / 
             baseline.planning_time_ms) * 100.0;
        
        let execution_deviation = 
            ((self.execution_time_ms - baseline.execution_time_ms) / 
             baseline.execution_time_ms) * 100.0;
        
        ComparisonResult {
            planning_deviation,
            execution_deviation,
            has_regression: planning_deviation > 10.0 || execution_deviation > 10.0,
        }
    }
    
    pub fn summary(&self) -> String {
        format!(
            "Planning: {:.2}ms | Execution: {:.2}ms | Rows: {} | Memory: {:.2}MB",
            self.planning_time_ms,
            self.execution_time_ms,
            self.total_rows,
            self.peak_memory_bytes as f64 / 1024.0 / 1024.0
        )
    }
}
```

---

## 📁 文件结构规划

```
benches/
├── analysis_bench.rs              // 新增: 分析型基准
├── analyzer/                      // 新增: 分析器模块
│   ├── mod.rs
│   ├── performance_analyzer.rs    // 性能分析器
│   ├── bottleneck_detector.rs     // 瓶颈检测
│   └── metrics.rs                 // 指标定义
├── common/
│   ├── mod.rs
│   ├── data_generator.rs          // 已有
│   ├── bench_utils.rs             // 已有
│   ├── test_context.rs            // 已有
│   └── analysis_metrics.rs        // 新增: 分析指标
├── queries/                       // 新增: 查询集
│   ├── storage.gql
│   ├── query.gql
│   ├── transaction.gql
│   └── search.gql
├── results/                       // 新增: 结果输出
│   ├── baselines/
│   ├── analysis/
│   └── reports/
└── README.md                      // 已有
```

---

## 🚀 立即可执行的步骤

### 第 1 步: 创建分析查询文件 (10 分钟)

```bash
mkdir -p benches/queries

cat > benches/queries/storage.gql << 'EOF'
# Storage performance analysis
INSERT VERTEX Data(value) VALUES "v1"(1)

BEGIN
INSERT VERTEX Data(value) VALUES "v1"(1)
INSERT VERTEX Data(value) VALUES "v2"(2)
INSERT VERTEX Data(value) VALUES "v3"(3)
COMMIT
EOF

cat > benches/queries/query.gql << 'EOF'
# Query performance analysis
MATCH (n:Data) RETURN n
MATCH (n:Data)->(m:Data) RETURN n, m
MATCH (n:Data) WHERE n.value > 100 RETURN n
EOF
```

### 第 2 步: 创建基础分析模块 (30 分钟)

```bash
cat > benches/analyzer/mod.rs << 'EOF'
pub mod performance_analyzer;
pub mod metrics;
EOF

cat > benches/analyzer/metrics.rs << 'EOF'
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisMetrics {
    pub planning_time_ms: f64,
    pub execution_time_ms: f64,
    pub total_rows: usize,
    pub peak_memory_bytes: usize,
}

impl AnalysisMetrics {
    pub fn summary(&self) -> String {
        format!(
            "Planning: {:.2}ms | Execution: {:.2}ms | Rows: {} | Memory: {:.2}MB",
            self.planning_time_ms,
            self.execution_time_ms,
            self.total_rows,
            self.peak_memory_bytes as f64 / 1024.0 / 1024.0
        )
    }
}
EOF
```

### 第 3 步: 创建基础分析基准 (30 分钟)

```bash
cat > benches/analysis_bench.rs << 'EOF'
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

fn bench_analyze_storage(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis_storage");
    group.measurement_time(Duration::from_secs(5));
    
    group.bench_function("analyze_insert_1k", |b| {
        b.iter_custom(|_iters| {
            // TODO: 执行 EXPLAIN ANALYZE INSERT ... 1000 vertices
            // TODO: 提取指标
            Duration::from_millis(50)
        });
    });
    
    group.finish();
}

fn bench_analyze_query(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis_query");
    group.measurement_time(Duration::from_secs(5));
    
    group.bench_function("analyze_simple_match", |b| {
        b.iter_custom(|_iters| {
            // TODO: 执行 EXPLAIN ANALYZE MATCH (n:Data) RETURN n
            // TODO: 提取指标
            Duration::from_millis(5)
        });
    });
    
    group.finish();
}

criterion_group!(benches, bench_analyze_storage, bench_analyze_query);
criterion_main!(benches);
EOF
```

### 第 4 步: 更新 Cargo.toml (10 分钟)

```toml
# 在 [[bench]] 部分添加
[[bench]]
name = "analysis_bench"
harness = false

# 在 [dev-dependencies] 添加
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 第 5 步: 测试编译 (5 分钟)

```bash
cargo check --benches
cargo bench --bench analysis_bench -- --output-dir benches/results
```

---

## 💡 关键洞察

### 为什么集成分析功能？

| 问题 | Criterion.rs | EXPLAIN ANALYZE | PROFILE |
|------|-------------|-----------------|---------|
| 平均延迟? | ✅ | ✅ | ✅ |
| 查询优化时间? | ❌ | ✅ | ✅ |
| 执行计划? | ❌ | ✅ | ✅ |
| 节点级性能? | ❌ | ✅ | ✅ |
| 行数统计? | ❌ | ✅ | ✅ |
| 内存使用? | ❌ | ✅ | ✅ |
| 瓶颈识别? | ❌ | ✅ | ✅ |

### EXPLAIN ANALYZE vs PROFILE

```
EXPLAIN ANALYZE:
  ✅ 显示实际执行计划和统计
  ✅ 基于 PostgreSQL 标准
  ✅ 易于解析
  ⚠️ 需要自己提取指标

PROFILE:
  ✅ 专为性能分析设计
  ✅ 返回结构化数据
  ✅ 完整的节点级统计
  ⚠️ GraphDB 特定格式
```

**建议**: 开始使用 EXPLAIN ANALYZE，后续可迁移到 PROFILE

---

## 🔍 示例: 如何解析 EXPLAIN ANALYZE 输出

```
预期输出格式:
────────────────────────────────────────────
Explain Analyze for query: MATCH (n:Data) RETURN n
────────────────────────────────────────────
 id | name                           | output | execution_time | rows    | memory
────┼────────────────────────────────┼────────┼────────────────┼─────────┼──────
 0  | Project                        | [n]    | 0.45 ms        | 1000    | 256KB
 1  |  Filter                        | [n]    | 2.12 ms        | 500     | 128KB
 2  |   Scan                         | [n]    | 45.67 ms       | 1000    | 512KB
────┴────────────────────────────────┴────────┴────────────────┴─────────┴──────

Planning Time: 5.32 ms
Execution Time: 48.24 ms
Total Rows: 1000
Peak Memory: 512 KB
```

**解析逻辑**:
```rust
fn parse_explain_analyze(output: &str) -> AnalysisMetrics {
    // 1. 查找 "Planning Time:" 行，提取数字
    let planning_time = extract_value(output, "Planning Time:"); // 5.32
    
    // 2. 查找 "Execution Time:" 行，提取数字
    let execution_time = extract_value(output, "Execution Time:"); // 48.24
    
    // 3. 查找 "Total Rows:" 行，提取数字
    let total_rows = extract_value(output, "Total Rows:"); // 1000
    
    // 4. 查找 "Peak Memory:" 行，提取数字和单位
    let peak_memory = extract_memory(output, "Peak Memory:"); // 512 * 1024
    
    AnalysisMetrics {
        planning_time_ms: planning_time,
        execution_time_ms: execution_time,
        total_rows: total_rows as usize,
        peak_memory_bytes: peak_memory,
    }
}
```

---

## 📊 性能指标解读

### 规划时间过长 (>100ms)？

```
原因:
1. 查询复杂（多个 JOIN、子查询）
2. 优化器内部算法复杂度高
3. 统计信息收集耗时

优化方案:
1. 简化查询结构
2. 使用 HINT 指定执行计划
3. 添加 INDEX 减少搜索空间
```

### 执行时间过长 (>1000ms)？

```
原因:
1. 数据量大
2. 执行计划次优
3. I/O 瓶颈

优化方案:
1. 查看执行计划中最慢的节点
2. 检查是否有全表扫描
3. 优化索引策略
```

### 内存使用过高 (>100MB)？

```
原因:
1. 中间结果集过大
2. 未充分使用流式处理
3. 内存泄漏

优化方案:
1. 添加 LIMIT 限制行数
2. 优化 GROUP BY 操作
3. 使用 STREAMING 执行模式
```

---

## 📈 预期收益

| 方面 | 收益 |
|------|------|
| **性能优化** | 能够定位 95% 以上的性能瓶颈 |
| **回归检测** | 自动发现性能下降问题 |
| **文档改进** | 为用户提供详细的执行计划 |
| **架构优化** | 指导系统设计和优化决策 |
| **成本控制** | 评估资源使用效率 |

---

## ❓ 常见问题

**Q: EXPLAIN ANALYZE 会影响性能吗？**  
A: 会，因为它需要实际执行查询并收集统计。用小数据集测试。

**Q: 如何避免基准污染？**  
A: 为分析型基准使用独立的数据集，避免与性能基准混用。

**Q: 如何处理缓存对分析的影响？**  
A: 每次分析前清空缓存，或运行多次取平均值。

**Q: 支持比较多个版本吗？**  
A: 是，保存每个版本的基线 JSON，使用差异工具对比。

---

## 📞 需要帮助？

- 查看完整设计文档: `docs/tests/benches/benchmark_analysis_integration.md`
- 查看现有基准: `benches/{storage,query,transaction,search,api,end_to_end}_bench.rs`
- 查看 EXPLAIN 实现: `/crates/graphdb-query/src/query/executor/explain/`
- 查看示例查询: `tests/e2e/data/*.gql`

---

**状态**: ✅ 设计完成，可开始实施  
**预期工作量**: 2-3 周完成基础集成 + 1-2 周完成高级功能
