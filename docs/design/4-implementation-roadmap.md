# 实施路线图与计划

> 更新日期：2026-06-27
> 总周期：9-12 周（取决于团队规模）
> 核心：四个阶段的清晰依赖关系

---

## 1. 四个阶段的高层概览

```
┌──────────────────────────────────────────────────────┐
│ 阶段 0（1 周）：基础架构搭建                         │
│ - StreamingExecutor enum 完整定义                   │
│ - ExecutionMode 支持                               │
│ - PlanExecutor 改造（两条执行路径）                │
│ - 基准测试框架                                     │
│ 依赖：无                                           │
│ 前置条件：无                                       │
└──────────────────────────────────────────────────────┘
                    ↓ (必须完成)
┌──────────────────────────────────────────────────────┐
│ 阶段 1（4-6 周）：并行执行框架（与阶段 2 并行）    │
│ - 数据分区架构                                     │
│ - Pipeline 调度器框架                             │
│ - 与 StreamingExecutor 集成                       │
│ 依赖：阶段 0                                       │
│ 目标：LIMIT 10-50x 加速                          │
└──────────────────────────────────────────────────────┘
        ↓ (与阶段 2 并行)
┌──────────────────────────────────────────────────────┐
│ 阶段 2（2-3 周）：关键 executor 流式化              │
│ - Scan（数据源）                                  │
│ - Filter、Project（单输入处理）                   │
│ - Limit（消费端）                                │
│ 依赖：阶段 0                                       │
│ 目标：稳定的流式系统                              │
└──────────────────────────────────────────────────────┘
        ↓ (独立，可与 1/2 并行)
┌──────────────────────────────────────────────────────┐
│ 阶段 3（2-3 周）：存储优化（P1.c）                 │
│ - 列级压缩选择器                                  │
│ - 并行 HashJoin                                   │
│ 依赖：无（但推荐在阶段 1 后进行）                 │
│ 目标：存储减少 30-70%，Join 加速 6-7x            │
└──────────────────────────────────────────────────────┘

总周期：9-12 周（3 人团队：9 周；2 人团队：11 周；1 人团队：12 周）
```

---

## 2. 阶段 0：基础架构搭建（1 周）

### 2.1 目标

建立 StreamingExecutor 的完整基础，使后续阶段可以逐步迁移具体的 executor。

### 2.2 工作内容

| 任务 | 工作量 | 开发者 | 优先级 |
|------|--------|--------|--------|
| StreamingExecutor enum 定义 + 基础实现 | 2 天 | A | P0 |
| DataChunk 和相关数据结构 | 1 天 | A | P0 |
| ExecutionMode 枚举和 PlanExecutor 改造 | 2 天 | B | P0 |
| 基准测试框架 + 性能监控 | 2 天 | C | P0 |
| 文档 + 代码审查 | 1 天 | 全部 | P1 |

### 2.3 文件清单

**新增文件**：
- `crates/graphdb-query/src/executor/streaming/mod.rs`
- `crates/graphdb-query/src/executor/streaming/executor.rs`（StreamingExecutor enum）
- `crates/graphdb-query/src/executor/streaming/chunk.rs`（DataChunk）
- `crates/graphdb-query/src/executor/streaming/base.rs`（StreamingBaseExecutor）

**修改文件**：
- `crates/graphdb-query/src/executor/factory/engine.rs`
  - 新增 `ExecutionMode` enum
  - 新增 `execute_streaming()` 方法
  - 修改 `execute_plan()` 签名

### 2.4 验收标准

- [ ] StreamingExecutor enum 编译通过，无 warnings
- [ ] PlanExecutor 支持两种模式，向后兼容
- [ ] 基准测试框架可用（性能基准已建立）
- [ ] 文档完整，架构清晰

---

## 3. 阶段 1：并行执行框架（4-6 周）

### 3.1 目标

实现数据分区和 Pipeline 调度器，为多核执行提供基础。不依赖阶段 2 的具体 executor，但需要阶段 0 的 StreamingExecutor 基础。

### 3.2 工作内容

| 任务 | 工作量 | 优先级 |
|------|--------|--------|
| 数据分区接口 + PartitionView | 3 天 | P0 |
| Pipeline 调度器框架 | 3 天 | P0 |
| 线程池和背压机制 | 2 天 | P0 |
| 与 QueryPipelineManager 集成 | 2 天 | P0 |
| 基准测试和集成测试 | 3 天 | P1 |
| 优化和文档 | 2 天 | P2 |

### 3.3 开发分工（推荐 3 人）

**Developer A - 分区架构**：
- 周 1：VertexTable/EdgeTable 分区接口
- 周 2：PartitionView 实现和测试

**Developer B - 调度器**：
- 周 2-3：Pipeline 调度器框架
- 周 3-4：线程池和背压机制
- 周 4：与 PlanExecutor 集成

