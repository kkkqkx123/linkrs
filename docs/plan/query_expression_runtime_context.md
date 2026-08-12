# 表达式求值器运行时上下文表达式支持方案

## 1. 现状分析

通用求值器 `executor/expression/evaluator/expression_evaluator.rs` 对以下
表达式变体直接返回 `type_error`（"require runtime context"）：

| 表达式变体 | 行号 | 现状 |
|------------|------|------|
| `Expression::Label(_)` | 270 | 返回错误 |
| `Expression::ListComprehension` | 273 | 返回错误 |
| `Expression::LabelTagProperty` | 276 | 返回错误 |
| `Expression::Predicate` | 289 | 返回错误 |
| `Expression::Reduce` | 292 | 返回错误 |
| `Expression::PathBuild` | 295 | 返回错误 |

对照：`Expression::Exists`（行 301）已通过 `ExpressionContext::execute_subquery`
获得运行时能力；`TagProperty` / `EdgeProperty`（行 255-288）已支持
`var.prop` 复合列快速路径。

**问题**：这些表达式并非不可求值，而是依赖"图运行时上下文"（变量绑定类型、
路径语义、标签解析）。当前它们只能靠特定算子内部的特判路径执行，通用求值器
无法消费，导致列式批量求值（`chunk/eval.rs` 的 `evaluate_expressions`）与
这些表达式互斥。

## 2. 方案设计

### 2.1 扩展 ExpressionContext trait

`executor/expression/evaluator/traits.rs` 的 `ExpressionContext` trait 增加
运行时上下文方法，全部提供默认实现（返回 `Unsupported`），保证现有实现
零破坏：

```rust
pub trait ExpressionContext {
    // ...现有方法...

    /// Evaluate a label expression against the current row binding.
    fn evaluate_label(&self, label: &LabelExpr) -> Result<Value, ExpressionError> {
        Err(ExpressionError::unsupported("label"))
    }

    fn evaluate_list_comprehension(
        &self,
        expr: &ListComprehension,
    ) -> Result<Value, ExpressionError> { /* 默认 Unsupported */ }

    fn evaluate_predicate(&self, expr: &PredicateExpr) -> Result<Value, ExpressionError> { /* 默认 */ }
    fn evaluate_reduce(&self, expr: &ReduceExpr) -> Result<Value, ExpressionError> { /* 默认 */ }
    fn evaluate_path_build(&self, expr: &PathBuildExpr) -> Result<Value, ExpressionError> { /* 默认 */ }
}
```

`expression_evaluator.rs` 的对应分支改为调用这些 trait 方法，删除内联的
`type_error` 返回。

### 2.2 实现运行时上下文

两类实现：

1. **行绑定上下文**（`evaluation_context/row_expression_context.rs` 等）：
   实现 `evaluate_label`（基于绑定变量的 tag 集合）、`evaluate_path_build`
   （基于绑定中的路径值）等；这些表达式在 MATCH/路径查询的投影阶段出现，
   行绑定内已有完整信息
2. **图算子专用上下文**（`evaluation_context/graph_storage.rs` 或
   `executor/traversal/`）：实现 `evaluate_list_comprehension`（对集合迭代 +
   子表达式求值）、`evaluate_predicate`（all/any/single/none 语义）、
   `evaluate_reduce`（累积归约）

### 2.3 列式批量求值支持

`chunk/eval.rs` 的 `evaluate_expressions` 对支持批量路径的表达式继续走列式，
对需要行级运行时上下文的表达式回退到逐行求值（复用 2.1 的 trait 方法）。
回退开关由表达式变体静态判定（编译期 match，无运行时开销）。

### 2.4 前置：使用路径核查

实施前先核查 binder 对这些表达式的产生路径：

- `rg "Expression::ListComprehension|Expression::Predicate|Expression::Reduce" crates/graphdb-query/src/query/binder`
- 确认哪些计划节点会携带这些表达式（Project 投影、Filter 谓词、WITH 子句等）
- 按实际出现路径确定各上下文实现的优先级，避免实现无消费方的能力

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 核查 binder 产生路径，确定优先级 | `query/binder/` |
| 2 | 扩展 `ExpressionContext` trait（默认 Unsupported） | `executor/expression/evaluator/traits.rs` |
| 3 | `expression_evaluator.rs` 分支改调 trait 方法 | `expression_evaluator.rs` |
| 4 | 行绑定上下文实现 Label / PathBuild / LabelTagProperty | `evaluation_context/` |
| 5 | 图算子上下文实现 Comprehension / Predicate / Reduce | `evaluation_context/graph_storage.rs`, `executor/traversal/` |
| 6 | 列式求值回退路径 | `chunk/eval.rs` |

## 4. 验证方法

- 单元测试：每个运行时上下文方法一条（正常值 + 绑定缺失错误路径）
- 表达式测试：MATCH 查询投影 ListComprehension / Predicate / Reduce 的执行结果
  与手工计算结果一致
- 回归：`cargo test -p graphdb-query` 全量
- 兼容性：所有现有 `ExpressionContext` 实现（默认 Unsupported）编译通过

## 5. 预期收益

- 消除通用求值器对 6 类表达式的功能性缺口（文档 4.3.7 项的基础）
- 列式批量求值覆盖范围扩大，与 DataChunk 向量化路径打通
- 错误信息从笼统的 "require runtime context" 变为精确的绑定/类型错误

## 6. 风险与回退

- **风险**：trait 默认实现掩盖未实现语义，出现"静默 Unsupported"。缓解：
  默认实现返回带表达式类型的错误信息；步骤 4/5 的消费方测试强制覆盖
- **回退**：trait 方法全部有默认实现，删除具体实现即恢复现状，无需改
  调用方
