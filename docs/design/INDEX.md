# 流式执行系统文档导航

> 更新日期：2026-06-27
> 完整的设计文档集，共 7 个文件

---

## 📚 完整文档列表

### 核心设计文档（阅读顺序）

| 文档 | 内容 | 目标读者 | 大小 |
|------|------|---------|------|
| **1-architecture-overview.md** | 整体架构、核心决策、四阶段概览 | 所有人 | 8KB |
| **2-streaming-executor-design.md** | StreamingExecutor enum 详设 | 开发者、架构师 | 12KB |
| **3-parallel-execution-framework.md** | 数据分区、Pipeline 调度器 | 开发者 | 10KB |
| **4-implementation-roadmap.md** | 四阶段计划、时间线、人力分配 | 项目经理、技术主管 | 10KB |
| **5-implementation-guide.md** | 代码框架、测试策略、常见问题 | 开发者 | 12KB |
| **6-storage-optimization.md** | 压缩选择器、并行 Join | 开发者（可选） | 8KB |

---

## 🎯 按角色推荐阅读顺序

### 对于项目经理/技术主管

1. **1-architecture-overview.md**（必读）
   - 了解整体战略方向
   - 理解核心决策（enum vs dyn）
   - 掌握四阶段的依赖关系

2. **4-implementation-roadmap.md**（必读）
   - 详细的时间线
   - 人力分配和分工
   - 风险评估和缓解

3. **2-streaming-executor-design.md**（第 1-2 章）
   - 快速理解架构亮点

4. **3-parallel-execution-framework.md**（第 1-2 章）
   - 理解并行框架的基本思路

### 对于架构师/技术负责人

1. **1-architecture-overview.md**（全读）
   - enum 决策的全面分析
   - 四阶段的科学依据

2. **2-streaming-executor-design.md**（全读）
   - StreamingExecutor 的完整设计
   - 执行模型的转变

3. **3-parallel-execution-framework.md**（全读）
   - 并行框架的深入设计
   - 与 executor 的协作模式

4. **4-implementation-roadmap.md**（全读）
   - 确认时间线和风险

### 对于开发者

1. **1-architecture-overview.md**（必读）
   - 理解为什么这样设计

2. **5-implementation-guide.md**（必读）
   - 代码框架和具体实现步骤
   - 测试策略
   - 常见问题

3. **2-streaming-executor-design.md**（参考）
   - 实现细节
   - 各个 executor 的迁移模式

4. **3-parallel-execution-framework.md**（参考）
   - 如果实现并行框架

5. **4-implementation-roadmap.md**（参考）
   - 了解自己的任务在整体计划中的位置

6. **6-storage-optimization.md**（可选）
   - 如果参与存储优化

---

## 📖 各文档的核心内容速览

### 1. 架构概览 (1-architecture-overview.md)

**核心观点**：
```
Enum vs Dyn 决策：
  ✅ 选择 Enum
  理由：
    - 编译期类型检查
    - 新增 executor 成本相同
    - 项目不需要运行时插件
    - 枚举爆炸问题可维护（Rust std 中有更多 variant）

Push vs Pull 决策：
  ✅ 选择 Pull
  理由：
    - 自然支持 LIMIT 中途停止
    - 逐 chunk 处理，内存恒定
    - 与并行框架完美配合

四阶段清晰依赖：
  阶段 0（1 周）→ 阶段 1+2（并行，5-6 周）→ 阶段 3（2-3 周）
```

**性能目标**：
- LIMIT 10（百万行）：500ms → 5ms（100x）
- 全表扫描（8 核并行）：500ms → 70ms（7x）

### 2. StreamingExecutor 设计 (2-streaming-executor-design.md)

**核心内容**：
```
StreamingExecutor Enum
  ├─ 数据源：ScanVertices, ScanEdges
  ├─ 单输入：Filter, Project, Limit
  ├─ 有状态：Aggregate, Sort
  └─ 双输入：HashJoin, NestedLoopJoin

三个生命周期方法：
  - open()：初始化，打开下游
  - next()：返回下一个 chunk
  - close()：清理资源

三种 executor 迁移模式：
  1. 无状态单输入（Filter）：简单
  2. 有状态单输入（Aggregate）：消费全部输入
  3. 二元输入（Join）：Build side 必须加载
```

**关键代码框架**：完整的 Enum 定义和 next() 实现示例

### 3. 并行框架 (3-parallel-execution-framework.md)

**核心内容**：
```
数据分区
  ├─ 粒度：CPU 核数 * 2（8 核 → 16 分区）
  └─ 方式：按 ID 范围（逻辑分区，不复制）

Pipeline 调度器
  ├─ 职责：协调 executor 树的拉取
  ├─ 背压控制：最多 10 个缓冲 chunk
  └─ 与 StreamingExecutor 的协作：Scan executor 持有分区信息

执行路径
  - 简单查询：无调度器，顺序执行
  - 复杂查询（有 LIMIT）：使用调度器，并行执行
```

**性能目标**：
- LIMIT 10-50x 加速（相比单线程）
- 全表 7x 加速（8 核）

### 4. 实施路线图 (4-implementation-roadmap.md)

**核心内容**：
```
阶段 0（1 周）：基础架构
  - StreamingExecutor enum 定义
  - ExecutionMode 支持
  - 基准测试框架

阶段 1（4-6 周）：并行框架
  - 数据分区
  - Pipeline 调度器
  - 与 QueryPipelineManager 集成

阶段 2（2-3 周）：关键 executor 流式化
  - Scan, Filter, Limit, Project
  - 与阶段 1 并行

阶段 3（2-3 周）：存储优化（独立）
  - 列级压缩
  - 并行 HashJoin

人力方案：
  - 3 人团队：9 周（推荐）
  - 2 人团队：11 周
  - 1 人团队：12 周
```