**Developer C - 测试和验证**：
- 周 2-6：基准测试（LIMIT、全表、WHERE）
- 周 5-6：性能优化建议
- 周 6：文档和总结

### 3.4 里程碑

| 里程碑 | 时间 | 验收标准 |
|--------|------|---------|
| **M1：分区架构完成** | 周 2 | PartitionView 可用，分区接口测试通过 |
| **M2：调度器可用** | 周 4 | 线程池运行，能获取 chunk |
| **M3：集成完成** | 周 5 | 与 QueryPipelineManager 链接，基本可执行 |
| **M4：性能验证** | 周 6 | LIMIT 10-50x，全表 7x 加速 |

### 3.5 验收标准

- [ ] LIMIT 查询：10-50x 加速（相比单线程）
- [ ] 全表扫描：7x 加速（8 核）
- [ ] 内存占用恒定（~4MB/chunk）
- [ ] 所有集成测试通过
- [ ] 文档完整，架构清晰

---

## 4. 阶段 2：关键 executor 流式化（2-3 周）

### 4.1 目标

将最常用的 executor 迁移到 StreamingExecutor，支持更多查询的流式执行。与阶段 1 并行进行。

### 4.2 工作内容和优先级

**优先级 1（必须，周 1）**：
- `StreamingScanVertices` / `StreamingScanEdges`
  - 难度：低
  - 原因：数据源，无依赖
  - 工作量：1 天

- `StreamingLimit`
  - 难度：低
  - 原因：LIMIT 优化的关键
  - 工作量：1 天

**优先级 2（周 2）**：
- `StreamingFilter`
  - 难度：低
  - 原因：高频，单输入，无状态
  - 工作量：1 天

- `StreamingProject`
  - 难度：低
  - 原因：常见，单输入，无状态
  - 工作量：1 天

**优先级 3（可选，周 3）**：
- `StreamingAggregate`
  - 难度：中
  - 原因：有状态，需要全量输入
  - 工作量：1.5 天

### 4.3 文件清单

**新增文件**：
- `crates/graphdb-query/src/executor/streaming/impl/scan.rs`
- `crates/graphdb-query/src/executor/streaming/impl/filter.rs`
- `crates/graphdb-query/src/executor/streaming/impl/project.rs`
- `crates/graphdb-query/src/executor/streaming/impl/limit.rs`
- `crates/graphdb-query/src/executor/streaming/impl/aggregate.rs`（可选）

**修改文件**：
- `crates/graphdb-query/src/executor/factory/executor_factory.rs`
  - 新增 `create_streaming_executor()` 方法

### 4.4 实现模式

每个 executor 在 StreamingExecutor enum 中添加一个 variant，并在 open/next/close 方法中实现：

```rust
pub enum StreamingExecutor {
    // ...
    Filter { input, condition, opened },
    // ...
}

impl StreamingExecutor {
    pub fn next(&mut self) -> DBResult<Option<DataChunk>> {
        match self {
            // ...
            Self::Filter { input, condition, .. } => {
                if let Some(chunk) = input.next()? {
                    let filtered = chunk.rows
                        .into_iter()
                        .filter(|row| condition.evaluate(row)?)
                        .collect();
                    Ok(Some(DataChunk::from_rows(filtered)))
                } else {
                    Ok(None)
                }
            }
        }
    }
}
```

### 4.5 验收标准

- [ ] StreamingScan 通过所有数据源测试
- [ ] StreamingFilter 通过过滤功能测试
- [ ] StreamingProject 通过投影测试
- [ ] StreamingLimit 通过 LIMIT 功能测试
- [ ] 流式执行结果与全物化执行完全等价
- [ ] 内存占用验证（~4MB per chunk）
- [ ] 集成测试全部通过

---

## 5. 阶段 3：存储优化（2-3 周）

### 5.1 目标

优化存储引擎，进一步提升性能和容量。可与阶段 1/2 并行，但推荐在之后进行。

### 5.2 工作内容

**列级压缩选择器**（1.5 周）：
- 分析列数据特征
- 选择最优压缩算法
- 动态适应不同数据类型

**并行 HashJoin**（1.5 周）：
- 构建阶段并行化
- 探测阶段并行化
- 背压处理

### 5.3 文件

- `crates/graphdb-storage/src/compression/selector.rs`
- `crates/graphdb-query/src/executor/streaming/impl/hash_join.rs`

### 5.4 验收标准

- [ ] 存储节省 30-70%
- [ ] Join 加速 6-7 倍
- [ ] 压缩选择正确性验证

---

## 6. 并行方案

### 6.1 方案 A：3 人团队（推荐）- 总 9 周

