# 因子化不变量改为显式错误：整改方案（R1–R6）

> 基线：`kkkqkx123/linkrs` @ `9eabc369`（main）
> 现状分析：见 `linkrs_query_analysis.md` §6.2（R1–R6）
> 规范依据：`AGENTS.md` —— 轻量单节点、No-backward-compatible、代码零中文、**禁 unwrap / 禁生产期 panic**
> 落地范围：仅 `crates/graphdb-query`；跨 crate 调用面已由既有 `OptimizeResult` 承载，无对外接口破坏。
> 编译核验：`cargo check -p graphdb-query -j 2` 与 `cargo check --tests -p graphdb-query -j 2` 均 **EXIT=0**（详见 §9）。

---

## 0. 结论速览

本轮**已落地代码**的核心是 **R2：把因子化不变量的 `assert!`/panic 改为 `Result`/错误返回**。之所以优先做 R2，是因为它直接违反 `AGENTS.md` 的"禁 unwrap/禁生产 panic"，且在坏计划下会 **abort 整个服务端进程**——是 R1–R6 中唯一"高优先级且可安全自动化改造"的一项。R1 以文档注释就地澄清；R3–R6 是设计取向问题，保留为开放点，不在本轮强推。

| 风险 | 主题 | 本轮处置 | 状态 |
|------|------|---------|------|
| **R1** | 命名/语义错位（因子化≠压缩因子表执行） | 在模块与不变量处补**澄清注释** | ✅ 已落地（文档） |
| **R2** | 不变量用 `assert!`/panic 而非 error | `FactorizedSchema` + `FactorizationGroup` **全部**校验/变换/登记方法改返回 `Result<_, FactorizationError>`；新增结构化错误枚举；`OptimizeError::FactorizationError` 保留**结构化变体**并接好 `source` 链 | ✅ 已落地（代码，含组级 API） |
| **R3** | 阻塞算子落盘不一致（仅 Distinct 落盘） | 方案条目 + 开放点 | ⏸ 待拍板 |
| **R4** | 阻塞物化态行存 vs 列存内存效率 | 方案条目 + 开放点 | ⏸ 待拍板 |
| **R5** | 两处巨型 match 漂移 | 方案条目 + 开放点 | ⏸ 待拍板 |
| **R6** | 别名三路径复杂度 | 方案条目 + 开放点 | ⏸ 待拍板 |

**一句话**：不变量违例从"进程级 panic"降级为"可恢复的 `OptimizeError`，最终以查询错误返回"，服务端不再因单个坏计划崩溃；R1 的语义澄清同时消除"看似有压缩因子表执行"的误读。

---

## 1. 问题定义（为什么必须改）

### 1.1 事实：旧实现用 `assert!`/panic

分析文档 §6.2 R2 指出：`FactorizedSchema` 的校验与变换方法在非法状态时 **panic**（`planning/plan/factorization.rs` 旧实现）：

- `flatten_group`：`assert!` 校验目标 group 合法 + `assert!` 校验 flatten 后仍满足"至多一个 unflat"。
- `validate_at_most_one_unflat`：`assert!(count <= 1, "at most one unflat group ...")`。
- `insert_to_group_and_scope` 系列：大量 `assert!`（重复登记、越界等）。

### 1.2 影响：坏计划 → 进程 abort

`FactorizationRewriter` 运行在优化器**末尾**（`optimizer/factorization/factorization_rewriter.rs:11-18`），由**任意**用户查询触发。一旦某个节点实现漏注册别名或推导出两个 unflat group，`assert!` 会让**整个服务端进程** `panic!`/`abort`，而不是把这一条查询判为失败。这与 `AGENTS.md` 的"禁 unwrap / 生产期禁 panic"直接冲突。

### 1.3 目标

1. 所有因子化不变量违例一律返回 `Result<_, FactorizationError>`，**不再 panic**。
2. 错误在因子化边界收敛为既有优化器错误类型，最终以查询错误返回，服务不崩。
3. 保持函数名/调用点形态基本不变（No-backward-compatible 允许签名变更），把改动面收敛在 `graphdb-query` 内。
4. R1：以注释就地澄清"本模块是规划侧 flatten 剪枝，非压缩因子表执行"，不改行为。

---

## 2. 设计：新增 `FactorizationError` 并全程传播

### 2.1 新增错误枚举（`planning/plan/factorization.rs:31`）

在 `factorization.rs` 顶部新增 `thiserror` 错误枚举（代码零中文，仅英文 doc 注释）：

```rust
/// Errors surfaced by factorization-schema validation and transformation.
///
/// These replace the former `assert!`/panic paths so that an invalid
/// factorized schema degrades to a recoverable optimizer error instead of
/// aborting the process (AGENTS.md: no production panics / no unwrap).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum FactorizationError {
    /// More than one group is left unflat at the same nesting level.
    #[error("at most one unflat group allowed, found {0}")]
    TooManyUnflatGroups(usize),
    /// A group that was expected to be flat is still unflat.
    #[error("group {0} expected flat but is unflat")]
    GroupExpectedFlat(FGroupPos),
    /// A group position passed to a flatten/set operation is out of range.
    #[error("group_pos {0} out of range")]
    GroupPosOutOfRange(FGroupPos),
    /// An expression was inserted into scope more than once.
    #[error("expression {0:?} already in scope")]
    ExpressionAlreadyInScope(ExpressionId),
    /// An expression was mapped to a group more than once.
    #[error("expression {0:?} already mapped to group")]
    ExpressionAlreadyMapped(ExpressionId),
    /// A duplicate expression id was registered inside a group.
    #[error("duplicate expression id {0:?} in group")]
    DuplicateExpressionId(ExpressionId),
    /// A duplicate expression name was registered inside a group.
    #[error("duplicate expression name {0} in group")]
    DuplicateExpressionName(String),
    /// `flatten_group` was given an invalid group position.
    #[error("flatten_group: invalid pos {0}")]
    InvalidFlattenPos(FGroupPos),
    /// `set_group_as_single_state` was given an invalid group position.
    #[error("set_group_as_single_state: invalid pos {0}")]
    InvalidSingleStatePos(FGroupPos),
    /// `set_flat` was called on a group that is already flat.
    #[error("group already flat")]
    GroupAlreadyFlat,
    /// `set_single_state` was called on a group that is already single-state.
    #[error("group already single state")]
    GroupAlreadySingleState,
    /// A group position set passed to a leader/validator was empty.
    #[error("groupPositions empty")]
    EmptyGroupPositions,
    /// A non-empty group position set was required but found empty.
    #[error("expected non-empty group positions")]
    NonEmptyGroupPositions,
}
```

