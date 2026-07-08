# GraphDB 基准测试分析功能集成指南

**完成日期**: 2026-06-18  
**目的**: 指导如何在性能基准测试中集成 EXPLAIN/PROFILE 分析功能，获取详细的性能指标

---

## 📋 执行总结

GraphDB 项目已有完整的 EXPLAIN/PROFILE 功能支持，包括：

- **EXPLAIN 语句**: 仅生成执行计划（不执行）
- **EXPLAIN ANALYZE**: 执行查询并收集统计信息
- **PROFILE 语句**: 执行查询并返回详细的性能数据
- **执行统计**: 行数、执行时间、内存使用、启动时间等

本文档说明如何将这些功能集成到现有基准测试框架中，建立三层分析体系。

---

## 🏗️ 现有的 EXPLAIN/PROFILE 架构

### 1. 核心数据结构

#### ExecutorStats (基础指标)
```rust
pub struct ExecutorStats {
    pub num_rows: usize,              // 处理的行数
    pub exec_time_us: u64,            // 执行时间（微秒）
    pub total_time_us: u64,           // 总时间（微秒）
    pub memory_peak: usize,           // 峰值内存
    pub memory_current: usize,        // 当前内存
    pub batch_count: usize,           // 批处理数
    pub other_stats: HashMap<String, String>, // 其他统计信息
}

// 方法
impl ExecutorStats {
    pub fn throughput_rows_per_sec(&self) -> f64 { ... }
    pub fn efficiency_rows_per_us(&self) -> f64 { ... }
}
```

#### NodeExecutionStats (节点级统计)
```rust
pub struct NodeExecutionStats {
    pub node_id: i64,                 // 计划节点ID
    pub executor_stats: ExecutorStats, // 执行统计
    pub startup_time_us: u64,         // 启动时间
}
```

#### GlobalExecutionStats (全局统计)
```rust
pub struct GlobalExecutionStats {
    pub planning_time_us: u64,        // 规划时间
    pub execution_time_us: u64,       // 执行时间
    pub total_rows: usize,            // 总行数
    pub peak_memory: usize,           // 峰值内存
    pub cache_hit_rate: f64,          // 缓存命中率
}
```

### 2. 主要执行器

| 执行器 | 位置 | 功能 |
|-------|------|------|
| `ExplainExecutor` | `/crates/graphdb-query/src/query/executor/explain/explain_executor.rs` | 执行 EXPLAIN 和 EXPLAIN ANALYZE |
| `ProfileExecutor` | `/crates/graphdb-query/src/query/executor/explain/profile_executor.rs` | 执行 PROFILE 语句，返回详细统计 |
| `InstrumentedExecutor` | `/crates/graphdb-query/src/query/executor/explain/instrumented_executor.rs` | 包装执行器，收集细粒度统计 |
| `ExecutionStatsContext` | `/crates/graphdb-query/src/query/executor/explain/execution_stats_context.rs` | 全局统计管理 |

### 3. 输出格式

```
位置: /crates/graphdb-query/src/query/executor/explain/format.rs

支持格式:
- Table:   人类可读的表格格式
- Dot:     Graphviz DOT 格式（用于可视化）
- Tree:    树形结构展示
```

---

## 🔄 三层基准测试分析体系

### 第一层: 基础性能基准 (现有)
```
目标: 测试操作延迟和吞吐量
当前实现: benches/{storage,query,transaction,search,api,end_to_end}_bench.rs
使用工具: Criterion.rs
指标: 平均时间、P95、P99
```

### 第二层: 分析型基准 (新增)
```
目标: 通过 EXPLAIN ANALYZE 获取执行计划和统计信息
实现: benches/analysis_bench.rs
使用工具: GraphDB EXPLAIN ANALYZE
指标: 规划时间、执行时间、行数、内存
```

### 第三层: 深度性能分析 (新增)
```
目标: 通过 PROFILE 语句进行深度性能分析
实现: benches/profile_bench.rs
使用工具: GraphDB PROFILE 语句
指标: 节点级统计、缓存命中率、启动时间
```