```
时间 ↓  | Developer A      | Developer B       | Developer C
--------|------------------|-------------------|------------
W1     | 架构重构         | 架构重构          | 基准框架
W2-3   | 数据分区         | Pipeline 调度     | 测试用例
W4-5   | Scan + Filter    | 调度器集成        | 性能验证
W6     | Limit + Project  | 背压机制          | 文档

并行W7-8 | 压缩选择器       | 并行 Join         | -
```

**优点**：
- 充分利用并行化
- 每个人专注于具体任务
- 进度快

**缺点**：
- 需要 3 个人

### 6.2 方案 B：2 人团队 - 总 11 周

```
时间 ↓  | Developer A       | Developer B
--------|-------------------|-------------------
W1     | 架构重构          | 基准框架
W2-3   | 数据分区          | Pipeline 调度器
W4-5   | Scan/Filter/Limit | 集成测试
W6-7   | 压缩选择器        | 并行 Join
W8     | 优化              | 文档
```

### 6.3 方案 C：1 人团队 - 总 12 周

```
时间 ↓  | 开发者
--------|----------
W1     | 架构重构
W2-4   | 数据分区 + Pipeline
W5-6   | Scan/Filter/Limit
W7     | 压缩选择器
W8     | 并行 Join
W9     | 优化和文档
```

---

## 7. 关键里程碑

| 里程碑 | 时间 | 工作项 | 验收标准 |
|--------|------|--------|---------|
| **M1：架构完成** | W1 | 阶段 0 | StreamingExecutor 可用 |
| **M2：并行框架可用** | W6 | 阶段 1 | LIMIT 100x 加速 |
| **M3：核心 executor 迁移** | W8 | 阶段 2 | Scan/Filter/Limit 流式化 |
| **M4：流式系统稳定** | W9 | 阶段 1+2 | 执行结果等价，内存恒定 |
| **M5：存储优化完成** | W12 | 阶段 3 | 存储减少 30-70%，Join 6-7x |

---

## 8. 风险评估与缓解

| 风险 | 概率 | 影响 | 缓解方案 |
|------|------|------|---------|
| **阶段 0 架构复杂度高** | 中 | 延期 | 提前充分评审；搭建测试框架 |
| **调度器集成困难** | 中 | 阶段 1 延期 | 与 PlanExecutor 团队紧密协作 |
| **executor 迁移遇见预期外问题** | 中 | 阶段 2 延期 | 优先级分明；可选的 executor 可延后 |
| **性能达不到目标** | 低 | 需要优化 | 提前基准测试；逐步验证 |
| **向后兼容性问题** | 低 | 需要修复 | ExecutionMode 完全隔离；充分测试 |

---

## 9. 资源需求

### 9.1 人力

| 规模 | 总周期 | 密度 | 推荐度 |
|------|--------|------|--------|
| **1 人** | 12 周 | 低 | ⚠️ 较慢 |
| **2 人** | 11 周 | 中 | ✅ 可接受 |
| **3 人** | 9 周 | 高 | ✅ 推荐 |

### 9.2 基础设施

- 基准测试环境（性能监控）
- CI/CD 流水线（自动化测试）
- 代码审查工具
- 文档平台

---

## 10. 过渡策略

### 10.1 ExecutionMode 控制

```rust
// 默认向后兼容
pub fn execute_query(&self, query: &str) -> DBResult<ExecutionResult> {
    self.execute_query_with_mode(query, ExecutionMode::Materialized)
}

// 显式选择流式
pub fn execute_query_streaming(&self, query: &str) -> DBResult<ExecutionResult> {
    self.execute_query_with_mode(query, ExecutionMode::Streaming)
}

// 环境变量控制
let mode = std::env::var("LINKRS_EXECUTION_MODE")
    .unwrap_or("materialized".to_string());
```

### 10.2 上线计划

| 时期 | Materialized | Streaming | 描述 |
|------|------------|-----------|------|
| **兼容期** | 默认 | 可选 | 新系统在开发/测试 |
| **双系统** | 可选 | 推荐 | 新系统充分验证 |
| **流式优先** | 备选 | 默认 | 新系统已稳定 |

---

## 11. 文档和交付

### 11.1 代码交付物

- StreamingExecutor enum 完整实现
- 各个 executor 的流式版本
- Pipeline 调度器
- 分区接口
- 完整的单元和集成测试

### 11.2 文档交付物

- 架构设计文档
- 实现指南
- API 文档
- 性能基准报告
- 迁移指南

---

## 总结

- ✅ **四阶段清晰**：0 → 1+2(并行) → 3
- ✅ **依赖关系明确**：阶段 0 是基础，后续可并行
- ✅ **灵活的人力方案**：1-3 人，总周期 9-12 周
- ✅ **充分的风险评估**：提前识别和缓解
- ✅ **完整的交付物**：代码 + 文档 + 测试

