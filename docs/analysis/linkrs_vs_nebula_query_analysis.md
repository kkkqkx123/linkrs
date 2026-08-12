# linkrs 与 Nebula Graph 查询模块架构对比分析

## 1. 项目概述

| 维度 | linkrs | Nebula Graph |
|------|--------|-------------|
| 语言 | Rust | C++ |
| 架构风格 | 单体 crate 内模块化（graphdb-query） | 分层服务（graphd / metad / storaged） |
| 查询语言 | 类 nGQL（自研解析器） | nGQL + openCypher（bison/flex） |
| 执行模型 | **Pull-based 流式执行**（DataChunk 驱动） | **Push-based DAG 异步执行**（folly::Future） |
| 优化器 | 两阶段：启发式 + 基于代价（CBO） | Cascades-style  Memo 结构（规则驱动探索） |

---

## 2. 整体查询处理流水线对比

### linkrs 查询流水线

```
SQL Text
  │
  ▼
Parser ───► AST
  │
  ▼
Binder ───► BoundStatement (语义验证 + 名称解析合一)
  │
  ▼
Planner ───► ExecutionPlan (逻辑计划)
  │
  ▼
Optimizer
  ├── 启发式优化 (Predicate Pushdown, Projection Pushdown, Elimination, Merge, Limit Pushdown)
  └── 基于代价优化 (Join Order, Index Selection, Traversal Strategy, Aggregate Strategy)
  │
  ▼
PhysicalPlanBuilder ───► PhysicalPlan
  │
  ▼
StreamingExecutionEngine ───► ResultStream / DataChunk
```

**关键特点：**
- Parse → Bind → Plan → Optimize → Build Physical → Execute 六阶段清晰分离
- Binder 合并了语义验证和名称解析，无需单独的 Validator 阶段
- 物理计划生成是显式阶段（`PhysicalPlanBuilder`），与逻辑计划分离
- 支持流式消费（`ResultStream`）和全物化两种执行模式

### Nebula 查询流水线

```
SQL Text
  │
  ▼
GQLParser (bison/flex) ───► Sentence (AST)
  │
  ▼
Validator(s) ───► AstContext + SubPlan
  ├── spaceChosen → validateImpl → checkPermission → toPlan
  └── 100+ 个独立 Validator 类
  │
  ▼
Optimizer (Cascades Memo) ───► 最优 PlanNode
  ├── RuleSet 驱动 (PushFilterDown, PushLimitDown, IndexScan, ...)
  ├── 最多 5 轮迭代探索
  └── PostProcess: Argument 重写 + Property Pruning
  │
  ▼
Scheduler (AsyncMsgNotifyBased) ───► Executor DAG
  │
  ▼
Executor 链 ───► ExecutionResponse
  ├── open() → execute() → close()
  └── folly::Future 异步链
```

**关键特点：**
- Validator 整合了语义验证和到 SubPlan 的转换
- Planner 嵌入在 Validator 的 `toPlan()` 方法中
- 优化器使用 Cascades 风格的 Memo 结构
- Scheduler 基于 BFS + 异步消息通知驱动 DAG 执行
- 执行器生命周期：open → execute(Future) → close

---

## 3. 各模块详细对比

### 3.1 解析器 (Parser)

| 维度 | linkrs | Nebula |
|------|--------|--------|
| 实现方式 | 纯 Rust 自研递归下降解析器 | C++ Bison/Flex 生成器 |
| AST 设计 | 强类型枚举 (`Stmt::Match`, `Stmt::Go`, ...) | Sentence 类层次结构 |
| 扩展性 | ExtensionRegistry 机制（插件式扩展） | 需修改 .yy / .lex 文件 |
| 错误恢复 | RecoveryScope 支持部分错误恢复 | 语法错误即终止 |

**linkrs 优势：** 纯 Rust 实现，无外部代码生成依赖；ExtensionRegistry 允许第三方扩展语法。

### 3.2 绑定/验证 (Binder / Validator)

| 维度 | linkrs (Binder) | Nebula (Validator) |
|------|-----------------|-------------------|
| 职责 | 语义验证 + 名称解析 + 类型推断合一 | 语义验证 + 权限检查 + 生成 SubPlan |
| 输出 | `BoundStatement` (IR) | `AstContext` + `SubPlan` (PlanNode 片段) |
| 设计模式 | 单一 Binder 结构体 | 100+ 个 Validator 子类（策略模式） |
| 权限检查 | 独立模块 (`permission.rs`) | 内嵌在 Validator 的 `checkPermission()` |
| 语义信息 | `SemanticInfo` 结构体 | `AstContext` 派生类体系 |