---

## 📊 指标收集体系

### A. 规划阶段指标

| 指标 | 含义 | 用途 |
|-----|------|------|
| `planning_time_us` | 查询优化耗时 | 评估优化器性能 |
| `plan_nodes_count` | 执行计划节点数 | 评估查询复杂度 |
| `optimization_rules_applied` | 应用的优化规则数 | 追踪优化过程 |

### B. 执行阶段指标

| 指标 | 含义 | 用途 |
|-----|------|------|
| `execution_time_us` | 执行耗时 | 评估执行效率 |
| `total_rows` | 总行数 | 评估数据量 |
| `startup_time_us` | 启动延迟 | 评估首字节延迟 |
| `peak_memory` | 峰值内存 | 评估内存效率 |

### C. 节点级指标

| 指标 | 含义 | 用途 |
|-----|------|------|
| `num_rows` | 节点输出行数 | 评估选择性 |
| `exec_time_us` | 节点执行时间 | 定位瓶颈 |
| `memory_peak` | 节点峰值内存 | 评估内存消耗 |
| `throughput_rows_per_sec` | 吞吐量 | 评估处理速度 |

### D. 缓存指标

| 指标 | 含义 | 用途 |
|-----|------|------|
| `cache_hit_rate` | 缓存命中率 | 评估缓存效率 |
| `plan_cache_hits` | 计划缓存命中 | 评估缓存策略 |
| `ctor_cache_hits` | CTE 缓存命中 | 评估子查询缓存 |

---

## 🔧 实现方案

### 方案 1: 分析型基准测试模块

创建 `benches/analysis_bench.rs`，在基准测试中运行 EXPLAIN ANALYZE：

```rust
use criterion::{criterion_group, criterion_main, Criterion};
use graphdb::api::client::GraphDBClient;
use std::time::Duration;

fn analyze_storage_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis_storage");
    group.measurement_time(Duration::from_secs(10));
    
    group.bench_function("analyze_batch_insert_1k", |b| {
        b.iter_custom(|iters| {
            let mut total_duration = Duration::ZERO;
            
            for _ in 0..iters {
                // 执行 EXPLAIN ANALYZE
                let query = "EXPLAIN ANALYZE INSERT VERTEX Data(value) VALUES ...";
                let result = client.execute(query).unwrap();
                
                // 提取指标
                let planning_time = extract_planning_time(&result);
                let execution_time = extract_execution_time(&result);
                let total_rows = extract_total_rows(&result);
                
                // 记录指标
                println!("Planning: {}ms, Execution: {}ms, Rows: {}", 
                    planning_time, execution_time, total_rows);
                
                total_duration += Duration::from_millis(execution_time as u64);
            }
            
            total_duration
        });
    });
    
    group.finish();
}

fn analyze_query_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("analysis_query");
    
    let queries = vec![
        "MATCH (n:Node) RETURN n",
        "MATCH (n:Node)->(m:Node) RETURN n, m",
        "MATCH (n:Node) WHERE n.value > 100 RETURN n",
    ];
    
    for query in queries {
        group.bench_function(
            format!("analyze_{}", query.replace(" ", "_")),
            |b| {
                b.iter_custom(|iters| {
                    let mut total_duration = Duration::ZERO;
                    
                    for _ in 0..iters {
                        let explain_query = format!("EXPLAIN ANALYZE {}", query);
                        let result = client.execute(&explain_query).unwrap();
                        
                        // 分析执行计划
                        let stats = parse_execution_stats(&result);
                        let bottleneck = identify_bottleneck(&stats);
                        
                        println!("Bottleneck: {:?}", bottleneck);
                        
                        total_duration += Duration::from_nanos(
                            stats.execution_time_us as u64 * 1000
                        );
                    }
                    
                    total_duration
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(benches, analyze_storage_operations, analyze_query_performance);
criterion_main!(benches);
```

### 方案 2: 深度性能分析器

创建 `benches/analyzer/performance_analyzer.rs`，提供可复用的分析工具：