> 已落地位置：`crates/graphdb-query/src/planning/plan/factorization.rs:31-68`。
> 13 个变体**全部可构造**（不再有"定义但未使用"的变体）：`TooManyUnflatGroups`/`GroupExpectedFlat`/`EmptyGroupPositions`/`NonEmptyGroupPositions` 由 `validate_*`、`get_leading_group_pos` 使用；`GroupPosOutOfRange`/`InvalidFlattenPos`/`InvalidSingleStatePos` 由 `flatten_group`/`set_group_as_single_state`/`append_flatten_if_necessary`/`insert_*` 使用；`ExpressionAlreadyMapped`/`DuplicateExpressionId`/`DuplicateExpressionName` 由 `insert_to_group_and_scope*`/`insert_expression*` 使用；`GroupAlreadyFlat`/`GroupAlreadySingleState` 由 `set_flat`/`set_single_state` 使用。

### 2.2 因子化边界错误桥接（**结构化变体**，`optimizer/error.rs:68`）

`OptimizeError` 新增一个**持有结构化错误**的变体；`#[from]` 自动生成 `From<FactorizationError>`，`#[source]` 接好 `Error::source()` 链——**不再 `to_string()` 压平**，上层可 `downcast` 到具体不变量违例：

```rust
// optimizer/error.rs
use crate::planning::plan::factorization::FactorizationError;

#[derive(Error, Debug, Clone)]
pub enum OptimizeError {
    // ... 既有变体 ...
    /// Factorization invariant violation (plan is malformed).
    ///
    /// Carries the structured `FactorizationError` rather than a flattened
    /// string so upstream layers can downcast and react to the specific
    /// invariant that was violated, while `Display`/`source` still render
    /// the full message.
    #[error("Factorization error: {0}")]
    FactorizationError(
        #[from]
        #[source]
        FactorizationError,
    ),
}
```

> 已落地位置：`crates/graphdb-query/src/optimizer/error.rs:11`（import）、`:68-80`（变体）。
> `From<FactorizationError> for OptimizeError` 由 `#[from]` **自动生成**（不再手写），`?` 在所有调用点照常工作。
> `FactorizationError` 满足 `Error + Clone + Send + Sync + 'static`，同时满足 `OptimizeError: Clone` 与 `QueryError::pipeline_optimization_error` 的 `Box<dyn Error + Send + Sync>`（`crates/graphdb-core/src/error.rs:69`）约束。

#### 2.2.1 端到端错误链（保留结构，不丢信息）

```
FactorizationError（结构化，如 GroupPosOutOfRange(99)）
   │  ? / #[from]
   ▼
OptimizeError::FactorizationError(inner)         ← source() 指向 inner
   │  QueryError::pipeline_optimization_error  (from_boxed 保留 source)
   ▼
QueryError { kind: Optimization, source: Box<OptimizeError> }
   │  DBError::from(QueryError)
   ▼
DBError → 用户可见（消息 + 错误码 ExecutionError）
```

- **正确性要点 1**：`QueryError::from_boxed`（`crates/graphdb-core/src/error/query.rs:302`）把整个 `OptimizeError` 装箱为 source，因此**源错误链完整**；调用方可 `source().downcast_ref::<OptimizeError>()` 再取 `FactorizationError` 做精细处理（如指标打点、重试判定）。
- **正确性要点 2**：`QueryError::to_public_message`（`query.rs:603`）对 `Optimization` 走 `self.message`（即 `OptimizeError` 的 Display），`to_error_code`（`query.rs:584`）映射为 `ExecutionError`——**对外不泄露内部结构**，只给稳定错误码 + 人类可读消息。
- **已知限制（记录，不改）**：`Clone for QueryError`（`query.rs:321`）刻意丢弃 `source`（保留 kind+message）。这是既有设计（小尺寸 + 可克隆），故"克隆后 downcast"不可用；如需在克隆后仍保留结构，应在克隆前完成 downcast。

### 2.3 签名变更清单（`planning/plan/factorization.rs`）

| 方法 | 旧签名 | 新签名 | 行 |
|------|--------|--------|----|
| `flatten_group` | `() -> ()` | `() -> Result<(), FactorizationError>` | `:420` |
| `flatten_all` | `() -> ()` | `() -> Result<(), FactorizationError>` | `:430` |
| `set_group_as_single_state` | `() -> ()` | `() -> Result<(), FactorizationError>` | `:442` |
| `validate_at_most_one_unflat` | `() -> ()` | `() -> Result<(), FactorizationError>` | `:517` |
| `flat_copy` | `() -> Self` | `() -> Result<Self, FactorizationError>` | `:536` |
| `get_leading_group_pos`（`SchemaUtils`） | `() -> FGroupPos` | `() -> Result<FGroupPos, FactorizationError>` | `:587` |
| `merge_schema`（`SinkOperatorUtil`） | `() -> ()` | `() -> Result<(), FactorizationError>` | `:659` |
| `recompute_schema`（`SinkOperatorUtil`） | `() -> ()` | `() -> Result<(), FactorizationError>` | `:742` |
| `FactorizedSchemaCompute::compute_factorized_schema` | `() -> FactorizedSchema` | `() -> Result<FactorizedSchema, FactorizationError>` | `:796` |
| `FactorizedSchemaCompute::compute_flat_schema` | `() -> FactorizedSchema` | `() -> Result<FactorizedSchema, FactorizationError>` | `:800` |

