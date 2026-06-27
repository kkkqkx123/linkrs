# P1 实施路线图（更新版）

> 更新日期：2026-06-27
> 核心改变：引入 Executor 架构重构为 P1 的基础

---

## 1. 架构改进的完整依赖关系

```
第 0 阶段（1 周）：Executor 架构重构（必须先做）
  ├─ StreamingExecutor trait 定义
  ├─ StreamingBaseExecutor 实现
  ├─ 决策：使用 Box<dyn StreamingExecutor>（完全动态分发）
  ├─ PlanExecutor 支持 ExecutionMode（Materialized vs Streaming）
  └─ 基准测试框架搭建

         ↓ (必须等待第 0 阶段完成)

第 1 阶段（4-6 周）：P1.a 并行执行框架
  ├─ 数据分区架构（Vertex/Edge 按 ID 范围）
  ├─ Pipeline 调度器框架（Pull-based 任务调度）
  ├─ 与新的 StreamingExecutor 无缝协作
  └─ LIMIT 查询 100x 加速

第 2 阶段（2-3 周）：关键 executor 迁移（与 P1.a 并行）
  ├─ StreamingScanVertices / StreamingScanEdges
  ├─ StreamingFilter / StreamingProject
  ├─ StreamingLimit（LIMIT 优化的关键）
  └─ 集成测试

第 3 阶段（2-3 周）：存储与执行优化（P1.c）
  ├─ 列级压缩选择器
  ├─ 并行 HashJoin
  └─ 性能验证

总周期：10-12 周（3 人团队并行）
```

---

## 2. 为什么架构重构是必须的

### 2.1 现有系统的问题

| 问题 | 影响 |
|------|------|
| 全物化执行 | 中间结果全在内存，LIMIT 浪费资源 |
| Push 模型 | Binary operator 立即执行子树，无法流式 |
| ExecutorEnum 爆炸 | 60+ variant，新增 executor 影响全局 |

### 2.2 架构重构的好处

1. **清晰的 Pull 模型**：最上层驱动下游，自然支持 LIMIT 中途停止
2. **无适配层开销**：直接改造 executor，不需要适配层
3. **完全动态分发**：新增 executor 无需改动现有代码
4. **与并行框架完美配合**：StreamingExecutor 天生支持 chunk 流

---

## 3. 详细的实施阶段

### 3.1 阶段 0：Executor 架构重构（1 周，必须先做）

**文件修改**：
- 新增：`executor/base/streaming_executor.rs`
- 新增：`executor/base/streaming_base_executor.rs`
- 修改：`executor/factory/engine.rs`（添加 ExecutionMode）

**关键决策**：
- ✅ **使用 `Box<dyn StreamingExecutor>`**（完全动态分发）
- ✅ 虚函数开销 < 0.2%，可忽略不计
- ✅ 避免 ExecutorEnum 爆炸问题
- ✅ 支持灵活的执行树构造

**验收标准**：
- [ ] StreamingExecutor trait 编译通过
- [ ] PlanExecutor 支持两种模式
- [ ] 基准测试框架可用

### 3.2 阶段 1：并行执行框架（4-6 周，与阶段 2 并行）

**文件修改**：
- 新增：`executor/streaming/mod.rs`（模块定义）
- 新增：`executor/scheduler/pipeline_scheduler.rs`
- 修改：`storage/vertex/vertex_table.rs`（分区接口）
- 修改：`storage/edge_table/mod.rs`（分区接口）

**分工建议**：
- **Developer A**：数据分区架构 + 分区视图
- **Developer B**：Pipeline 调度器核心逻辑
- **Developer C**：基准测试和性能验证

**关键功能**：
1. VertexTable/EdgeTable 分区（按 ID 范围）
2. QueryScheduler（任务队列、背压控制）
3. 与 StreamingExecutor 的协作
4. ExecutionMode::Streaming 支持

**验收标准**：
- [ ] LIMIT 查询性能：从 500ms → 5ms（100x 加速）
- [ ] 全表扫描：从 500ms → 70ms（7x 加速）
- [ ] 内存占用恒定（不随数据量增长）
- [ ] 所有集成测试通过

### 3.3 阶段 2：关键 executor 迁移（2-3 周，与阶段 1 并行）

**依赖关系**：
- 需要等待阶段 0 的 StreamingExecutor trait 完成
- 可以与阶段 1 的 Pipeline 调度器同时进行