```rust
use graphdb::api::client::GraphDBClient;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceAnalysisResult {
    // 规划阶段
    pub planning_time_ms: f64,
    pub plan_nodes_count: usize,
    
    // 执行阶段
    pub execution_time_ms: f64,
    pub startup_time_ms: f64,
    pub total_rows: usize,
    pub peak_memory_bytes: usize,
    
    // 节点级分析
    pub node_stats: Vec<NodeAnalysis>,
    
    // 缓存分析
    pub cache_hit_rate: f64,
    pub cache_memory_bytes: usize,
    
    // 瓶颈分析
    pub bottlenecks: Vec<Bottleneck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeAnalysis {
    pub node_id: i64,
    pub node_name: String,
    pub output_rows: usize,
    pub execution_time_ms: f64,
    pub memory_used_bytes: usize,
    pub throughput_rows_per_sec: f64,
    pub is_bottleneck: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Bottleneck {
    PlanningTime {
        time_ms: f64,
        severity: BottleneckSeverity,
    },
    ExecutionTime {
        node_id: i64,
        node_name: String,
        time_ms: f64,
        percentage: f64,
        severity: BottleneckSeverity,
    },
    MemoryUsage {
        peak_bytes: usize,
        severity: BottleneckSeverity,
    },
    LowThroughput {
        node_id: i64,
        rows_per_sec: f64,
        severity: BottleneckSeverity,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum BottleneckSeverity {
    Low,
    Medium,
    High,
    Critical,
}

pub struct PerformanceAnalyzer {
    client: GraphDBClient,
}

impl PerformanceAnalyzer {
    pub fn new(client: GraphDBClient) -> Self {
        Self { client }
    }
    
    /// 使用 EXPLAIN ANALYZE 分析查询
    pub async fn analyze_with_explain(&self, query: &str) 
        -> Result<PerformanceAnalysisResult> {
        let explain_query = format!("EXPLAIN ANALYZE {}", query);
        let result = self.client.execute(&explain_query).await?;
        
        self.parse_explain_result(&result)
    }
    
    /// 使用 PROFILE 进行深度分析
    pub async fn analyze_with_profile(&self, query: &str)
        -> Result<PerformanceAnalysisResult> {
        let profile_query = format!("PROFILE {}", query);
        let result = self.client.execute(&profile_query).await?;
        
        self.parse_profile_result(&result)
    }
    
    /// 识别性能瓶颈
    pub fn identify_bottlenecks(&self, analysis: &PerformanceAnalysisResult)
        -> Vec<Bottleneck> {
        let mut bottlenecks = vec![];
        
        // 规划时间瓶颈（>100ms）
        if analysis.planning_time_ms > 100.0 {
            bottlenecks.push(Bottleneck::PlanningTime {
                time_ms: analysis.planning_time_ms,
                severity: if analysis.planning_time_ms > 500.0 {
                    BottleneckSeverity::Critical
                } else if analysis.planning_time_ms > 300.0 {
                    BottleneckSeverity::High
                } else {
                    BottleneckSeverity::Medium
                },
            });
        }
        
        // 执行时间瓶颈
        let total_exec_time: f64 = analysis.node_stats.iter()
            .map(|n| n.execution_time_ms)
            .sum();
        
        for node in &analysis.node_stats {
            let percentage = (node.execution_time_ms / total_exec_time) * 100.0;
            
            if percentage > 20.0 {
                bottlenecks.push(Bottleneck::ExecutionTime {
                    node_id: node.node_id,
                    node_name: node.node_name.clone(),
                    time_ms: node.execution_time_ms,
                    percentage,
                    severity: if percentage > 60.0 {
                        BottleneckSeverity::Critical
                    } else if percentage > 40.0 {
                        BottleneckSeverity::High
                    } else {
                        BottleneckSeverity::Medium
                    },
                });
            }
        }
        
        // 内存使用瓶颈
        if analysis.peak_memory_bytes > 100 * 1024 * 1024 {
            bottlenecks.push(Bottleneck::MemoryUsage {
                peak_bytes: analysis.peak_memory_bytes,
                severity: if analysis.peak_memory_bytes > 500 * 1024 * 1024 {
                    BottleneckSeverity::Critical
                } else if analysis.peak_memory_bytes > 300 * 1024 * 1024 {
                    BottleneckSeverity::High
                } else {
                    BottleneckSeverity::Medium
                },
            });
        }
        
        // 低吞吐量瓶颈
        for node in &analysis.node_stats {
            if node.throughput_rows_per_sec < 1000.0 && node.output_rows > 100 {
                bottlenecks.push(Bottleneck::LowThroughput {
                    node_id: node.node_id,
                    rows_per_sec: node.throughput_rows_per_sec,
                    severity: if node.throughput_rows_per_sec < 100.0 {
                        BottleneckSeverity::Critical
                    } else if node.throughput_rows_per_sec < 500.0 {
                        BottleneckSeverity::High
                    } else {
                        BottleneckSeverity::Medium
                    },
                });
            }
        }
        
        bottlenecks
    }
    
    /// 生成详细分析报告
    pub fn generate_report(&self, analysis: &PerformanceAnalysisResult) -> String {
        let mut report = String::new();
        
        report.push_str("=== Performance Analysis Report ===\n\n");
        
        report.push_str("Planning Phase:\n");
        report.push_str(&format!("  Planning Time: {:.2}ms\n", analysis.planning_time_ms));
        report.push_str(&format!("  Plan Nodes: {}\n\n", analysis.plan_nodes_count));
        
        report.push_str("Execution Phase:\n");
        report.push_str(&format!("  Execution Time: {:.2}ms\n", analysis.execution_time_ms));
        report.push_str(&format!("  Startup Time: {:.2}ms\n", analysis.startup_time_ms));
        report.push_str(&format!("  Total Rows: {}\n", analysis.total_rows));
        report.push_str(&format!("  Peak Memory: {:.2}MB\n\n", 
            analysis.peak_memory_bytes as f64 / 1024.0 / 1024.0));
        
        report.push_str("Node Analysis:\n");
        for node in &analysis.node_stats {
            report.push_str(&format!(
                "  Node {} ({}): {} rows, {:.2}ms, {:.0} rows/sec\n",
                node.node_id, node.node_name, node.output_rows,
                node.execution_time_ms, node.throughput_rows_per_sec
            ));
        }
        report.push_str("\n");
        
        report.push_str("Cache Analysis:\n");
        report.push_str(&format!("  Cache Hit Rate: {:.2}%\n", 
            analysis.cache_hit_rate * 100.0));
        report.push_str(&format!("  Cache Memory: {:.2}MB\n\n", 
            analysis.cache_memory_bytes as f64 / 1024.0 / 1024.0));
        
        report.push_str("Bottlenecks:\n");
        if analysis.bottlenecks.is_empty() {
            report.push_str("  No significant bottlenecks detected\n");
        } else {
            for bottleneck in &analysis.bottlenecks {
                report.push_str(&format!("  - {:?}\n", bottleneck));
            }
        }
        
        report
    }
    
    fn parse_explain_result(&self, result: &str) 
        -> Result<PerformanceAnalysisResult> {
        // 解析 EXPLAIN ANALYZE 输出
        // 实现细节待完成
        todo!()
    }
    
    fn parse_profile_result(&self, result: &str)
        -> Result<PerformanceAnalysisResult> {
        // 解析 PROFILE 输出
        // 实现细节待完成
        todo!()
    }
}
```