**第二轮补全：组级（`FactorizationGroup`）与登记（`FactorizedSchema`）API**——同属 R2，之前遗漏、本轮补齐（`assert!` → `Result`）：

| 方法 | 旧行为 | 新签名 | 错误变体 |
|------|--------|--------|---------|
| `FactorizationGroup::set_flat` | `assert!(!self.flat)` | `() -> Result<(), FactorizationError>` | `GroupAlreadyFlat` |
| `FactorizationGroup::set_single_state` | `assert!(!self.single_state)` | `() -> Result<(), FactorizationError>` | `GroupAlreadySingleState` |
| `FactorizationGroup::insert_expression` | 调用带名版 | `(ExpressionId) -> Result<(), FactorizationError>` | 透传 |
| `FactorizationGroup::insert_expression_with_name` | `assert!`（id/name 重复） | `(ExpressionId, Option<String>) -> Result<(), FactorizationError>` | `DuplicateExpressionId` / `DuplicateExpressionName` |
| `FactorizedSchema::insert_to_group_and_scope` | `assert!` | `(ExpressionId, FGroupPos) -> Result<(), FactorizationError>` | 透传 |
| `FactorizedSchema::insert_to_group_and_scope_with_name` | `assert!`（越界/重复映射） | `(...) -> Result<(), FactorizationError>` | `GroupPosOutOfRange` / `ExpressionAlreadyMapped` |
| `FactorizedSchema::insert_to_group_and_scope_batch` | `for` 循环 | `(Vec<ExpressionId>, FGroupPos) -> Result<(), FactorizationError>` | 透传 |
| `FactorizedSchema::insert_to_scope_may_repeat` | `assert!`（越界） | `(ExpressionId, FGroupPos) -> Result<(), FactorizationError>` | `GroupPosOutOfRange` |
| `FactorizedSchema::insert_to_group_and_scope_may_repeat` | 无校验 | `(ExpressionId, FGroupPos) -> Result<(), FactorizationError>` | `GroupPosOutOfRange` |
| `FactorizationRewriter::append_flatten_if_necessary` | `assert!`（越界） | `(...) -> Result<LogicalNodeEnum, FactorizationError>` | `GroupPosOutOfRange` |
| `FactorizationRewriter::append_flattens` | `() -> LogicalNodeEnum` | `(...) -> Result<LogicalNodeEnum, FactorizationError>` | 透传 |
| `FactorizationRewriter::replace_child_and_flatten` | `()` | `(...) -> Result<(), FactorizationError>` | 透传 |
| `FactorizationRewriter::replace_node_and_flatten` | `()` | `(...) -> Result<(), FactorizationError>` | 透传 |
| `FactorizationRewriter::flatten_barrier_child/_single/_binary` | `()` | `(...) -> Result<(), FactorizationError>` | 透传 |

> 全部为 `graphdb-query` crate 内方法/trait，无外部调用者；跨 crate 只经 `optimize()`（返回 `OptimizeResult`）。
> **落地后核验**：`grep` 确认生产代码（`#[cfg(test)]` 之前）已无 `assert!`/`panic!`（仅保留 2 处 `debug_assert!`，见 §10 开放点 7）。

### 2.4 关键实现原则（避免"过 unwrap"）

`factorization_compute` 的巨型 `match` 分发里，**分支终值**（`node.compute_factorized_schema(child_schemas)` / `return node.compute_factorized_schema(...)`）本身**现在就是 `Result`**，因此**不能再加 `?`**；只有**被绑定的调用**（`let x = foo(...)?`）与**语句调用**（`foo(...)?;`）才加 `?`。这是本轮最容易写错、也最返工的一点（见 §8）。

---

## 3. 改动清单（按文件）

### 3.1 核心 schema 与 compute

- **`planning/plan/factorization.rs`**：新增 `FactorizationError`（`:31`）；校验/变换方法改返回 `Result`（§2.3）；`assert!` → `ok_or(...)?` / `Err(...)`。示例：

```diff
-    pub fn flatten_group(&mut self, pos: FGroupPos) {
-        assert!(pos < self.groups.len(), "flatten_group: invalid pos {pos}");
+    pub fn flatten_group(&mut self, pos: FGroupPos) -> Result<(), FactorizationError> {
+        let group = self
+            .groups
+            .get_mut(pos)
+            .ok_or(FactorizationError::InvalidFlattenPos(pos))?;
         // ... flatten ...
-        assert!(
-            self.has_at_most_one_unflat(),
-            "at most one unflat group allowed"
-        );
+        if let Some(n) = self.too_many_unflat_count() {
+            return Err(FactorizationError::TooManyUnflatGroups(n));
+        }
+        Ok(())
     }
```

```diff
-    pub fn validate_at_most_one_unflat(&self) {
-        assert!(self.has_at_most_one_unflat(), "at most one unflat group allowed");
+    pub fn validate_at_most_one_unflat(&self) -> Result<(), FactorizationError> {
+        match self.too_many_unflat_count() {
+            Some(n) => Err(FactorizationError::TooManyUnflatGroups(n)),
+            None => Ok(()),
+        }
     }
```

- **`planning/plan/factorization.rs`（组级 API 第二轮补全）**：`FactorizationGroup::{set_flat, set_single_state, insert_expression, insert_expression_with_name}` 与 `FactorizedSchema::{insert_to_group_and_scope*, insert_to_scope_may_repeat}` 由 `assert!` 改 `Result`（§2.3 第二表）；生产调用点（`access.rs`/`assign.rs`/`operation.rs`/`set_ops.rs`/`flat_leaf.rs`/`join.rs`/`unwind.rs`）全部加 `?` 传播。
- **`planning/plan/factorization_compute.rs`**：trait 两方法改返回 `Result`（`:94`、`:212`）；巨型 `match` 各分支终值直接返回 `Result`（不加 `?`）；被绑定调用加 `?`；`compute_flat_schema` 里 `child.flat_copy()?` 传播；`flat_leaf`/`merge`/`flatten_all_from_child` 等 helper 同步改签名。
- **`planning/plan/factorization_compute/{access,assign,control_flow,flat_leaf,join,operation,set_ops,traversal,unwind}.rs`**：各节点 `compute_factorized_schema` 实现同步改返回 `Result`；`traversal.rs` 的 `bi_expand`/`bi_traverse` 终值直接返回 `Result`（**不加 `?`**）。

