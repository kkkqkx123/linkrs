# 设计文档导航与整理说明

> 更新日期：2026-06-27
> 说明：文档整理，标记过时文档，提供清晰的导航

---

## 📋 核心设计文档（应保留）

### 第 0 阶段：Executor 架构重构

| 文档 | 目标读者 | 核心内容 |
|------|---------|---------|
| **`streaming-executor-design-decisions.md`** | 架构师、技术主管 | StreamingExecutor 使用 dyn 的决策分析：为什么用 dyn 而不是 enum；性能对比；最终设计选择 |
| **`executor-architecture-refactoring.md`** | 开发者、架构师 | 详细的 executor 重构设计：现有问题、Push vs Pull、StreamingExecutor trait 定义、PlanExecutor 改造 |

### 第 1 阶段：并行执行框架

| 文档 | 目标读者 | 核心内容 |
|------|---------|---------|
| **`P1a-parallel-execution-framework-v2.md`** | 开发者 | 数据分区架构、Pipeline 调度器设计、与 StreamingExecutor 的协作（v2.0：基于 StreamingExecutor，而非全物化） |

### 第 2 阶段：Executor 迁移（指导）

| 文档 | 目标读者 | 核心内容 |
|------|---------|---------|
| **`executor-implementation-guide.md`** | 开发者 | 逐步实现指南：代码框架、StreamingScanExecutor/FilterExecutor/LimitExecutor 的具体实现 |

### 第 3 阶段：存储与执行优化

| 文档 | 目标读者 | 核心内容 |
|------|---------|---------|
| **`P1c-storage-and-execution-optimizations.md`** | 开发者 | 列级压缩选择器、并行 HashJoin 的设计 |

### 整合与计划

| 文档 | 目标读者 | 核心内容 |
|------|---------|---------|
| **`P1-implementation-roadmap.md`** | 项目经理、技术主管 | 完整的实施路线：4 个阶段的划分、时间线、并行策略、里程碑、风险评估 |
| **`executor-and-p1-integration.md`** | 架构师 | 四维改造结构、dyn 设计决策、执行流程完整描述、融合方案 |

---

## ⚠️ 过时文档（应删除或存档）

### 待删除的文档

以下文档已被新的设计整合，不再使用：

| 文档 | 原因 | 被替代为 |
|------|------|---------|
| ❌ **`P1b-streaming-executor-migration.md`** | 内容已整合到 executor 重构和实现指南中；提议的 ChunkingAdapter 被放弃 | `streaming-executor-design-decisions.md` + `executor-architecture-refactoring.md` + `executor-implementation-guide.md` |
| ⚠️ **`P1-roadmap.md`** | 已被更新版本替代 | `P1-implementation-roadmap.md` |
| ❌ **`P1a-parallel-execution-framework.md`** （v1.0） | v1.0 有重大设计缺陷：假设基于全物化 DataSet，未依赖第 0 阶段架构重构，与新设计不兼容 | `P1a-parallel-execution-framework-v2.md` |

### 删除命令（供参考）

```bash
# 在 codebase 中删除过时文档
rm docs/design/P1b-streaming-executor-migration.md
rm docs/design/P1-roadmap.md
rm docs/design/P1a-parallel-execution-framework.md

# 保留的最新文档
# - P1a-parallel-execution-framework-v2.md (新)
# - P1-implementation-roadmap.md (新)
```

---

## 📖 推荐阅读顺序

### 对于架构师/技术主管

1. **`streaming-executor-design-decisions.md`**
   - 理解为什么使用 dyn
   - 理解性能开销分析
   
2. **`executor-architecture-refactoring.md`**（第 1-5 章）
   - 理解现有问题
   - 理解新的架构模式
   
3. **`P1-implementation-roadmap.md`**
   - 理解完整的实施计划
   - 理解里程碑和风险
   
4. **`executor-and-p1-integration.md`**
   - 理解各部分的融合方式

### 对于开发者

1. **`executor-implementation-guide.md`**
   - 了解怎么做
   - 代码框架和关键实现点
   
2. **`executor-architecture-refactoring.md`**（第 4-5 章）
   - 了解具体实现模式
   
3. **`streaming-executor-design-decisions.md`**
   - 理解为什么这么做

### 对于项目经理

1. **`P1-implementation-roadmap.md`**
   - 时间线
   - 人力分配
   - 里程碑

---

## 🎯 各文档的主要观点总结

### 1. 核心决策：使用 Dyn

**文件**：`streaming-executor-design-decisions.md`

**结论**：
- ✅ StreamingExecutor **必须使用 `Box<dyn>`**
- 虚函数开销 < 0.2%，可忽略不计
- 避免 ExecutorEnum 爆炸（60+ variant 维护困难）
- 新增 executor 无需改动现有代码