### 方案 3: 基准测试扩展模块

创建 `benches/common/analysis_metrics.rs`，定义可复用的指标收集模块：

```rust
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkAnalysisMetrics {
    /// 规划时间（毫秒）
    pub planning_time_ms: f64,
    
    /// 执行时间（毫秒）
    pub execution_time_ms: f64,
    
    /// 启动延迟（毫秒）
    pub startup_time_ms: f64,
    
    /// 处理的总行数
    pub total_rows: usize,
    
    /// 峰值内存使用（字节）
    pub peak_memory_bytes: usize,
    
    /// 吞吐量（行/秒）
    pub throughput: f64,
    
    /// 缓存命中率（0-1）
    pub cache_hit_rate: f64,
    
    /// 执行计划复杂度
    pub plan_complexity: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonResult {
    /// 基线指标
    pub baseline: BenchmarkAnalysisMetrics,
    
    /// 当前指标
    pub current: BenchmarkAnalysisMetrics,
    
    /// 偏差百分比
    pub deviations: HashMap<String, f64>,
    
    /// 是否有回归
    pub has_regression: bool,
    
    /// 回归列表
    pub regressions: Vec<String>,
}

impl BenchmarkAnalysisMetrics {
    /// 计算性能得分（0-100）
    pub fn calculate_score(&self) -> f64 {
        let mut score = 100.0;
        
        // 规划时间过长扣分
        if self.planning_time_ms > 100.0 {
            score -= (self.planning_time_ms / 10.0).min(20.0);
        }
        
        // 执行时间过长扣分
        if self.execution_time_ms > 1000.0 {
            score -= (self.execution_time_ms / 100.0).min(20.0);
        }
        
        // 启动延迟过高扣分
        if self.startup_time_ms > 50.0 {
            score -= (self.startup_time_ms / 10.0).min(10.0);
        }
        
        // 内存使用过多扣分
        if self.peak_memory_bytes > 1024 * 1024 * 100 {
            score -= ((self.peak_memory_bytes as f64 / (1024.0 * 1024.0)) / 10.0).min(15.0);
        }
        
        // 吞吐量低扣分
        if self.throughput < 1000.0 {
            score -= ((1000.0 - self.throughput) / 100.0).min(15.0);
        }
        
        score.max(0.0)
    }
    
    /// 生成人类可读的总结
    pub fn summary(&self) -> String {
        format!(
            "Planning: {:.2}ms | Execution: {:.2}ms | Rows: {} | Memory: {:.2}MB | Throughput: {:.0} rows/sec | Score: {:.1}",
            self.planning_time_ms,
            self.execution_time_ms,
            self.total_rows,
            self.peak_memory_bytes as f64 / 1024.0 / 1024.0,
            self.throughput,
            self.calculate_score()
        )
    }
}

impl ComparisonResult {
    /// 创建对比结果
    pub fn new(baseline: BenchmarkAnalysisMetrics, current: BenchmarkAnalysisMetrics) -> Self {
        let mut deviations = HashMap::new();
        let mut regressions = vec![];
        
        // 规划时间对比
        let planning_deviation = 
            ((current.planning_time_ms - baseline.planning_time_ms) / baseline.planning_time_ms) * 100.0;
        deviations.insert("planning_time".to_string(), planning_deviation);
        
        if planning_deviation > 10.0 {
            regressions.push(format!(
                "Planning time regression: {:.1}% ({:.2}ms -> {:.2}ms)",
                planning_deviation, baseline.planning_time_ms, current.planning_time_ms
            ));
        }
        
        // 执行时间对比
        let execution_deviation = 
            ((current.execution_time_ms - baseline.execution_time_ms) / baseline.execution_time_ms) * 100.0;
        deviations.insert("execution_time".to_string(), execution_deviation);
        
        if execution_deviation > 10.0 {
            regressions.push(format!(
                "Execution time regression: {:.1}% ({:.2}ms -> {:.2}ms)",
                execution_deviation, baseline.execution_time_ms, current.execution_time_ms
            ));
        }
        
        // 内存对比
        let memory_deviation = 
            ((current.peak_memory_bytes as i64 - baseline.peak_memory_bytes as i64) as f64 / baseline.peak_memory_bytes as f64) * 100.0;
        deviations.insert("memory".to_string(), memory_deviation);
        
        if memory_deviation > 20.0 {
            regressions.push(format!(
                "Memory usage regression: {:.1}% ({:.2}MB -> {:.2}MB)",
                memory_deviation, 
                baseline.peak_memory_bytes as f64 / 1024.0 / 1024.0,
                current.peak_memory_bytes as f64 / 1024.0 / 1024.0
            ));
        }
        
        let has_regression = !regressions.is_empty();
        
        Self {
            baseline,
            current,
            deviations,
            has_regression,
            regressions,
        }
    }
    
    /// 生成对比报告
    pub fn report(&self) -> String {
        let mut report = String::new();
        
        report.push_str("=== Performance Comparison Report ===\n\n");
        
        report.push_str("Baseline:\n");
        report.push_str(&format!("  {}\n\n", self.baseline.summary()));
        
        report.push_str("Current:\n");
        report.push_str(&format!("  {}\n\n", self.current.summary()));
        
        if self.has_regression {
            report.push_str("⚠️ Regressions Detected:\n");
            for regression in &self.regressions {
                report.push_str(&format!("  - {}\n", regression));
            }
        } else {
            report.push_str("✅ No regressions detected\n");
        }
        
        report
    }
}
```

