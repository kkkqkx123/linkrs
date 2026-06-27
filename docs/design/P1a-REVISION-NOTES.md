# P1a 重大修订说明

> 修订日期：2026-06-27
> 原因：发现 v1.0 有重大设计缺陷

---

## 发现的问题

用户提出了一个关键问题：**P1a 设计中说"无需改动现有 executor"是错的**。

这表明 P1a v1.0 的设计有以下缺陷：

1. **假设基础不对**
   - v1.0 假设在"现有全物化 DataSet"基础上设计
   - 实际上应该基于"第 0 阶段的 StreamingExecutor"

2. **依赖关系不清**
   - v1.0 没有明确依赖第 0 阶段
   - 实际上 Pipeline 调度器必须建立在 StreamingExecutor 之后

3. **架构不一致**
   - v1.0 提议改造现有 Executor，创建 `ExecutionMode::Parallel`
   - v2.0 改为基于 StreamingExecutor，使用 `ExecutionMode::Streaming`

---

## v1.0 和 v2.0 的关键差异

### 执行模式

**v1.0**（错误）：
```
原有的 ExecutionMode 之外，新增一种 Parallel 模式
ExecutionMode {
    Materialized,      // 现有
    Parallel { ... },  // 新增，基于全物化 DataSet
}
```

**v2.0**（正确）：
```
与第 0 阶段保持一致
ExecutionMode {
    Materialized,  // 现有，使用 ExecutorEnum
    Streaming,     // 新的，使用 StreamingExecutor
}
```

### Pipeline 调度器的职责

**v1.0**（误解）：
- QueryScheduler 直接驱动 ExecutorEnum
- 对 executor 调用 `execute()`，获得完整 DataSet
- 然后分割成 chunk

**v2.0**（正确）：
- QueryScheduler 驱动 StreamingExecutor 树
- 对 executor 调用 `next()`，获得 chunk
- 天然支持分区并行化

### 与其他阶段的关系

**v1.0**（模糊）：
```
P0 (完成)
  ↓
P1.a (设计模糊，与后续阶段关系不清)
P1.b
P1.c
```

**v2.0**（清晰）：
```
第 0 阶段：Executor 架构重构
  ├─ StreamingExecutor trait
  ├─ ExecutionMode 支持
  └─ PlanExecutor 两条路径
       ↓ (必须完成)

第 1 阶段：P1.a 并行执行框架
  └─ 基于 StreamingExecutor
       ↓ (与第 2 并行)

第 2 阶段：Executor 迁移
  └─ StreamingScan/Filter/Limit
```

---

## v2.0 的核心改进

### 1. 架构清晰度提升

v2.0 明确了：
- ✅ 第 1 阶段必须依赖第 0 阶段
- ✅ StreamingExecutor 是基础
- ✅ Pipeline 调度器是可选的协调层
- ✅ 现有系统完全保留（Materialized 模式）

### 2. 设计一致性提升

v2.0 确保了：
- ✅ 与 streaming-executor-design-decisions.md 一致
- ✅ 与 executor-architecture-refactoring.md 一致
- ✅ 与 P1-implementation-roadmap.md 一致
- ✅ 与 executor-and-p1-integration.md 一致

### 3. 实现可行性提升

v2.0 提供了：
- ✅ 明确的前置条件（第 0 阶段完成）
- ✅ 清晰的分界线（什么时候需要 Pipeline 调度器，什么时候不需要）
- ✅ 与 executor 迁移的紧密配合
- ✅ 向后兼容的保证

---

## 版本对照表

| 方面 | v1.0 | v2.0 |
|------|------|------|
| **基础** | 全物化 DataSet | StreamingExecutor |
| **前置条件** | P0（架构无特殊要求） | 第 0 阶段必须完成 |
| **ExecutionMode** | 新增 Parallel 模式 | 使用 Streaming 模式 |
| **Pipeline 调度器** | 改造 ExecutorEnum 和 Executor | 基于 StreamingExecutor |
| **现有系统** | 需要改动 | 完全保留 |
| **与第 2 阶段的关系** | 模糊 | 清晰（第 2 提供更多 StreamingExecutor） |
| **推荐度** | ❌ 不推荐 | ✅ 推荐 |

---

## 修订后的实施顺序

```
第 0 阶段（1 周）：Executor 架构重构
  ├─ StreamingExecutor trait 定义        [必须]
  ├─ ExecutionMode 支持                  [必须]
  └─ PlanExecutor.execute_streaming 路径 [必须]
       ↓
第 1 阶段（4-6 周）：P1.a 并行执行框架 (v2.0)
  ├─ 数据分区                            [周 1]
  ├─ Pipeline 调度器框架                 [周 2-3]
  ├─ 与 PlanExecutor 集成                [周 4]
  └─ 性能验证                            [周 5-6]
       ↓ (并行)
第 2 阶段（2-3 周）：关键 executor 迁移
  ├─ StreamingScanVerticesExecutor       [周 1]
  ├─ StreamingFilterExecutor             [周 2]
  └─ StreamingLimitExecutor              [周 2]
```

---

## 应该删除的文档

以下文档因设计缺陷应该删除：

```bash
rm docs/design/P1a-parallel-execution-framework.md  # v1.0，被 v2.0 替代
```

新文档：
```
保留 docs/design/P1a-parallel-execution-framework-v2.md  # v2.0，推荐
```

---

## 对现有计划的影响

### 时间线

**无影响**：v2.0 的工期与 v1.0 相同（4-6 周）

### 人力

**无影响**：推荐仍然是 3 人团队

### 工作内容

**改变**：
- ❌ v1.0：改造 ExecutorEnum 和现有 Executor
- ✅ v2.0：基于 StreamingExecutor，现有系统不动

### 风险

**降低**：
- ❌ v1.0：现有系统需要改动，风险高
- ✅ v2.0：完全隔离，只扩展新路径，风险低

---

## 总结

这是一个**重要的架构改进**：

1. **从"改造现有系统"改为"扩展新路径"**
   - 风险大幅降低
   - 兼容性得到保证

2. **从"模糊的依赖关系"改为"清晰的阶段划分"**
   - 第 0 → 第 1 → 第 2，清晰的依赖链
   - 每个阶段的前置条件明确

3. **从"独立的 P1.a"改为"四阶段的完整方案"**
   - P1a 不再独立
   - P1a、P1c、executor 重构形成有机整体

**感谢用户的深入思考和质疑，这次修订使设计更加合理和可行。**
