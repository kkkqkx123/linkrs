# 流式执行架构设计概览

> 更新日期：2026-06-27
> 核心决策：采用 Enum 而非 Dyn，pull-based 流式执行模型

---

## 1. 问题与目标

### 1.1 现有系统的问题

| 问题 | 表现 | 影响 |
|------|------|------|
| **全物化执行** | 所有结果一次性加载到内存 | LIMIT 10 等同于 LIMIT 百万，浪费资源 |
| **Push 模型** | Binary operator 立即执行子树，无法流式 | 无法实现中途停止，内存占用与数据量成正比 |
| **ExecutorEnum 爆炸** | 60+ executor variant，新增 executor 影响全局 | 代码维护困难，但实际上这不是核心问题 |

### 1.2 目标与收益

```
改进前：SELECT * FROM V LIMIT 10 (百万行)
  - 时间：500ms（扫描全表）
  - 内存：~500MB（全表）

改进后：
  - 时间：5ms（只扫描必要的行）
  - 内存：~4MB（固定大小的 chunk）
  - 加速比：100x
```

---

## 2. 核心决策：Enum vs Dyn

### 2.1 为什么选择 Enum

**选择对比表**：

| 方面 | Enum | Dyn Trait |
|------|------|-----------|
| **类型安全** | ✅ 编译期完整性检查 | ❌ 运行时分发 |
| **性能** | ✅ Match 优化，可能更快 | ⚠️ 虚函数 <0.2%（可忽略） |
| **新增 executor 成本** | ~5 行（在 enum 中添加 variant） | 0 行（自动工作） |
| **总成本** | 恒定（改工厂、写实现、加测试） | 恒定（改工厂、写实现、加测试） |
| **维护清晰度** | ✅ 所有类型明确列出 | ⚠️ 运行时才知道具体类型 |
| **项目规模适配** | ✅ 60 个 variant 完全可维护 | ⚠️ 适合需要运行时插件 |

**关键认识**：
1. **"新增 executor 会大范围修改"是真的** → 两种方案成本相同
2. **"项目不需要插件支持"是真的** → dyn 的主要优势不适用
3. **"编译期安全很重要"** → enum + match 全胜
4. **"枚举爆炸可维护"** → Rust std 库中有更多 variant 的 enum

**结论**：✅ **采用 Enum 方案**

---

## 3. 新的执行模型

### 3.1 从 Push 到 Pull

**旧模型（Push）**：
```
PlanExecutor
  └─ Limit.execute()
      └─ 立即调用 Filter.execute()
          └─ 立即调用 Scan.execute()
              └─ 返回完整 DataSet（全表）
          └─ 过滤
      └─ 截断
```
**特点**：执行者驱动，子树被动执行，一次全部

**新模型（Pull）**：
```
Limit.next()                    ← 消费者驱动
  └─ Filter.next()               ← 链式拉取
      └─ Scan.next()
          └─ 返回 chunk(1024 行) ← 逐层向上

[重复多次直到 LIMIT 满足]
```
**特点**：消费者驱动，逐个 chunk，自然支持中途停止

### 3.2 StreamingExecutor (Enum)

```rust
pub enum StreamingExecutor {
    // 数据源
    ScanVertices {
        table: Arc<VertexTable>,
        partition_id: usize,
        partition_range: Range<u32>,
    },
    ScanEdges { /* ... */ },
    
    // 单输入
    Filter {
        input: Box<StreamingExecutor>,
        condition: Expression,
    },
    Project {
        input: Box<StreamingExecutor>,
        columns: Vec<String>,
    },
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
    },
    
    // 双输入
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        condition: Expression,
    },
    
    // 有状态
    Aggregate {
        input: Box<StreamingExecutor>,
        group_by: Vec<String>,
        agg_funcs: Vec<AggFunc>,
    },
}

impl StreamingExecutor {
    pub fn open(&mut self) -> DBResult<()> { /* ... */ }
    pub fn next(&mut self) -> DBResult<Option<DataChunk>> { /* ... */ }
    pub fn close(&mut self) -> DBResult<()> { /* ... */ }
}
```

---

## 4. 四个实施阶段