---

## 🚀 集成步骤

### 步骤 1: 准备 EXPLAIN/PROFILE 查询集

```bash
# 创建标准查询文件
mkdir -p benches/queries/

# 存储操作查询
cat > benches/queries/storage_queries.gql << 'EOF'
# Single vertex insert
INSERT VERTEX Data(value) VALUES "v1"(1)

# Batch vertices insert
BEGIN
INSERT VERTEX Data(value) VALUES "v1"(1)
INSERT VERTEX Data(value) VALUES "v2"(2)
INSERT VERTEX Data(value) VALUES "v3"(3)
COMMIT

# Single edge insert
INSERT EDGE Connect() VALUES "v1"->"v2"()
EOF

# 查询查询
cat > benches/queries/query_queries.gql << 'EOF'
# Simple vertex query
MATCH (n:Data) RETURN n

# Path query
MATCH (n:Data)->(m:Data) RETURN n, m

# Aggregation query
MATCH (n:Data) RETURN COUNT(n), AVG(n.value)
EOF

# 事务查询
cat > benches/queries/transaction_queries.gql << 'EOF'
# Begin-commit transaction
BEGIN
INSERT VERTEX Data(value) VALUES "v1"(1)
INSERT VERTEX Data(value) VALUES "v2"(2)
COMMIT

# Rollback transaction
BEGIN
INSERT VERTEX Data(value) VALUES "v1"(1)
ROLLBACK
EOF
```