### 3.2 两个重写器（生产链路）

- **`optimizer/factorization/factorization_rewriter.rs`**：
  - `rewrite` → `Result<(), FactorizationError>`；`return;` → `return Ok(());`；`let _ = self.visit_operator(plan);` → `self.visit_operator(plan)?; Ok(())`。
  - `visit_operator` → `Result<FactorizedSchema, FactorizationError>`；被绑定的 `self.visit_operator(...)` / `flatten_group(...)` / `flatten_all()` 加 `?`；分支终值 `node.compute_factorized_schema(...)` **保持不加 `?`**。
  - 5 个 join helper（`visit_hash_join_inner/left/generic_inner/right/full_outer`）→ `Result<(), FactorizationError>`，体末 `Ok(())`，调用点加 `?`。
- **`optimizer/factorization/remove_factorization_rewriter.rs`**：
  - `rewrite` → `Result<(), FactorizationError>`（`Self::visit_operator(old)?; Ok(())`）。
  - `visit_operator`/`visit_operator_replace` → `Result<(LogicalNodeEnum, FactorizedSchema), FactorizationError>`；绑定/语句调用加 `?`；多依赖 `.collect()` 改为 `Result` collect。

### 3.3 优化器边界与 schema 树

- **`optimizer/engine.rs`**：
  - `apply_remove_factorization`（`:1136`）与 `apply_factorization`（`:1146`）→ `OptimizeResult<ExecutionPlan>`；`rewrite(&mut logical.root)?`；末尾 `Ok(plan)`。
  - 调用点 `optimize_with_layout`：`:458` `apply_remove_factorization(current_plan)?;`、`:475` `apply_factorization(current_plan)?;`。
  - `compute_schema_tree`（`:1376`）→ `Result<FactorizedSchema, FactorizationError>`；递归 `.map(...).collect::<Result<Vec<_>, _>>()?`；终值直接返回 `Result`。
  - `validate_factorized_invariant`（`:1369`）从 `catch_unwind(...).is_ok()` 改为 `Self::compute_schema_tree(root).is_ok()`（方法不再 panic）。
- **`planning/join_order/plan_join_order.rs`**：
  - `compute_schema_for_plan`（`:321`）→ `Result<FactorizedSchema, FactorizationError>`；递归调用加 `?`。
  - `encode_plan`（`:256`）**故意保留 `u64` 返回**（有 ~11 个调用者）；改为**降级不 panic**：

```diff
-        let schema = Self::compute_schema_for_plan(&mut owned);
+        let schema = match Self::compute_schema_for_plan(&mut owned) {
+            Ok(s) => s,
+            // Degrade to all-unflat on error; encode_plan must stay infallible
+            // because join-order encoding has many callers that cannot fail.
+            Err(_) => crate::planning::plan::factorization::FactorizedSchema::new(),
+        };
```

### 3.4 WCOJ 交互

- **`planning/plan/logical/logical_nodes/wco_intersect.rs`**（测试区 `:248-249`）：调用加 `.unwrap()`（见 §4）。

---

## 4. 测试代码适配（`cfg(test)` + 集成测试）

生产库改返回 `Result` 后，测试里对以下方法的调用需补 `.unwrap()`：`validate_at_most_one_unflat` / `validate_no_unflat` / `flat_copy` / `flatten_group` / `flatten_all` / `get_leading_group_pos` / `merge_schema` / `recompute_schema` / `set_group_as_single_state` / `compute_factorized_schema` / `compute_flat_schema` / `rewrite` / `visit_operator` / `apply_factorization` / `apply_remove_factorization` / `compute_schema_tree` / `compute_schema_for_plan` / **`insert_to_group_and_scope*` / `insert_to_scope_may_repeat` / `insert_expression*` / `set_flat` / `set_single_state` / `append_flatten_if_necessary`** / `SinkOperatorUtil::recompute_schema` / 自由函数 `sort`。

**处理原则**：
1. 仅在 `#[cfg(test)]` 模块与集成测试文件（`crates/graphdb-query/tests/*.rs`）内插入 `.unwrap()`，**绝不触碰生产代码**（避免破坏生产链路里如 `compute_flat_schema` 内的 `cs.flat_copy()?`）。
2. 已消费的调用（`.unwrap/.expect/?/.is_err/.is_ok/.ok/.err/.unwrap_or...`）跳过，保证幂等、不产生 `??` 或双 unwrap。
3. **旧 panic 测试必须重写**（方法不再 panic）。共 4 处：

```diff
     #[test]
     fn schema_at_most_one_unflat_invariant() {
         // ...
-        // Two unflat groups should panic on validate.
-        let result = std::panic::catch_unwind(|| schema.validate_at_most_one_unflat().unwrap());
-        assert!(result.is_err());
+        // Two unflat groups must be rejected by validate (it returns Result, no panic).
+        assert!(schema.validate_at_most_one_unflat().is_err());
     }

     #[test]
-    #[should_panic(expected = "at most one unflat group")]
     fn flatten_group_validates_invariant_at_runtime() {
         // ...
-        schema.flatten_group(flat_pos).unwrap();
+        assert!(schema.flatten_group(flat_pos).is_err());
     }
```

其余三处 `#[should_panic(expected = "out of range")]` 改为断言**结构化变体**：

```diff
     #[test]
-    #[should_panic(expected = "out of range")]
     fn flatten_out_of_range_reports() {
         // ...
-        let _ = flatten.compute_factorized_schema(&[schema]).unwrap();
+        let err = flatten.compute_factorized_schema(&[schema])
+            .expect_err("out-of-range flatten must fail");
+        assert_eq!(err, FactorizationError::GroupPosOutOfRange(99));
     }
```