**Nebula 优势：** Validator 子类体系更细粒度，每个语句类型有专门的验证逻辑；权限检查与验证流程天然集成。

**linkrs 优势：** Binder 设计更简洁，通过 `BoundStatement` IR 实现验证与规划的清晰分离；单一职责原则贯彻更好。

### 3.3 规划器 (Planner)

| 维度 | linkrs | Nebula |
|------|--------|--------|
| 架构 | Planner trait + 具体 Planner 实现 + PlannerEnum 调度 | Planner 基类 + match/transform 模式 + 静态注册 |
| 计划节点 | 丰富 PlanNode 层次（50+ 类型） | PlanNode::Kind 枚举（50+ 类型） |
| 子计划组合 | SubPlan + SegmentsConnector | SubPlan 结构体 + 链表组合 |
| MATCH 支持 | MatchStatementPlanner + 路径规划 | MatchPathPlanner + 多阶段规划器 |
| 索引选择 | 独立 seek 策略模块（index_seek, vertex_seek, etc.） | LabelIndexSeek, PropIndexSeek, ScanSeek |
| 物理计划 | 显式 PhysicalPlan 阶段 | 无独立物理计划层（PlanNode 直接映射到 Executor） |

**linkrs 优势：**
- 物理计划 (`PhysicalPlan`) 与逻辑计划 (`ExecutionPlan`) 显式分离，支持更精细的物理优化
- 索引选择策略模块化（`seeks/` 目录），便于扩展新的索引访问方法
- `SegmentsConnector` 提供清晰的子计划连接抽象

**Nebula 优势：**
- Planner 静态注册机制（`PlannersRegister`）使添加新规划器更简洁
- 规划器与 Validator 的集成更紧密，减少了阶段间信息传递的复杂度

### 3.4 优化器 (Optimizer)

| 维度 | linkrs | Nebula |
|------|--------|--------|
| 架构 | 两阶段：启发式 → 基于代价 | Cascades 风格 Memo 结构 |
| 代价模型 | CostCalculator + SelectivityEstimator + CostModelConfig | 无显式代价模型（规则驱动） |
| 统计信息 | StatisticsManager + 采样收集器 | 无独立统计系统（依赖 Meta 服务） |
| 规则/策略 | 启发式规则 + 代价驱动策略 | OptRule 子类体系（50+ 规则） |
| 分区规划 | PartitioningConfig + PartitioningPlanner | 无分区级别优化 |
| CTE 缓存 | CteCacheManager + CteCacheDecisionMaker | 无 |
| 执行反馈 | FeedbackDrivenSelectivity + ExecutionFeedback | 无 |

**linkrs 优势：**
- 拥有完整的 CBO 代价模型（统计信息采集 → 选择性估计 → 代价计算 → 最优选择）
- 统计数据收集器支持按需采样和版本控制
- 执行反馈回路可自适应优化未来的查询计划
- 支持查询内分区规划（`PartitioningPlanner`）
- CTE 缓存决策机制避免重复计算

**Nebula 优势：**
- Cascades 模型通过 Memo 结构能并行探索更多计划空间
- 规则丰富（50+ 条），覆盖更多优化场景
- 属性裁剪（Property Pruning）作为后处理步骤，减少 IO

### 3.5 执行引擎 (Executor)

| 维度 | linkrs | Nebula |
|------|--------|--------|
| 模型 | Pull-based 流式执行 | Push-based 异步 DAG 执行 |
| 数据单位 | DataChunk（列式批次） | DataSet / Value |
| 算子类型 | 10+ 类算子（Source, Unary, Blocking, Join, Graph, Gather, etc.） | 按功能分目录（query, mutate, maintain, algo, logic, admin） |
| 并行 | P8 并行（MorselWorkerPool + 有界通道） | runMultiJobs（folly::collectAll） |
| 内存管理 | MemoryTracker + MemoryBudget + Spill | MemoryTracker + 水线检查 |
| JOIN 实现 | HashJoin, MergeJoin, NestedLoopJoin, CrossJoin | HashJoin, InnerJoin, LeftJoin |
| 流式输出 | ResultStream 支持逐块消费 | 全物化后返回 |
| 取消机制 | 运行时 Cancel 信号 | 无显式取消（Error 传播） |
| Profile | OperatorProfile + ProfileCollector | execTime + numRows 统计 |