**关键里程碑**：
- W1：架构完成
- W6：LIMIT 100x 加速
- W9：流式系统稳定

### 5. 实现指南 (5-implementation-guide.md)

**核心内容**：
```
完整的代码框架
  ├─ StreamingExecutor enum 模板
  ├─ DataChunk 实现
  ├─ ExecutionMode 定义
  ├─ PlanExecutor 改造
  └─ PartitionView 实现

具体的 executor 实现
  ├─ StreamingScanVertices（1024 行一个 chunk）
  ├─ StreamingFilter（拉取上游、过滤、返回）
  ├─ StreamingLimit（达到配额时停止）
  └─ StreamingAggregate（消费全部，迭代返回）

测试策略
  ├─ 单元测试（每个 executor）
  ├─ 集成测试（查询端到端）
  ├─ 性能基准测试（criterion）
  └─ 代码质量检查清单

常见问题
  - Q：executor 还未流式化怎么办？
    A：暂不支持混合，选 Materialized 或 Streaming
  
  - Q：如何测试 ExecutionMode 切换？
    A：对同一查询执行两种模式，验证结果等价
```

### 6. 存储优化 (6-storage-optimization.md)

**核心内容**：
```
列级压缩选择器
  ├─ ID/时间戳：差值编码 + RLE（70-80%）
  ├─ 低基数：字典编码（80-90%）
  ├─ 文本：ZSTD 强压缩（40-60%）
  └─ 高基数随机数据：无压缩

并行 HashJoin
  ├─ 构建阶段：右表分区并建哈希表
  ├─ 探测阶段：左表分区并与右表 join
  └─ 性能目标：6-7 倍加速

可独立进行，推荐在阶段 1 完成后
```

---

## 📊 文档关系图

```
1-architecture-overview.md
        ↓ (基础，必读)
        
    ┌───┴────┬────┬────┐
    ↓        ↓    ↓    ↓
   2-SE    3-PE  4-IM  5-IG
   (设计)  (框架) (计划) (指南)
    │       │    │    │
    └───┬───┴┬───┴────┘
        ↓   ↓
    6-存储优化
    (可选)
```

**说明**：
- SE = StreamingExecutor 设计
- PE = 并行执行框架
- IM = 实施路线图
- IG = 实现指南

---

## 🚀 快速开始

### 第一天

1. 阅读 `1-architecture-overview.md`（30 分钟）
   - 理解整体设计方向

2. 浏览 `5-implementation-guide.md` 的代码框架（30 分钟）
   - 了解实现的样子

### 第一周

1. 完整阅读 `1-architecture-overview.md`
2. 阅读 `2-streaming-executor-design.md`
3. 根据 `4-implementation-roadmap.md` 确定自己的任务

### 开始开发

1. 参考 `5-implementation-guide.md` 的代码框架
2. 逐个实现阶段 0 的任务
3. 通过单元测试和集成测试验证

---

## 📝 文档维护

### 何时更新

- 核心决策改变：更新相关文档
- 发现 bug 或错误：立即修正
- 实现完成：更新进度清单

### 禁止事项

- ❌ 不添加临时标记（如"Phase 1"、"Plan A"）
- ❌ 不包含中文注释在代码中
- ❌ 不分散到多个文件（集中在这 7 个文档）

### 永久注释

所有注释都应该是永久的解释，而非临时标记：

```rust
// ✅ 好的注释：
/// 拉取下一个 chunk，或返回 None 表示数据结束
pub fn next(&mut self) -> DBResult<Option<DataChunk>> { ... }

// ❌ 避免的注释：
/// TODO Phase 1: implement streaming
/// HACK: temporary solution for P1a
```

---

## 🎓 学习路径

### 如果你想了解...

**"为什么选择 Enum 而不是 Dyn？"**
→ 阅读 `1-architecture-overview.md` 的第 2 章

**"StreamingExecutor 的具体实现"**
→ 阅读 `2-streaming-executor-design.md` 和 `5-implementation-guide.md` 的代码框架

**"如何实现并行化"**
→ 阅读 `3-parallel-execution-framework.md`

**"整个项目要花多长时间"**
→ 阅读 `4-implementation-roadmap.md` 的第 6-7 章

**"写代码时该怎么做"**
→ 阅读 `5-implementation-guide.md`

**"存储压缩怎么实现"**
→ 阅读 `6-storage-optimization.md`

---

## ✅ 验收标准

### 文档质量

- [ ] 没有拼写错误
- [ ] 代码示例可编译
- [ ] 性能数字有依据
- [ ] 没有过时的参考

### 实现验收

- [ ] 所有代码通过 `cargo clippy`
- [ ] 所有测试通过 `cargo test`
- [ ] 文档与代码保持同步
- [ ] 功能完整，无遗留

---

## 📞 问题与反馈

如有疑问，请参考对应的文档：

| 问题类型 | 参考文档 |
|---------|--------|
| 架构和设计 | 1-architecture-overview.md |
| Executor 实现 | 2-streaming-executor-design.md + 5-implementation-guide.md |
| 并行化 | 3-parallel-execution-framework.md |
| 时间线和计划 | 4-implementation-roadmap.md |
| 具体代码问题 | 5-implementation-guide.md |
| 存储优化 | 6-storage-optimization.md |

---

## 总结

这套文档涵盖了从**战略决策**到**具体实现**的全面设计。

- 📌 **核心文档**：1-5 个
- 📌 **可选文档**：6（存储优化）
- 📌 **总字数**：~60KB
- 📌 **完整度**：包括设计、实现、测试、人力规划

**开始阅读：从 1-architecture-overview.md 开始。**