### 2. 架构改造的四个阶段

**文件**：`P1-implementation-roadmap.md`

**结论**：
- **第 0 阶段**（1 周）：Executor 架构重构（必须先做）
- **第 1 阶段**（4-6 周）：并行执行框架
- **第 2 阶段**（2-3 周）：关键 executor 迁移（与第 1 并行）
- **第 3 阶段**（2-3 周）：存储与执行优化（独立）
- **总周期**：9-12 周（取决于团队人数）

### 3. 执行模型的转变

**文件**：`executor-architecture-refactoring.md`

**结论**：
- **从 Push 改为 Pull**：消费者驱动，最上层拉数据
- **从全物化改为流式**：逐个 chunk 处理，内存占用恒定
- **无适配层**：直接改造，清晰的执行链

### 4. 设计特点

**文件**：`executor-and-p1-integration.md`

**结论**：
- 四维改造结构，清晰的依赖关系
- 新旧系统通过 ExecutionMode 隔离
- 向后兼容性完全保证
- dyn 设计理由充分

---

## 📊 文档关系图

```
streaming-executor-design-decisions.md
        ↓ (dyn 决策确认)
executor-architecture-refactoring.md
        ↓ (基础架构完成)
    ┌───┴───┐
    ↓       ↓
P1a-    P1c-
parallel storage-exec
        ↓ (都依赖)
executor-and-p1-integration.md
        ↓ (完整融合方案)
P1-implementation-roadmap.md
        ↓ (最终实施计划)
executor-implementation-guide.md
        ↓ (开发者执行)
[开始实施]
```

---

## ✅ 文档清单（最终）

### 保留的文档（7 个）

- [x] `streaming-executor-design-decisions.md` - dyn 决策分析
- [x] `executor-architecture-refactoring.md` - 架构重构详设
- [x] `executor-implementation-guide.md` - 开发者指南
- [x] `P1a-parallel-execution-framework.md` - 并行框架设计
- [x] `P1c-storage-and-execution-optimizations.md` - 存储优化设计
- [x] `executor-and-p1-integration.md` - 融合方案
- [x] `P1-implementation-roadmap.md` - 实施路线图（新）

### 删除的文档（2 个）

- [x] ❌ `P1b-streaming-executor-migration.md` - 已过时（内容整合到新文档）
- [x] ❌ `P1-roadmap.md` - 已过时（被 P1-implementation-roadmap.md 替代）

---

## 🚀 实施建议

### 立即行动

1. **审核新文档**
   - 重点审核：`streaming-executor-design-decisions.md`（dyn 决策）
   - 重点审核：`P1-implementation-roadmap.md`（时间线）

2. **删除过时文档**
   ```bash
   rm docs/design/P1b-streaming-executor-migration.md
   rm docs/design/P1-roadmap.md
   ```

3. **更新项目维基或 README**
   - 指向 `P1-implementation-roadmap.md`
   - 说明最新的架构决策

4. **开始第 0 阶段**
   - 时间：本周开始
   - 工作：StreamingExecutor trait 定义 + PlanExecutor 改造
   - 人员：1-2 人

### 关键日期

- **W1 末**：第 0 阶段完成（架构就位）
- **W6 末**：第 1 阶段完成（LIMIT 100x 加速）
- **W9 末**：第 2 阶段完成（流式系统稳定）
- **W12 末**：第 3 阶段完成（整体优化）

---

## 📝 文档版本历史

| 版本 | 日期 | 主要更新 |
|------|------|---------|
| v1.0 | 2026-06-27 | 初始设计（P1a/P1b/P1c） |
| v2.0 | 2026-06-27 | 引入 executor 重构，舍弃 ChunkingAdapter，确认 dyn 方案 |
| v2.1 | 2026-06-27（现在）| 整理文档，删除过时，提供清晰导航 |

---

## ❓ 常见问题

**Q: 为什么要删除 P1b？**
A: 内容已整合到 executor 重构中。executor 架构重构本身包含了流式化的设计，不需要单独的 P1b 阶段。

**Q: 新的路线图和旧的有什么区别？**
A: 旧的是 3 个并列的 P1a/P1b/P1c；新的是 4 个串联的阶段（0→1→2→3），第 0 是基础，第 1-2 并行，第 3 独立。

**Q: Dyn 真的不会有性能问题吗？**
A: 是的。虚函数开销 < 0.2%，相对于数据处理和 IO 完全可忽略。详见 `streaming-executor-design-decisions.md`。

**Q: 什么时候应该迁移到新系统？**
A: 第 1 阶段完成后，新系统可用但不是默认。通过 ExecutionMode 开关。建议在充分测试后逐步推广。

---

**总结**：设计已定型，文档已整理，可以开始实施。关键是坚持 dyn 方案和四阶段路线。