**linkrs 优势：**
- **流式执行架构**：支持边计算边输出，降低首块延迟，节省内存
- **列式 DataChunk**：更利于向量化执行和 SIMD 优化
- **完善的并行框架**：P8 Morsel 驱动并行，有界通道控制背压，支持并行 Gather/MergeSort/Join
- **Spill-to-disk**：阻塞算子支持磁盘溢出，避免 OOM
- **显式内存预算**：MemoryBudget 体系防止单个查询耗尽资源
- **丰富的 JOIN 类型**：MergeJoin + NestedLoopJoin 补充 HashJoin

**Nebula 优势：**
- folly::Future 异步模型天然支持高并发请求
- `runMultiJobs` 框架支持通用的数据并行 scatter-gather 模式
- 执行器拓扑图（depends/successors）清晰表达算子依赖关系

### 3.6 调度器 (Scheduler)

| 维度 | linkrs | Nebula |
|------|--------|--------|
| 调度方式 | 无独立调度器（引擎驱动 pull） | AsyncMsgNotifyBasedScheduler（BFS 异步通知） |
| DAG 执行 | 树形递归 pull | DAG 拓扑 + 依赖通知 |
| 并行控制 | 通过 MaxWorkers + 有界通道 | 内部通过 Promise/Future 链 |

**Nebula 优势：** 独立的 Scheduler 层支持更灵活的 DAG 执行拓扑和异步调度策略。

---

## 4. linkrs 需要改进的方面

### 4.1 高优先级

#### 1. 优化器丰富度不足

**问题：** linkrs 优化器规则数量远少于 Nebula（50+ 条规则）。

**建议：**
- 增加更多启发式规则：
  - 过滤器合并（CombineFilter）：多个 Filter 节点合并为一个
  - 投影消除（CollapseProject）：连续 Project 合并
  - 无用节点消除（EliminateNoop）：消除无意义的 Project/Filter
  - 过滤条件下推至 Traverse/Expand 算子内部
- 增加与 Nebula 类似的规则变体：如 `PushFilterDownGetNeighborsRule`、`PushFilterDownTraverseRule` 等图遍历特定优化
- 实现属性裁剪（Property Pruning）：分析最终输出需要的列，裁剪中间算子读取的列

**参考文件：**
- Nebula: `src/graph/optimizer/rule/` 目录下 50+ 规则文件
- linkrs: `crates/graphdb-query/src/query/optimizer/heuristic/`

#### 2. 缺乏执行反馈回路

**问题：** 虽然已有 `FeedbackDrivenSelectivity` 框架，但实际执行反馈闭环尚未完整落地。

**建议：**
- 完善 `QueryExecutionFeedback` → `SelectivityFeedbackManager` → `CostCalculator` 的反馈链路
- 将执行器 `ProfileCollector` 收集的实际行数/代价反馈给优化器，用于调整后续查询的选择性估计
- 实现慢查询自动重优化机制

**参考文件：**
- linkrs: `crates/graphdb-query/src/query/optimizer/stats/` 中的 `FeedbackDrivenSelectivity`、`ExecutionFeedbackCollector`

#### 3. 解析器错误恢复能力有限

**问题：** 虽然 `RecoveryScope` 提供基本错误恢复，但相比生产级解析器功能有限。

**建议：**
- 增强错误恢复策略：支持多 token 同步、括号匹配恢复
- 提供更友好的错误信息（错误位置 + 预期 token + 上下文提示）
- 使用 `ExtensionRegistry` 实现 IDE 友好的增量解析支持

**参考文件：**
- linkrs: `crates/graphdb-query/src/query/parser/parsing.rs`

### 4.2 中优先级

#### 4. 缺乏 ACID 事务与查询的深度集成

**问题：** 事务管理在 `transaction/` 模块，但与查询执行引擎的集成较浅。

**建议：**
- 将 MVCC 快照信息注入 `QueryContext`，使执行器能感知事务隔离级别
- 在流式执行引擎中支持事务性游标（保持快照一致性读）
- 实现写冲突检测与查询执行阶段的事务回滚集成

**参考文件：**
- linkrs: `crates/graphdb-transaction/` 模块
- Nebula: `src/graph/context/` 中的事务支持

#### 5. 分区规划集成度不够

**问题：** `PartitioningPlanner` 主要在优化器层面工作，与执行器 Gather 的集成有改进空间。

**建议：**
- 将分区规划的结果直接编码到物理计划中，避免在 `StreamingExecutionEngine` 中后置组装
- 支持更丰富的分区策略：Hash 分区、Range 分区、Round-Robin 分区
- 实现分区级别的动态负载均衡（工作窃取）

**参考文件：**
- linkrs: `crates/graphdb-query/src/query/optimizer/partitioning.rs`
- linkrs: `crates/graphdb-query/src/query/executor/streaming/engine.rs`