### 步骤 2: 创建分析基准测试

```bash
cat > benches/analysis_bench.rs << 'EOF'
use criterion::{criterion_group, criterion_main, Criterion};
use std::time::Duration;

// 在这里添加分析型基准测试
// 使用 EXPLAIN ANALYZE 语句

criterion_group!(benches, analyze_storage, analyze_query, analyze_transaction);
criterion_main!(benches);
EOF
```

### 步骤 3: 创建性能分析器

```bash
mkdir -p benches/analyzer/
cat > benches/analyzer/mod.rs << 'EOF'
pub mod performance_analyzer;
pub mod metrics;
EOF

cat > benches/analyzer/performance_analyzer.rs << 'EOF'
// 实现 PerformanceAnalyzer（见上面的代码样本）
EOF

cat > benches/analyzer/metrics.rs << 'EOF'
// 实现 BenchmarkAnalysisMetrics 和 ComparisonResult（见上面的代码样本）
EOF
```

### 步骤 4: 更新 Cargo.toml

```toml
# 添加分析基准
[[bench]]
name = "analysis_bench"
harness = false

# 添加依赖
[dev-dependencies]
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
```

### 步骤 5: 集成到现有基准

修改现有的基准文件，添加分析功能：

```rust
// 在 benches/query_bench.rs 中添加：

fn analyze_simple_query_performance(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_analysis");
    
    group.bench_function("analyze_simple_match_query", |b| {
        b.iter_custom(|_iters| {
            let query = "MATCH (n:Node) RETURN n";
            let explain_query = format!("EXPLAIN ANALYZE {}", query);
            
            // 执行 EXPLAIN ANALYZE
            let result = execute_query(&explain_query);
            
            // 提取和分析指标
            let metrics = parse_metrics(&result);
            
            // 记录到文件
            save_metrics("query_analysis_simple.json", &metrics);
            
            Duration::from_nanos(metrics.execution_time_ms as u64 * 1_000_000)
        });
    });
    
    group.finish();
}
```