> 已落地位置：
> - `planning/plan/factorization.rs` 的 `schema_at_most_one_unflat_invariant` / `flatten_group_validates_invariant_at_runtime`。
> - `planning/plan/factorization_compute.rs` 的 `flatten_out_of_range_reports`（断言 `GroupPosOutOfRange(99)`）。
> - `optimizer/factorization/factorization_rewriter.rs` 的 `append_flatten_out_of_range_reports`（断言 `GroupPosOutOfRange(99)`）。
> - `tests/factorization_row_equivalence.rs` 的 `flatten_group_out_of_range_is_hard_error`。
>
> 另补：`SchemaUtils::get_leading_group_pos(...).unwrap()`、`SinkOperatorUtil::recompute_schema(...).unwrap()` 等自由函数/关联函数调用。
>
> **与 R2 无关、保持不动的 `#[should_panic]`**（不同不变量、未触及）：`join_operator.rs:1072`（row/column mismatch）、`wco_intersect.rs:184`（empty build side）。

**改动文件（测试面）**：
- 单元测试：`planning/plan/factorization.rs`、`planning/plan/factorization_compute.rs`、`.../operation.rs`、`.../wco_intersect.rs`、`optimizer/factorization/{factorization_rewriter,remove_factorization_rewriter,flatten_resolver,group_dependency_analyzer}.rs`、`optimizer/engine/tests.rs`
- 集成测试：`tests/factorization_schema.rs`、`tests/factorization_schema_compute.rs`、`tests/factorization_row_equivalence.rs`

---

## 5. R1：命名/语义澄清（已落地）

在易误读处补注释（不改行为），明示"规划侧 flatten 剪枝 ≠ 压缩因子表执行"：

- `optimizer/factorization/factorization_rewriter.rs` 模块首注释处，补充一句：本重写器负责 **factorized-plan 描述与最小化 flatten**，**不**实现 Kuzu/Ladybug 的压缩因子表 / `SEMI_MASKER` 执行。
- `planning/plan/factorization.rs` 的 `FactorizedSchema` doc 注释处，补充：仅在**逻辑计划层**维持分组结构，不进入存储/执行器。

> 承 `linkrs_query_analysis.md` §2.6 / R1：linkrs 的"因子化"收益是"少物化几次、每次少扇出一点"，与 `docs/analysis/因子化_重命名_扩展机制引入影响分析.md §2` 的 "P3 不引入真·因子化执行" 判定一致。

---

## 6. R3–R6：R3/R4 已立项，其余保留为设计提案

### 6.1 R3 阻塞算子落盘不一致（中）

现状：`Distinct` 支持落盘（`executor/streaming/operators/blocking/materialize_operator.rs:57`/`:399`），而 `Materialize`/`DataCollect`/`RollUpApply` 的 `spill_*` 走 `spill_not_supported`（`materialize_operator.rs:417-439`）。
提案：为这三类补 `HashPartitionSpiller` 支持，复用 `Distinct` 的分区溢写/回放框架；超 `MemoryTracker` 预算时溢写而非报错。验收：大结果集 `Materialize` 在低内存预算下不 OOM、结果正确。**处置：与 R4 合并立项**（落盘格式统一定为列式 run，见 §6.2 设计文档 §3.4）。

### 6.2 R4 阻塞物化态行存→列存（中）—— 已出设计文档

现状：流式用列存 `DataChunk`，阻塞退回 `Vec<Vec<Value>>` + `HashSet<Vec<Value>>`（`blocking/materialize.rs:9`/`:22`），交叉积展开（`operators/flatten.rs:52`/`:110`）再回行，分配与局部性差。
提案：阻塞物化态改列存 `MaterializedBatch`，`Flatten` 直接对列批次做选择向量重放。**与 `graphdb-storage` 列存重构合并立项**，共享列存底座。
**设计文档**：[`docs/plan/columnar_materialization_state_design.md`](./columnar_materialization_state_design.md)（**下一阶段任务**）。

### 6.3 R5 巨型 match 漂移（中）

现状：`compute_factorized_schema`（`factorization_compute.rs` 巨型 `match`）与 `FactorizationRewriter::visit_operator` 都按 `LogicalNodeEnum` 分发，新增节点易只改一处。
提案：用 visitor trait 收敛两边分发的"算子类别 → 行为"表（单输入按需 flatten / 双输入 join / 单输入屏障 / 双输入屏障 / flat leaf），并对未覆盖分支 `unreachable!` + 单测枚举全部变体。

### 6.4 R6 别名三路径（低中）

现状：`ExpressionId` 主路径 + 变量名 fallback + bare name（`factorization.rs` 的 `insert_name_for_group`），`group_dependency_analyzer.rs` 双路径查询。
提案：以单一 `AliasRegistry`（id↔name↔group 三向映射）收敛三路径，`mark_unresolved` 保守全拍平的安全网保留。**风险：触碰核心正确性，需大范围回归，建议排在 R3/R4 之后。**

---

## 7. 兼容性与影响面

| 维度 | 结论 |
|------|------|
| 对外接口 | 无破坏。跨 crate 只经 `optimize()`（`OptimizeResult`）；新增 `From<FactorizationError>` 桥接 |
| 行为语义 | 不变量违例由"panic/abort"变为"返回错误"；合法计划路径**完全不变** |
| 影响 crate | 仅 `graphdb-query`（`graphdb-sync` 为其依赖，未调用被改方法，不受影响） |
| 性能 | 无热点开销：错误路径仅在非法状态触发；正常路径仅多了 `Result` 包装与 `Ok` |
| No-backward-compatible | 允许函数签名变更；未保留旧 panic 行为 |

---

## 8. 实施踩坑记录（供复现）