#### 6. 缺乏 EXPLAIN/PROFILE 的深度集成

**问题：** 虽然 `explain/` 模块存在，但 EXPLAIN 和 PROFILE 功能与执行引擎的集成有待加强。

**建议：**
- 实现 EXPLAIN 输出物理计划详情（算子类型、估计行数、代价、分区策略）
- PROFILE 收集每个算子的实际执行时间、行数、内存使用
- 提供可视化计划输出（JSON/Graphviz 格式）

**参考文件：**
- Nebula: `src/graph/service/QueryInstance.cpp` 中的 `explainOrContinue()`
- linkrs: `crates/graphdb-query/src/query/executor/explain/`

### 4.3 低优先级

#### 7. 表达式框架优化

**问题：** 表达式求值通过 `Expression::evaluate()` 遍历实现，性能有优化空间。

**建议：**
- 实现表达式编译（Expression Compilation）：将表达式树编译为更高效的执行路径
- 批处理表达式求值（向量化）：对 DataChunk 按列求值而非逐行
- 常量折叠优化：在 Binder 阶段预计算常量表达式

**参考文件：**
- Nebula: `src/graph/visitor/FoldConstantExprVisitor.cpp`
- linkrs: `crates/graphdb-query/src/query/executor/expression/`

#### 8. 子查询优化

**问题：** 子查询处理通过 `SubqueryUnnestingOptimizer` 实现，但覆盖场景有限。

**建议：**
- 实现子查询去关联化（Subquery Decorrelation）：将相关子查询转换为 JOIN
- 支持 EXISTS / IN / NOT IN 子查询的高效执行
- 对于可缓存子查询（CTE），完善 `CteCacheManager` 的缓存策略

**参考文件：**
- linkrs: `crates/graphdb-query/src/query/optimizer/cost_based/subquery_unnesting.rs`

#### 9. 多语句事务支持

**问题：** 当前查询引擎主要面向单语句执行，多语句事务支持有限。

**建议：**
- 完善 `SessionTransactionController` 在多语句事务中的状态管理
- 支持事务内的 Savepoint 和部分回滚
- 实现查询间变量传递（`$var` 引用）的事务一致性保证

**参考文件：**
- linkrs: `crates/graphdb-query/src/query/executor/streaming/transaction_scope.rs`

---

## 5. 架构对比总结

### linkrs 的独特优势

| 能力 | 说明 |
|------|------|
| 流式执行 | 全链路 pull-based 流式处理，低延迟、低内存 |
| 列式 DataChunk | 为向量化和 SIMD 优化奠定基础 |
| CBO 代价模型 | 统计信息驱动的代价优化，比纯规则更精准 |
| 执行反馈回路 | 实际执行数据反馈优化，自适应调整 |
| 分区级并行 | P8 并行框架 + 有界通道背压控制 |
| Spill-to-disk | 阻塞算子磁盘溢出，避免 OOM |
| 物理计划显式化 | 逻辑/物理计划分离，支持更精细的物理优化 |
| 纯 Rust 实现 | 内存安全、无 GC、高性能 |
| 查询计划缓存 | 参数化查询 + Schema 版本感知的缓存失效 |

### Nebula 的独特优势

| 能力 | 说明 |
|------|------|
| Cascades 优化器 | 更多计划空间探索，理论上能找到更优计划 |
| 丰富优化规则 | 50+ 规则覆盖大量优化场景 |
| Validator 体系 | 细粒度语句验证，错误报告更精确 |
| 异步 DAG 调度 | 独立的 Scheduler 层，灵活的执行拓扑 |
| 生产验证 | 大型分布式生产环境验证 |
| 属性裁剪 | 自动裁剪不需要的列，减少 IO |

### 架构演进建议路线图

```
Phase 1 (短期) ─── 优化器增强
  ├── 增加 10-15 条启发式优化规则
  ├── 完善属性裁剪（Property Pruning）
  └── EXPLAIN 物理计划输出

Phase 2 (中期) ─── 执行反馈 + 分区优化
  ├── 执行反馈闭环完整落地
  ├── MVCC 与查询引擎深度集成
  ├── 分区规划与执行器集成优化
  └── 子查询优化增强

Phase 3 (长期) ─── 高级优化
  ├── 表达式编译/向量化求值
  ├── 分布式查询支持
  ├── 自适应查询执行（Adaptive Query Execution）
  └── 多语句事务完整支持
```

---

*分析日期：2026-08-06*
*分析基于：linkrs (kkkqkx123/linkrs) 与 Nebula Graph (vesoft-inc/nebula) 源码*