---

## 📈 指标分析示例

### 示例 1: 查询性能分析

```
Query: MATCH (n:Node)->(m:Node) RETURN n, m
───────────────────────────────────────────────

Planning Phase: 5.32ms
  - Parser: 0.12ms
  - Validator: 0.45ms
  - Optimizer: 4.75ms
  - Plan Nodes: 8

Execution Phase: 142.56ms
  - Startup Time: 0.23ms
  - Total Rows: 125,000
  - Peak Memory: 245.32MB

Node Analysis:
  Node 0 (Scan): 100,000 rows, 45.23ms, 2.21M rows/sec
  Node 1 (Filter): 50,000 rows, 23.12ms, 2.16M rows/sec
  Node 2 (Edge Traversal): 125,000 rows, 65.42ms, 1.91M rows/sec
  Node 3 (Return): 125,000 rows, 8.79ms, 14.22M rows/sec

Bottlenecks:
  ⚠️ High: Node 2 (Edge Traversal) takes 45.9% of execution time
  ⚠️ Medium: Peak memory usage 245.32MB (threshold: 200MB)
```

### 示例 2: 基线对比

```
Benchmark: Storage Batch Insert (1000 vertices)
────────────────────────────────────────────────

Baseline (v1.0):
  Planning: 1.23ms | Execution: 45.67ms | Rows: 1000 | Memory: 12.45MB | Score: 92.3

Current (v1.1):
  Planning: 1.45ms | Execution: 43.21ms | Rows: 1000 | Memory: 14.32MB | Score: 91.2

Changes:
  ✅ Execution time: -4.9% (improvement)
  ⚠️ Planning time: +17.9% (regression)
  ⚠️ Memory: +15.0% (regression)

Overall: 🟡 Minor regression detected
```

---

## 🔍 高级分析技巧

### 1. 识别 CPU 瓶颈

```rust
pub fn identify_cpu_bottleneck(analysis: &PerformanceAnalysisResult) {
    // 找到执行时间最长的节点
    let slowest_node = analysis.node_stats
        .iter()
        .max_by(|a, b| a.execution_time_ms.partial_cmp(&b.execution_time_ms).unwrap());
    
    if let Some(node) = slowest_node {
        eprintln!("CPU Bottleneck: {} ({}ms, {:.1}% of total)",
            node.node_name,
            node.execution_time_ms,
            (node.execution_time_ms / analysis.execution_time_ms) * 100.0
        );
    }
}
```

### 2. 识别内存瓶颈

```rust
pub fn identify_memory_bottleneck(analysis: &PerformanceAnalysisResult) {
    // 找到内存使用最多的节点
    let memory_intensive = analysis.node_stats
        .iter()
        .max_by(|a, b| a.memory_used_bytes.cmp(&b.memory_used_bytes));
    
    if let Some(node) = memory_intensive {
        eprintln!("Memory Bottleneck: {} ({:.2}MB, {:.1}% of total)",
            node.node_name,
            node.memory_used_bytes as f64 / 1024.0 / 1024.0,
            (node.memory_used_bytes as f64 / analysis.peak_memory_bytes as f64) * 100.0
        );
    }
}
```

### 3. 选择性分析

```rust
pub fn analyze_selectivity(analysis: &PerformanceAnalysisResult) {
    for node in &analysis.node_stats {
        if let Some(prev_node) = analysis.node_stats.iter()
            .find(|n| n.node_id == node.node_id - 1) {
            
            let selectivity = node.output_rows as f64 / prev_node.output_rows as f64;
            
            if selectivity > 0.9 {
                eprintln!("⚠️ Low selectivity at {}: {:.1}%", 
                    node.node_name, selectivity * 100.0);
            }
        }
    }
}
```

