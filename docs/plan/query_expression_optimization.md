# 表达式求值优化：常量折叠 + 批量求值 方案

> 状态：待实施。对应分析文档 4.3-7「表达式框架优化」——
> 运行时上下文表达式已落地（2026-08-12），本文档覆盖剩余三项。

## 1. 现状分析

`ExpressionEvaluator::evaluate_recursive`（`expression_evaluator.rs:41-324`）
是**递归树遍历、逐行求值**：

- 每个 `Expression` 节点一次 match 分发 + 子表达式递归
- `Property { object: Variable, .. }` 有快路径（compound 列名直查，
  行 234-243），但整体无批处理
- `chunk/eval.rs` 对 DataChunk 是逐行回退路径，无列式求值

### 三项未落地

1. **常量折叠**：`ExpressionEvaluator::can_evaluate`（行 36-38，
   基于 `core/types/expr/analysis_utils.rs::is_evaluable`）已存在，
   全仓库**无调用方**——没有任何阶段把常量表达式预计算为 Literal
2. **批量/向量化求值**：无列式表达式求值路径，谓词/投影按行逐个求值
3. **表达式编译**：无编译为执行路径的机制

## 2. 方案设计

### 2.1 常量折叠（先落地，低风险高收益）

在启发式优化器新增 `FoldConstantsRule`（规则计数 53 → 54）：

- **适用条件**：`is_evaluable(expr) == true`（无变量、无属性访问、
  无运行时上下文依赖的表达式）
- **转换**：对可折叠表达式调用
  `ExpressionEvaluator::evaluate(expr, &mut NoopContext)`（用空上下文，
  因可折叠表达式不触碰上下文），结果替换为 `Expression::Literal`
- **不折叠**：含 `Variable` / `Property` / `Parameter` /
  `SessionVariable` / 聚合 / 窗口函数的表达式（`is_evaluable` 天然排除）；
  含副作用的函数（如 `rand()`）需在函数注册表增加
  `pure: bool` 标记，非纯函数跳过（新增字段，默认 false 即保守跳过）
- **挂载**：`optimizer/heuristic/batch.rs` 的常量优化批次
  （与 CombineFilter / CollapseProject 同批），EXPLAIN 可见
  `folded` 标记（可选）

```rust
// 伪代码（rewrite_rule.rs 模式）
fn apply(&self, node: &PlanNodeEnum) -> RewriteResult {
    for expr in node.expressions_mut() {
        if is_evaluable(expr) && pure_function(expr) {
            let value = ExpressionEvaluator::evaluate(expr, &mut NoopContext)?;
            *expr = Expression::Literal(value);
        }
    }
    RewriteResult::Changed
}
```

### 2.2 批量（列式）求值（中期）

分层实现，先覆盖高收益形状：

| 形状 | 列式路径 | 收益 |
|------|----------|------|
| `Filter(expr)` 简单谓词（`Variable` + 比较/逻辑运算） | 对 chunk 单列向量化计算 mask，收集命中行索引后 `take_rows` | 免逐行 dispatch + 免 `Value` 装箱（SIMD 友好） |
| `Project` 表达式 = 单列引用 / 常量 | 直接列引用（零拷贝）或广播列 | 最热路径 |
| `a + b` 数值二元 | 双列 zip 计算新列 | 通用批处理 |

- 实现位置：`executor/streaming/chunk/` 新增 `eval_batch.rs`，
  与 `eval.rs` 逐行回退并存——**批量路径检测不到的形状自动回退逐行**
  （检测函数 `try_batch_shape(expr) -> Option<BatchKind>`，简单保守）
- `ExpressionEvaluator` 保留为逐行求值器（表达式编译的中介目标），
  不重复实现

### 2.3 表达式编译（长期，仅设计）

- 方向 A（闭包链）：将表达式树编译为 `&dyn Fn(&RowContext) -> Value`
  闭包栈，避免每行递归 dispatch
- 方向 B（字节码）：与现有 `Expression` 枚举解耦的指令集
  （`Literal`/`LoadVar`/`LoadCompound`/`Binary`/`Call`/`Cast`），
  编译器在计划构建时（`plan.rs` 物理计划阶段）生成，执行器解释执行
- 前置依赖：2.2 的批处理框架；在热路径（Filter/Project 算子）内验证收益后
  再推广

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 | 阶段 |
|------|------|----------|------|
| 1 | 函数注册表增加 `pure` 标记（默认 false） | `executor/expression/functions/registry.rs`, `signature.rs` | 常量折叠 |
| 2 | `FoldConstantsRule` + 批次挂载 + 单测 | `optimizer/heuristic/rewrite_rule.rs`（或新文件 `fold_constants.rs`）, `batch.rs`, `rule_enum.rs` | 常量折叠 |
| 3 | 批量求值：`try_batch_shape` + Filter/Project 列式路径 + 回退 | `executor/streaming/chunk/eval_batch.rs`, `eval.rs`, Filter/Project 算子 | 批量 |
| 4 | 表达式编译（闭包链原型） | `executor/expression/compiler.rs`（新） | 编译 |
| 5 | 基准：filter/expression 微基准对比逐行 vs 批量 | `benches/` | 全部 |

## 4. 验证方法

- 单元测试（常量折叠）：`1+2`、`"a"+"b"`、`1+rand()`（不折叠）、
  `x + 1`（不折叠）；折叠后计划 EXPLAIN 断言 Literal
- 正确性（批量）：批量路径与逐行回退在随机数据上结果一致
  （属性等价测试）；空列/Null/类型混合边界
- 回归：`cargo test -p graphdb-query` 全量；e2e 关键路径
- 基准：`cargo bench` 对比折叠前后 / 批量 vs 逐行的吞吐

## 5. 预期收益

- 常量折叠：消除重复计算，缩小计划体积，几乎零风险
- 批量求值：Filter/Project 热路径吞吐提升（预期数量级收益来自
  免 `Value` 装箱与单次 dispatch）
- 为分析文档路线图 Phase 3 的「表达式编译/向量化求值」提供落点

## 6. 风险与回退

- **风险**：`is_evaluable` 判定与运行时求值语义不一致（如
  `ListComprehension` 需要迭代上下文但被误判可折叠）。缓解：折叠后
  结果与折叠前求值对比的单测锁死；`pure` 标记默认 false 保守
- **风险**：批量路径与逐行语义（错误顺序、副作用、Null 传播）偏差。
  缓解：形状检测严格保守，任一不满足即回退；差异用属性等价测试覆盖
- **回退**：`FoldConstantsRule` 从批次摘除即完全恢复；批量路径删除
  不影响逐行路径