**迁移优先级**：

#### 优先级 1（周 1）：数据源 + 消费端
- `StreamingScanVertices` / `StreamingScanEdges`
  - 难度：低
  - 原因：无依赖，基础
  
- `StreamingLimit`
  - 难度：低
  - 原因：LIMIT 优化的关键

#### 优先级 2（周 2）：处理算子
- `StreamingFilter`
  - 难度：低
  - 原因：高频，单输入，无状态
  
- `StreamingProject`
  - 难度：低
  - 原因：常见，单输入，无状态

#### 优先级 3（周 3）：可选
- `StreamingAggregate`
  - 难度：中
  - 原因：有状态，需要消费所有输入
  
- 其他 executor（延后）

**文件修改**：
- 新增：`executor/streaming/impl/scan.rs`
- 新增：`executor/streaming/impl/filter.rs`
- 新增：`executor/streaming/impl/project.rs`
- 新增：`executor/streaming/impl/limit.rs`
- 修改：`executor/factory/executor_factory.rs`（支持创建流式 executor）

**验收标准**：
- [ ] StreamingScan 通过所有数据源测试
- [ ] StreamingLimit 通过 LIMIT 功能测试
- [ ] 流式执行结果与全物化执行等价
- [ ] 内存占用验证（~4MB per chunk）

### 3.4 阶段 3：存储与执行优化（2-3 周）

**不依赖阶段 2**，可与阶段 1/2 并行进行。

**分工建议**：
- **Developer A**：列级压缩选择器（2 周）
- **Developer B**：并行 HashJoin（1.5 周）

**详见**：`P1c-storage-and-execution-optimizations.md`

---

## 4. 时间线示意图

### 方案 A：3 人团队（推荐）

```
时间 ↓  | A 开发者          | B 开发者         | C 开发者
-------+-------------------+-----------------+----------
W1    | 架构重构          | 架构重构         | 基准框架
W2-3  | 数据分区          | Pipeline 调度    | 测试用例
W4-5  | Scan + Filter     | 调度器集成       | 性能验证
W6    | Limit + Project   | 背压机制         | 文档
      |                   |                 |
并行  | 压缩选择器(W7-8)  | 并行 Join(W7-9) | -

总周期：9 周
```

### 方案 B：2 人团队

```
时间 ↓  | A 开发者          | B 开发者
-------+-------------------+------------------
W1    | 架构重构          | 基准框架
W2-3  | 数据分区          | Pipeline 调度器
W4-5  | Scan/Filter/Limit | 集成测试
W6-7  | 压缩选择器        | 并行 Join
W8    | 优化              | 文档

总周期：8 周
```

### 方案 C：1 人团队

```
时间 ↓  | 开发者
-------+-----------
W1    | 架构重构
W2-4  | 数据分区 + Pipeline
W5-6  | Scan/Filter/Limit
W7    | 压缩选择器
W8    | 并行 Join
W9    | 优化和文档

总周期：9 周
```

---

## 5. 新旧系统的并存策略

### 5.1 ExecutionMode 选择

```rust
pub enum ExecutionMode {
    /// 全物化模式（现有系统）
    Materialized,
    /// 流式模式（新系统）
    Streaming,
}

// 默认向后兼容
let mode = std::env::var("LINKRS_EXECUTION_MODE")
    .unwrap_or("materialized".to_string());
```

### 5.2 过渡策略

| 时间点 | Materialized | Streaming | 现状 |
|--------|-------------|-----------|------|
| 现在 | 默认 | 可选 | 旧系统运行 |
| 2-3 个月 | 可选 | 推荐 | 两系统可用 |
| 6+ 个月 | 备选 | 默认 | 新系统主导 |

### 5.3 用户可见的 API

```rust
// 默认向后兼容
pipeline_manager.execute_query(query)  // 用 Materialized

// 显式选择流式
pipeline_manager.execute_query_with_mode(query, ExecutionMode::Streaming)

// 查询 hint（后续支持）
// SELECT /*+ EXECUTION_MODE(streaming) */ * FROM V LIMIT 10
```

---

## 6. 性能目标

### 6.1 各阶段的性能提升