```
┌──────────────────────────────────────────────────────┐
│ 阶段 0（1 周）：基础架构搭建                         │
│ - StreamingExecutor enum 定义                       │
│ - StreamingBaseExecutor 实现基础                    │
│ - ExecutionMode (Materialized vs Streaming)        │
│ - PlanExecutor 两条执行路径                         │
└──────────────────────────────────────────────────────┘
                    ↓ (必须完成)
┌──────────────────────────────────────────────────────┐
│ 阶段 1（4-6 周）：并行执行框架（与阶段 2 并行）    │
│ - 数据分区（Vertex/Edge 按 ID 范围）               │
│ - Pipeline 调度器（任务调度、背压）                │
│ - 与 StreamingExecutor 的集成                      │
│ 收益：LIMIT 10-50x 加速                           │
└──────────────────────────────────────────────────────┘
          ↓ (与阶段 1 并行)
┌──────────────────────────────────────────────────────┐
│ 阶段 2（2-3 周）：关键 executor 流式化              │
│ - Scan、Filter、Limit、Project                    │
│ - 集成测试                                         │
│ 收益：稳定的流式系统                               │
└──────────────────────────────────────────────────────┘
          ↓ (独立进行)
┌──────────────────────────────────────────────────────┐
│ 阶段 3（2-3 周）：存储优化（P1.c）                  │
│ - 列级压缩选择器                                   │
│ - 并行 HashJoin                                    │
│ 收益：存储减少 30-70%，性能再提 6-7x              │
└──────────────────────────────────────────────────────┘

总周期：9-12 周（团队 1-3 人）
```

---

## 5. 执行模式的切换

### 5.1 向后兼容策略

```rust
pub enum ExecutionMode {
    /// 现有系统（全物化）
    Materialized,
    /// 新系统（流式）
    Streaming,
}

impl QueryPipelineManager {
    pub fn execute_query(&self, query: &str) -> DBResult<ExecutionResult> {
        // 默认向后兼容
        self.execute_query_with_mode(query, ExecutionMode::Materialized)
    }
    
    pub fn execute_query_with_mode(
        &self,
        query: &str,
        mode: ExecutionMode,
    ) -> DBResult<ExecutionResult> {
        match mode {
            ExecutionMode::Materialized => {
                // 现有逻辑，完全无改动
            }
            ExecutionMode::Streaming => {
                // 新的流式执行路径
            }
        }
    }
}
```

### 5.2 过渡时间线

| 时期 | Materialized | Streaming | 状态 |
|------|------------|-----------|------|
| 现在 | 默认 | 可选 | 兼容期 |
| 2-3 个月 | 可选 | 推荐 | 双系统 |
| 6 个月+ | 备选 | 默认 | 流式优先 |

---

## 6. 整体架构图

```
QueryPipelineManager
  │
  ├─ execute_query()
  │  └─ ExecutionMode::Materialized
  │     └─ 现有的 ExecutorEnum + Push 执行
  │
  └─ execute_query_streaming()
     └─ ExecutionMode::Streaming
        ├─ 构建 StreamingExecutor enum 树
        ├─ 可选：创建 Pipeline 调度器（并行化）
        └─ Pull 循环：root_executor.next() 直到结束
           └─ DataChunk（chunk_size=1024） → 处理 → 返回结果

存储层
  ├─ VertexTable（分区接口）
  └─ EdgeTable（分区接口）
```

---

## 7. 关键特点

| 特点 | 说明 |
|------|------|
| **无适配层** | 直接改造 executor，不需要 ChunkingAdapter 之类的过渡层 |
| **完全隔离** | Materialized 和 Streaming 两条完全独立的路径，互不影响 |
| **类型安全** | Enum + match，所有类型检查在编译期完成 |
| **自然支持 LIMIT** | Pull 模型天然支持中途停止，无需特殊处理 |
| **内存高效** | chunk 固定大小（~4MB），内存占用恒定 |
| **并行友好** | 数据分区 + Pipeline 调度器，支持多核利用 |

---

## 8. 与前期设计的主要差异

### 对比表

| 方面 | 前期（dyn 方案） | 现在（enum 方案） |
|------|-----------------|-----------------|
| **StreamingExecutor** | trait object (`Box<dyn>`) | enum 类型 |
| **类型安全** | 运行时 | 编译期 ✅ |
| **新增 executor** | 0 行额外代码 | ~5 行（enum variant） |
| **维护清晰度** | 运行时才知道具体类型 | 所有类型明确列出 ✅ |
| **项目适配** | 为插件系统设计 | 为单体系统设计 ✅ |

---

## 9. 下一步

1. **详见**：`2-streaming-executor-design.md`（StreamingExecutor 详细设计）
2. **详见**：`3-parallel-execution-framework.md`（并行框架）
3. **详见**：`4-implementation-roadmap.md`（实施计划）
4. **详见**：`5-implementation-guide.md`（开发者指南）

---

## 总结

- ✅ **选择 Enum**：更适合这个项目（类型安全、维护清晰）
- ✅ **Pull 模型**：自然支持流式执行和 LIMIT 优化
- ✅ **四阶段计划**：清晰的依赖关系和并行策略
- ✅ **向后兼容**：旧系统通过 ExecutionMode 保留