1. **分支终值不能加 `?`**：`match` 分支返回 `node.compute_factorized_schema(...)` 时，该表达式类型**已是 `Result`**；再加 `?` 会 `E0308/E0277`。只有被绑定调用（`let x = ...?`）与语句调用（`foo()?;`）才加 `?`。
2. **`rewrite` 的裸 `return;`**：改 `Result` 后须 `return Ok(());`。
3. **`traversal.rs` 的 `bi_expand`/`bi_traverse`** 终值直接返回 `Result`，**不要** `?`。
4. **嵌套括号**：`self.visit_operator(n.left_input_mut())?` —— 单个 `?` 落在 `visit_operator(...)` 的 `Result` 上，**不能**写成 `left_input_mut()?)`（会把 `left_schema` 变成 `Result`）。
5. **join helper 返回类型**：5 个 `visit_hash_join_*` 内部用了 `flatten_group(...)?`，故其返回类型必须同步升为 `Result<(), FactorizationError>` 并在调用点加 `?`。
6. **`encode_plan` 不回 `Result`**：约 11 个调用者无法失败，故对 `compute_schema_tree` 错误**降级为全 flat**，保持 `u64` 返回。
7. **结构化变体的 `#[source]`**：`thiserror` 对 `#[error("...{0}")]` 的元组变体**不会**自动把它设为 `source()`；必须显式 `#[source]`（本轮已加），否则"克隆/传播后 downcast"会失效。
8. **多行调用与 `expect_err`**：行级脚本会漏掉多行调用（如 `foo(\n  a,\n b,\n);`）与 `let err = x.expect_err(...)`（脚本误加 `.unwrap()`）；本轮已手工修正，落地时需逐项复核。
9. **⚠️ 栈帧膨胀导致测试线程栈溢出（本轮发现的真实回归）**：`RemoveFactorizationRewriter::visit_operator_replace` 是一个 ~40 分支的巨型 `match`，各分支在递归调用期间各自持有大型具体节点临时量；编译器会为**所有分支的并集**在该函数的一帧内预留栈空间。签名从 `(LogicalNodeEnum, FactorizedSchema)` 升为 `Result<..>` 后，每个递归帧再叠加一层 `Result` 判别式与更多同时存活的分支临时量，**实测**：
   - 基线（HEAD）最小栈需求 ≈ **1.31 MiB**（默认 2 MiB 测试线程栈刚好够）；
   - 仅加 `?`、不重构后 ≈ **2.6 MiB** → 默认 2 MiB 栈**溢出**（`dql::aggregation::test_group_by_execution` abort）。
   **修复**：把三类形态**统一**的分支（单输入 17、deps 向量 9、二元 14）抽到 `#[inline(never)]` 薄封装 `visit_single_input` / `visit_deps` / `visit_binary`，递归由封装函数发起，巨型 `match` 帧不再承载所有分支临时量的并集。修复后最小栈需求 ≈ **0.79 MiB**，**优于基线约 40%**。
   **配套**：为 `LogicalSingleInputNode` 增加 `take_input()`（非 panic 取子，替代裸字段 `input.take()`，使封装可泛型化）；`LogicalBinaryInputNode` 用 `LogicalNodeEnum::default()` 哨兵 + `std::mem::replace` 取左/右子。
   **教训**：递归 + 巨型 `match` 的函数，任何"包一层 `Result`/元组"的签名变化都可能放大栈帧；此类函数应尽早把统一分支下沉为 `#[inline(never)]` 薄封装（与 R5 visitor 收敛同源）。

---

## 9. 编译与测试核验

```bash
# 全局 ~/.cargo/config.toml：crates.io -> rsproxy.cn sparse
# 容器内增量编译缓存（incremental）在 overlayfs 上会触发 rustc SIGBUS，
# 必须 CARGO_INCREMENTAL=0 关闭增量编译后再编译/测试。
CARGO_INCREMENTAL=0 cargo check -p graphdb-query -j 2            # EXIT=0（仅 graphdb-sync 既有告警）
CARGO_INCREMENTAL=0 cargo check --tests -p graphdb-query -j 2    # EXIT=0，无 error、无 warning
CARGO_INCREMENTAL=0 cargo test  -p graphdb-query -j 1            # 全绿：
#   lib 2043 passed; dql 190 passed; 其余集成测试共 ~1100 passed; 0 failed
```

> 容器环境统一限并发（`-j 1`/`-j 2`）避免系统崩溃；并**必须** `CARGO_INCREMENTAL=0`（否则 rustc/ld 在增量缓存 mmap 上 SIGBUS）。
> 生产代码扫描：`#[cfg(test)]` 之前已无 `assert!`/`panic!`；仅保留 2 处 `debug_assert!`（`factorization_compute.rs:205` 与 `remove_factorization_rewriter.rs` 的 `rewrite`，开发期兜底，见开放点 7）。
> **栈回归专项核验**：`dql::aggregation::test_group_by_execution` 在默认 2 MiB 测试栈下通过；`RUST_MIN_STACK` 二分测得修复后阈值 ≈ 0.79 MiB（§8.9）。
>
> **patch 自验**：`git add -N docs/plan/... && git diff > linkrs-changes.patch`（排除构建产物/core dump），随后 `git stash push -u` → `git apply --check` ✅ → `git apply` ✅ → `CARGO_INCREMENTAL=0 cargo check -p graphdb-query` ✅ → `git stash drop`。patch 可干净应用到 `9eabc369` 基线。

---

## 10. 待拍板开放点清单

1. ~~**R2 错误粒度**~~ **✅ 已定**：`OptimizeError::FactorizationError` 保留**结构化变体** `FactorizationError`（`#[from]` + `#[source]`），不再字符串化；`QueryError` 端到端保留 `source` 链（§2.2.1）。**残留子问题**：`Clone for QueryError` 丢弃 `source`（既有设计），是否需要在克隆前完成 downcast 由调用方约定。
2. **`encode_plan` 降级策略**：出错时"降级为全 flat"是否可接受？是否应改为向上返回错误、由调用者决定（需改 ~11 处调用者）？
3. **R3 落盘**：`Materialize`/`DataCollect`/`RollUpApply` 统一落盘的优先级与排期？**（与 R4 合并立项，见 §6.2 设计文档）**
4. ~~**R4 列存物化态**~~ **✅ 已立项**：与 `graphdb-storage` 列存重构合并立项，设计文档见 [`docs/plan/columnar_materialization_state_design.md`](./columnar_materialization_state_design.md)。
5. **R5 visitor 收敛**：是否近期引入 `LogicalNodeEnum` 分发 visitor trait，并加"全变体覆盖"单测？
6. **R6 别名收敛**：是否立项统一 `AliasRegistry`？风险（核心正确性）与收益评估。
7. **debug_assert 去留**：`factorization_compute.rs` 与 `remove_factorization_rewriter.rs` 的 `debug_assert!` 是否保留为开发期兜底？（建议保留：release 不触发，开发期可早发现。）
8. **`insert_expression_with_name` 的 `name.clone()`**：转换后先查重再插入，保留了一次 `name.clone()`（用于错误路径与插入）；可评估是否用 `entry` API 消除该克隆（微优化）。
9. **`ExpressionAlreadyInScope` 变体**：已定义但当前 `insert_to_group_and_scope_with_name` 走 `ExpressionAlreadyMapped`；是否需要在"仅 scope 重复"场景构造 `ExpressionAlreadyInScope` 以更精确区分？（当前语义足够，暂不拆。）