| 阶段 | 功能 | LIMIT 查询 | 全表扫描 | 内存占用 |
|------|------|-----------|--------|---------|
| 阶段 0 | 架构完成 | 基准 | 基准 | 基准 |
| 阶段 1 | 并行化 | 10-50x | 7x | 恒定 |
| 阶段 2 | 流式化 | 50x | 7x | 恒定 |
| 阶段 3 | 存储优化 | 50x | 7x | 恒定 + 30-70% 节省 |

### 6.2 具体数据（百万行表）

| 查询 | 现状 | 改进后 | 加速比 |
|------|------|--------|--------|
| SELECT * LIMIT 10 | 500ms | 5ms | 100x |
| SELECT * WHERE prop>X | 800ms | 200ms | 4x |
| SELECT * WHERE prop>X LIMIT 1000 | 800ms | 50ms | 16x |

---

## 7. 风险评估与缓解

| 风险 | 概率 | 影响 | 缓解 |
|------|------|------|------|
| 架构复杂度高，集成困难 | 中 | 阶段 0 延期 | 充分的设计评审；提前搭建测试框架 |
| 虚函数开销大于预期 | 低 | 需要优化 | 性能指标 < 0.2%，有优化空间 |
| 某些 executor 难以流式化 | 中 | 迁移不完整 | 允许混合模式；逐步迁移 |
| 新旧系统冲突 | 低 | 回退复杂 | ExecutionMode 完全隔离 |

---

## 8. 文档对应关系

### 设计文档

| 文档 | 内容 | 适用阶段 |
|------|------|---------|
| `streaming-executor-design-decisions.md` | StreamingExecutor 设计决策分析 | 阶段 0 |
| `executor-architecture-refactoring.md` | Executor 架构重构的详细设计 | 阶段 0 |
| `executor-implementation-guide.md` | 开发者实现指南 | 阶段 0-2 |
| `P1a-parallel-execution-framework.md` | 并行执行框架设计 | 阶段 1 |
| `P1c-storage-and-execution-optimizations.md` | 存储与执行优化 | 阶段 3 |
| `executor-and-p1-integration.md` | 整体集成说明 | 所有阶段 |

### 已过时的文档（应删除）

⚠️ **以下文档已过时，应删除**：
- ❌ `P1b-streaming-executor-migration.md`（已被架构重构合并）

---

## 9. 关键里程碑

| 里程碑 | 时间 | 验收标准 |
|--------|------|---------|
| **M1：架构完成** | 第 1 周 | StreamingExecutor trait 编译通过 |
| **M2：并行框架可用** | 第 6 周 | LIMIT 查询 100x 加速 |
| **M3：关键 executor 迁移** | 第 8 周 | Scan/Filter/Limit 通过所有测试 |
| **M4：流式系统稳定** | 第 9 周 | 执行结果完全等价，内存占用恒定 |
| **M5：性能优化完成** | 第 12 周 | 存储节省 30-70%，Join 加速 6-7 倍 |

---

## 10. 总结表格

| 问题 | 答案 |
|------|------|
| **为什么需要架构重构？** | 解决全物化和 ExecutorEnum 爆炸的问题 |
| **使用 dyn 会影响性能吗？** | 否，开销 < 0.2%，可忽略 |
| **何时开始？** | 立即，作为 P1 的基础 |
| **总周期？** | 9-12 周（取决于团队人数） |
| **向后兼容吗？** | 是，旧系统通过 ExecutionMode 保留 |
| **有备选方案吗？** | 没有，这是最优的设计 |

---

## 附录：执行树示例

### 现有的全物化 Push 模型

```
PlanExecutor
  └─ build_executor_chain() [同步 Push]
      ├─ Limit.execute()
      │  └─ Filter.execute()
      │     ├─ 立即调用 Scan.execute()
      │     │  └─ 返回完整 DataSet（全表）
      │     └─ 过滤 DataSet
      └─ 返回结果 DataSet
```

### 新的流式 Pull 模型

```
PlanExecutor (ExecutionMode::Streaming)
  └─ build_streaming_tree() [异步 Pull]
      └─ Limit.next()  [消费者驱动]
         └─ Filter.next()
            └─ Scan.next()
               └─ 返回 chunk(1024 行)
                  ↑ ↑ ↑ (逐层向上)
               
[重复多次直到 LIMIT 满足或无更多数据]
```

**关键差异**：
- 旧模型：一次性加载全表，再逐层处理
- 新模型：按需拉取 chunk，自然支持 LIMIT 中途停止