### 4. 扩展性分析

```rust
pub fn analyze_scalability(
    small_data: &PerformanceAnalysisResult,
    large_data: &PerformanceAnalysisResult,
) {
    let size_ratio = large_data.total_rows as f64 / small_data.total_rows as f64;
    let time_ratio = large_data.execution_time_ms / small_data.execution_time_ms;
    
    let complexity = match time_ratio / size_ratio {
        r if r < 1.1 => "O(n)",
        r if r < 1.5 => "O(n log n)",
        r if r < 3.0 => "O(n^1.5)",
        _ => "O(n^2) or worse",
    };
    
    eprintln!("Scalability: Size {}x → Time {}x → Complexity {}", 
        size_ratio as i32, time_ratio as i32, complexity);
}
```

---

## 📊 输出和报告

### 生成 JSON 报告

```bash
cargo bench --release -- --output-dir benches/analysis_results
# 输出位置: benches/analysis_results/analysis_*.json
```

### 生成 HTML 报告

```bash
# 创建报告生成器
python3 scripts/generate_analysis_report.py \
    --input benches/analysis_results/ \
    --output target/analysis_report.html
```

### 生成 Markdown 报告

```bash
cat > scripts/generate_markdown_report.sh << 'EOF'
#!/bin/bash

echo "# GraphDB Performance Analysis Report" > analysis_report.md
echo "Generated: $(date)" >> analysis_report.md
echo "" >> analysis_report.md

# 合并所有 JSON 结果
for f in benches/analysis_results/*.json; do
    echo "## $(basename $f)" >> analysis_report.md
    cat "$f" >> analysis_report.md
    echo "" >> analysis_report.md
done
EOF
```

---

## ✅ 验收标准

- [x] GraphDB 已支持 EXPLAIN ANALYZE 和 PROFILE
- [x] 执行统计结构已定义（ExecutorStats, NodeExecutionStats, GlobalExecutionStats）
- [x] 分析型基准测试设计完成
- [x] 性能分析器框架设计完成
- [x] 指标收集模块设计完成
- [x] 集成步骤清晰
- [x] 示例和高级技巧提供

---

## 📝 后续实施任务

### Phase 1: 核心集成 (1-2周)
- [ ] 创建 `benches/analysis_bench.rs` 模块
- [ ] 创建 `benches/analyzer/` 目录和模块
- [ ] 实现 `PerformanceAnalyzer` 基本功能
- [ ] 集成 EXPLAIN ANALYZE 查询

### Phase 2: 指标收集 (1周)
- [ ] 实现指标提取逻辑
- [ ] 创建 JSON 序列化支持
- [ ] 实现基线保存和对比

### Phase 3: 高级分析 (1-2周)
- [ ] 实现瓶颈自动识别
- [ ] 添加扩展性分析
- [ ] 生成详细报告

### Phase 4: 可视化和CI集成 (2周)
- [ ] 创建报告生成工具
- [ ] 集成到 CI/CD 流程
- [ ] 性能监控仪表板

---

## 🔗 相关资源

### GraphDB 代码位置
- EXPLAIN 执行器: `/crates/graphdb-query/src/query/executor/explain/explain_executor.rs`
- PROFILE 执行器: `/crates/graphdb-query/src/query/executor/explain/profile_executor.rs`
- 执行统计: `/crates/graphdb-core/src/core/stats/executor_stats.rs`
- 格式化: `/crates/graphdb-query/src/query/executor/explain/format.rs`

### 现有文档
- `docs/tests/benches/performance_benchmark_plan.md` - 性能规划
- `docs/tests/benches/benchmark_implementation.md` - 实施细节
- `benches/README.md` - 基准使用指南

---

**完成度**: 分析和设计完成，可开始实施  
**下一步**: 根据 Phase 1 计划开始编码实现