---

## 11. 核验记录（file:line）

| 事实 | 引证 |
|------|------|
| 新增 `FactorizationError` 枚举（13 变体） | `crates/graphdb-query/src/planning/plan/factorization.rs:31-68` |
| `OptimizeError::FactorizationError` **结构化变体**（`#[from]`+`#[source]`） | `crates/graphdb-query/src/optimizer/error.rs:68-80` |
| `BoxedError = Box<dyn Error + Send + Sync>`（source 约束） | `crates/graphdb-core/src/error.rs:69` |
| `QueryError::from_boxed` 保留 source | `crates/graphdb-core/src/error/query.rs:302` |
| `QueryError::pipeline_optimization_error` | `crates/graphdb-core/src/error/query.rs:499` |
| `QueryError::to_public_message`/`to_error_code` | `crates/graphdb-core/src/error/query.rs:603`, `:584` |
| `FactorizationGroup::set_flat`/`set_single_state` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:117`, `:122` |
| `FactorizationGroup::insert_expression_with_name` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:158` |
| `FactorizedSchema::insert_to_group_and_scope_with_name` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:299` |
| `insert_to_scope_may_repeat`/`insert_to_group_and_scope_may_repeat` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:335`, `:341` |
| `FactorizationRewriter::append_flatten_if_necessary` 返回 `Result` | `crates/graphdb-query/src/optimizer/factorization/factorization_rewriter.rs:1228` |
| `append_flattens`/`replace_child_and_flatten`/`replace_node_and_flatten` 返回 `Result` | `crates/graphdb-query/src/optimizer/factorization/factorization_rewriter.rs:1179`, `:1200`, `:1216` |
| `flatten_group`/`flatten_all` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:420`, `:430` |
| `validate_at_most_one_unflat` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:517` |
| `flat_copy` 返回 `Result<Self, _>` | `crates/graphdb-query/src/planning/plan/factorization.rs:536` |
| `SchemaUtils::get_leading_group_pos` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:587` |
| `SinkOperatorUtil::merge_schema/recompute_schema` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:659`, `:742` |
| trait `compute_factorized_schema/compute_flat_schema` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:796`, `:800` |
| `apply_remove_factorization`/`apply_factorization` 返回 `OptimizeResult` | `crates/graphdb-query/src/optimizer/engine.rs:1136`, `:1146` |
| 生产链路调用点加 `?` | `crates/graphdb-query/src/optimizer/engine.rs:458`, `:475` |
| `compute_schema_tree` 返回 `Result` + `Result` collect | `crates/graphdb-query/src/optimizer/engine.rs:1376-1387` |
| `validate_factorized_invariant` 去 `catch_unwind` | `crates/graphdb-query/src/optimizer/engine.rs:1369-1373` |
| `encode_plan` 保持 `u64`，错误降级全 flat | `crates/graphdb-query/src/planning/join_order/plan_join_order.rs:256-263` |
| `compute_schema_for_plan` 返回 `Result` | `crates/graphdb-query/src/planning/join_order/plan_join_order.rs:321-340` |
| panic 测试重写为 `is_err()` | `crates/graphdb-query/src/planning/plan/factorization.rs:858-883` |
| **栈修复**：`LogicalSingleInputNode::take_input`（非 panic 取子） | `crates/graphdb-query/src/planning/plan/logical/logical_node_traits.rs:32` |
| `take_input` 宏实现（SingleInputNode） | `crates/graphdb-query/src/planning/plan/logical/logical_macros.rs:258` |
| `take_input` 手写实现（Flatten/Union/Minus/Intersect） | `.../logical_nodes/flatten.rs:147`；`.../logical_nodes/graph_ops.rs:43`, `:146`, `:185` |
| **栈修复**：`visit_single_input`/`visit_deps`/`visit_binary`（`#[inline(never)]`） | `crates/graphdb-query/src/optimizer/factorization/remove_factorization_rewriter.rs:362`, `:383`, `:408` |
| `remove_factorization_rewriter` 签名升 `Result` + `#[inline(never)]` 封装 | `.../remove_factorization_rewriter.rs:25`, `:38`, `:42` |
| 生产调用点正确传播 `?` | `crates/graphdb-query/src/optimizer/engine.rs:1138-1139` |
| R1 语义澄清（分析） | `linkrs_query_analysis.md` §2.6、§6.2 R1 |
| R3 落盘不一致 | `crates/graphdb-query/src/executor/streaming/operators/blocking/materialize_operator.rs:57`, `:399`, `:417-439` |
| R4 行存物化态 | `crates/graphdb-query/src/executor/streaming/operators/blocking/materialize.rs:9`, `:22` |

---

## 12. 剩余问题修改方向与本轮落地（R4 除外）

> R4（阻塞物化态行存→列存）已独立立项（§6.2 设计文档），本节不涉 R4。
> 方向原则：凡触碰执行语义或大范围正确性的（统一落盘、visitor 收敛、AliasRegistry）
> 只做“可验证的最小收敛 + 显式 deferred”，把行为变化收敛到错误显式化与查表顺序统一。

### 12.1 R2 残留：三个登记方法仍 `assert!`（已补齐）

**方向**：`insert_to_scope` / `insert_to_scope_with_name` / `insert_name_for_group`
与既有 R2 同策——一律返回 `Result<_, FactorizationError>`，越界报
`GroupPosOutOfRange`，重复报 `ExpressionAlreadyMapped`（§12.3 细化）。
`register_output_names` 与 `SinkOperatorUtil::remap_names` 同步改为
`Result` 并用 `?` 传播；`operation` / `assign` / `flat_leaf` /
`set_ops` / `traversal` / `unwind` / `access` 调用点全部加 `?`。

**落地**：`planning/plan/factorization.rs`（三方法签名 + 内部 `?`）、
`factorization_compute.rs`（`register_output_names`、`bi_expand_schema`）、
各子模块调用点；测试补 `scope_and_bare_name_registration_reject_out_of_range`。

### 12.2 开放点 8：`insert_expression_with_name` 的 `name.clone()`（已消除）

**方向**：按 `Option<String>` 所有权消费——先查重、再直接 `insert(n, …)`，
不再为查重 clone。`insert_to_group_and_scope_with_name`
仍需一次 `name.clone()`（group 插入与 schema 两级 map 各消费一次），
属必要拷贝，不再优化。

### 12.3 开放点 9：`ExpressionAlreadyInScope` 未使用（已接线）

**方向**：精确区分两层重复——`expression_to_group` 已有映射报
`ExpressionAlreadyMapped`；仅 `expressions_in_scope` 含（映射缺失的
不一致态）报 `ExpressionAlreadyInScope`。`insert_to_scope` 与
`insert_to_group_and_scope_with_name` 均实现该顺序。
测试 `duplicate_scope_registration_distinguishes_mapping_from_scope` 覆盖两种变体。

### 12.4 开放点 2：`encode_plan` 降级策略（已定：保持 `u64` + 可见降级）

**方向**：不改 ~11 个调用者。`encode_plan` 保持 infallible，错误时降级全 flat
（仅合并 DP 候选，不改行语义），并加 `log::warn!` 使降级可见而非静默。
位置：`planning/join_order/plan_join_order.rs:encode_plan`。

### 12.5 R3 阻塞算子落盘（最小显式化；统一落盘 deferred 到列存）

**方向**：在列存底座（R4）落地前，不为 `Materialize` / `DataCollect` /
`RollUpApply` 补分区溢写。改为显式错误：`spill_not_supported` 首参新增
`operator: &'static str`，三算子（含内存预算 breaches 的三处 `next_*`
调用点）各自报 `Spill is not implemented for blocking operator {op}…`，
超预算时返回错误而非 OOM/部分落盘。统一 `HashPartitionSpiller` 复用
待 R4 列存 run 格式确定后立项。

### 12.6 R5 巨型 match 漂移（漂移 guard 已加；visitor trait deferred）

**方向**：完整 visitor 收敛风险高（两处分发语义不完全同构），暂缓。
最小收敛：新增 `factorization_compute::dispatch_module`——对
`LogicalNodeEnum` 全变体无通配 exhaustive 匹配，返回 Owning 子模块名；
`compute_factorized_schema` 入口以 `debug_assert!` 链接该表；
单测 `dispatch_module_stays_in_sync_with_compute` 覆盖代表分支。
新增变体时此处编译失败，强制同步更新两处分发。

### 12.7 R6 别名三路径（单点收敛已加；`AliasRegistry` deferred）

**方向**：完整三向 `AliasRegistry` 触碰核心正确性，排在 R3/R4 之后。
最小收敛：新增 `FactorizedSchema::resolve_group_pos(id, name)` 作为唯一
规范顺序（id → id-linked name → bare name）；`GroupDependencyAnalyzer::visit`
与 `visit_expression(Variable)` 改走该函数，不再各自直查 map。
`mark_unresolved` 保守全拍平安全网保留。

### 12.8 开放点 7：`debug_assert!` 去留（已定：保留）

**方向**：保留。`factorization_compute.rs` 的后条件断言与
`dispatch_module` 链接断言、`remove_factorization_rewriter.rs` 的开发期
兜底均为 `debug_assert!`——release 零成本，开发期早发现；另 `remove_*`
的 `rewrite` 断言同样保留。

### 12.9 核验

```bash
CARGO_INCREMENTAL=0 cargo check -p graphdb-query -j 2            # EXIT=0
CARGO_INCREMENTAL=0 cargo check --tests -p graphdb-query -j 2    # EXIT=0
CARGO_INCREMENTAL=0 cargo test -p graphdb-query -j 1 --lib       # 2047 passed, 0 failed
CARGO_INCREMENTAL=0 cargo test -p graphdb-query -j 1 --test factorization_schema --test factorization_schema_compute --test factorization_row_equivalence  # 全绿
CARGO_INCREMENTAL=0 cargo test -p graphdb-query -j 1 --test dql  # 190 passed
```

| 新增事实 | 引证 |
|------|------|
| `insert_to_scope` / `insert_to_scope_with_name` / `insert_name_for_group` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization.rs:276`, `:295`, `:417` |
| `resolve_group_pos` 规范顺序 | `crates/graphdb-query/src/planning/plan/factorization.rs:resolve_group_pos` |
| `register_output_names` 返回 `Result` | `crates/graphdb-query/src/planning/plan/factorization_compute.rs:30` |
| `dispatch_module` 穷尽匹配 + `debug_assert` 链接 | `crates/graphdb-query/src/planning/plan/factorization_compute.rs:62`, `:~186` |
| `encode_plan` 降级加 warn | `crates/graphdb-query/src/planning/join_order/plan_join_order.rs:encode_plan` |
| `spill_not_supported(operator, …)` 显式算子名 | `crates/graphdb-query/src/executor/streaming/operators/blocking/helpers.rs:41`；`materialize_operator.rs:422`, `:430`, `:438` + 三处 `next_*` |
| `GroupDependencyAnalyzer` 走 `resolve_group_pos` | `crates/graphdb-query/src/optimizer/factorization/group_dependency_analyzer.rs:visit`, `:visit_expression` |
