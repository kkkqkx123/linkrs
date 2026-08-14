# 执行器算子上下文重构：拆分 OperatorBase + left/right 角色显式化

> 状态：设计方案（2026-08-14）。
>
> 针对 `streaming/operators` 两个核查结论做彻底重构：
>
> 1. **base 参数问题**：五个生命周期方法统一传 `&mut OperatorBase`，
>    但 38% 的用法是仪式性 lifecycle 标记、部分算子完全不用
>    （`_base` 8 处）；`&mut` 过约束热路径；算子状态双栖（self /
>    runtime arena）使统一签名隐藏真实依赖。
> 2. **left/right 参数问题**：hash join 构建侧硬编码为物理右子节点
>    （角色由位置隐式编码）、`JoinCtx` 混合上下文/子树/自身状态三类
>    归属、`left_consumed` 命名与语义错位、`Except/Minus` 在 `next`
>    内临时 open/close 右子树绕过生命周期协议。
>
> 目标：**算子的 `next()` 热路径零上下文参数**；生命周期状态机收归
> executor 独占；join 的 build/probe 角色显式化并加校验。

## 0. 结论摘要

| 项 | 现状 | 决策 |
|----|------|------|
| A | `open/next/stop/reset/close` 统一传 `&mut OperatorBase`，大量未用/仪式性 | 拆分：不可变配置构造期注入算子字段；`runtime` 句柄 open 时注入 self；**`next` 无上下文参数** |
| B | lifecycle 由 executor（executor.rs:723/727/751/776）与算子（177 处引用中的写入）双重写入 | 状态机收归 executor 独占，算子所有 lifecycle 写入删除 |
| C | hash join build 侧硬编码为物理右子节点（hash_join.rs:25） | `JoinSpec` 增 `build_side: BuildSide` 角色字段，实现按角色绑定输入，`debug_assert` 校验键一致性 |
| D | `JoinCtx` 混合 base/children/self-state（join_operator.rs:23-29） | 删除 JoinCtx；算法函数直接以 self 状态字段 + children 传参 |
| E | `Except/Minus` 在 `next` 内临时 `right.open()/close()`（set_operator.rs:296-306） | 子树生命周期统一由 open/close_tree 管理，next 内删除临时 open/close |
| F | `left_consumed` 在 HashJoin 实际指「build 阶段完成」 | 改名 `build_done`（HashJoin/HashLeftJoin），Union/UnionAll 保留原名 |
| G | `stop/close` 的 children 参数全部未用（`_left`/`_right`） | 签名裁剪为无 children 参数；子树清理由 `close_tree`/`stop_tree` 统一负责（语义不变） |

## 1. 现状核查（代码事实）

### 1.1 base 参数问题

1. **统一签名仪式**：`dispatch!`（executor.rs:111-132）对 15 种算子 × 5 方法
   统一传 `&mut OperatorBase`。全库 468 处 `base.*` 中：
   - `lifecycle.*` 177 处（38%），其中写入类（mark_opened/stopped/closed）
     全是仪式性（executor 在 dispatch 后已统一 mark）；
   - `output_layout` 120 处——纯只读，且每个算子构造 chunk 都用，
     却每次方法调用都随 `&mut` 传入；
   - `_base` 未用 8 处：`source.stop`、`unary/apply/set/join/graph/
     recursive.reset`、`txn.open`。
2. **`&mut` 过约束**：unary 的 `Filter/Project/Assign` 等 `next()` 完全不碰
   base（unary_operator.rs:193-229），却必须持有独占可变引用；热路径被强加
   不存在的写访问。
3. **状态双栖**：算子状态一半在 self（buffered 游标 `current_index`）、一半
   在 runtime arena（`SourceState::Start.emitted`、哈希表，经
   `base.state_key()` 访问）。`next` 是否需要 base 取决于变体，统一签名
   无法表达——这是 `_base` 遍地的根源。
4. **字段直访**：算子直接 `base.runtime.as_ref()`（unary_operator.rs:159-160）、
   `base.lifecycle.mark_stopped()`（txn_operator.rs:118），绕过封装。

### 1.2 left/right 参数问题

1. **build 侧隐式约定（最实质）**：`build_side_loop`（hash_join.rs:15-42）
   硬编码 `right.advance()` 构建哈希表，`JoinSpec::HashJoin` 只携带
   `hash_keys/probe_keys`（specs.rs:1124-1128），**无 build_side 标记**。
   转换层（conversion.rs:547-558）按逻辑顺序 push 左右子树、不交换。
   后果：build/probe 角色由物理位置隐式编码，三处各自理解（算法函数、
   `JoinCtx`、spec builder）；未来若按基数选择 build 侧（小表建表）而只
   交换子树忘记交换 `hash_keys/probe_keys`，将静默产出错误 join 结果——
   无任何校验。
2. **JoinCtx 混合三类归属**（join_operator.rs:23-29）：`base`（运行时上下文）、
   `left/right`（子树）、`memory_tracker`/`right_col_names`（算子自身变体
   字段，join_operator.rs:455-475 解构后装入）打包为一个结构。self-state
   经两条通道传递（self 字段 + JoinCtx），归属模糊。
3. **命名与语义错位**：HashJoin 的 `left_consumed` 在 `next_hash_join`
   中实际含义是「build 阶段完成」（build 侧是 right！）；UnionAll 的同名
   字段才是「左侧消费完」。同名异义。
4. **相位不对称**：build 相位只用 right、probe 相位只用 left，但签名始终
   同时传两侧——统一签名隐藏真实依赖的另一实例。
5. **生命周期协议绕过**：`Except`（set_operator.rs:296-306）与 `Minus`
   （:295-307）在 `next` 内首次调用时临时 `right.open()` → `advance` →
   `right.close()`；而 Union/Intersect 在 `open()` 统一 open。同一算子的
   子树生命周期行为不一致，且 `next` 内直接调用 children 的 `open/close`
   绕过 executor 的 `close_tree` 协议（资源重复清理风险：executor 的
   `close_tree` 会对同一子树再 close 一次——幂等保护目前掩盖了它）。
6. **stop/close 的 children 参数未用**（`_left`/`_right`）：子树清理由
   `close_tree`（executor.rs:783，后序）/`stop_tree`（:802，根先序）统一
   负责——语义正确，但参数冗余，且 `stop_tree` 已保证资源释放，算子侧
   无需再触达 children。

## 2. 目标设计

### 2.1 上下文拆分与注入

**算子自持两字段**（消除热路径上下文传参）：

```rust
// 所有算子 struct 统一新增（构造时注入，测试可省）
pub struct OperatorRuntimeHandle {
    /// Immutable output contract, injected at construction.
    pub output_layout: Arc<SlotLayout>,
    /// Execution runtime, injected at `open()`. `None` in unit tests.
    pub runtime: Option<Arc<ExecutionRuntime>>,
}
```

- `output_layout`：构造期（`from_spec`/materializer）注入，算子存 self，
  构造 chunk 时 `Arc::clone(&self.output_layout)`——删除 120 处 base 传参。
- `runtime: Option<Arc<ExecutionRuntime>>`：`open()` 时从 executor 克隆注入
  （`ExecutionRuntime` 本身是 `Arc`，内部 arena/锁/取消令牌均经 `Arc`
  访问，无需 `&mut`）。`ensure_not_cancelled`/`spill_manager`/
  `state_arena`/`register_resource`/`profile` 全部经 self 句柄访问。
- 单元测试直接构造 `None`/`default`，与现状 `with_runtime(None)` 等价。

**删除 `OperatorBase` 传参**：原 `base` 中
- 不可变配置（plan_node_id/physical_operator_id/partition_id/is_global/
  chunk_size/output_layout）→ executor 持有的 `OperatorInfo`（仅 executor
  内部与 profile/EXPLAIN 使用，不传算子）；
- `runtime` → 算子 self 句柄（open 注入）；
- `lifecycle` → executor 独占（见 2.2）；
- `correlation_row` → 改经 `inject_correlation_frame` 直接写入
  `SourceOperator::Argument` 的 `frame: Option<(Arc<SlotLayout>, Vec<Value>)>`
  字段（executor 递归遍历时 match 到 `Self::Source(_, SourceOperator::Argument)`
  写入，见 executor.rs:665-673 现状语义不变）；`Argument::next()` 内
  `self.frame.take()` 消费，语义与 `take_correlation_row` 完全一致；
- `reset_used_fallback` → executor 持有（仅 EXPLAIN 读取）。

### 2.2 生命周期收归 executor

- executor 补 `mark_opened` 到 `open()`（executor.rs:678-707，dispatch
  成功后由 executor 写入），`advance/stop/close` 已有 mark（:723/727/
  751/776）不动；
- **删除算子内全部 lifecycle 写入**：`open()` 内 `base.lifecycle.mark_opened()`
  （source_operator.rs:307、unary_operator.rs:187 等）、`stop()` 内
  `mark_stopped`（8 个算子）、`close()` 内 `mark_closed`（close_common 等）、
  `graph_operator.rs:389` 的 `can_close()` 读取改为算子内部自有清理防护；
- 算子只以返回值表达状态：`open()` 返回 `Ok` 即视为已打开。

### 2.3 新签名矩阵

```
open ( &mut self, rt: &OperatorRuntimeHandle, children… )   // 注入 runtime 句柄
next ( &mut self, children… )                                // 零上下文参数
stop ( &mut self )                                           // 不写 lifecycle，无 children
reset( &mut self, children… )                                // 纯自身状态回卷 + 子树 reset
close( &mut self )                                           // 纯自身资源清理（take_state 走 self.runtime）
```

- `_base`/`_left`/`_right` 全部消失；
- `dispatch!` 同步更新为 `op.open(&rt, children)` / `op.next(children)` /
  `op.stop()` / `op.reset(children)` / `op.close()`；
- `StreamingExecutor` 变体结构：`Self::Source(info, rt, op)`（info 为
  不可变 `OperatorInfo`，rt 为 `OperatorRuntimeHandle`），`base()`/`base_mut()`
  访问器与全部 match 站点同步（~40 处，编译器驱动）。

### 2.4 left/right 角色显式化

1. **BuildSide 角色枚举**（`operators/spec.rs` 或 `join_operator.rs`）：

   ```rust
   pub enum BuildSide { Left, Right }
   ```

   `JoinSpec::HashJoin`/`HashLeftJoin` 增加 `build_side: BuildSide` 字段
   （构造默认 `Right`，与现状行为一致）。`next_hash_join` 按角色绑定输入：
   build 侧输入 + `hash_keys` 建表，探测侧输入 + `probe_keys` 探测；
   实现内按 `build_side` 选择 `left/right` 引用，避免复制两套逻辑
   （借用技巧：先取引用，或用小型包装 `struct SideRefs` 统一处理）。
   增加 `debug_assert`：`hash_keys` 列数 = build 侧输出列数、
   `probe_keys` 列数 = 探测侧输出列数（等价键宽校验，防交换子树漏换键）。
2. **删除 JoinCtx**：算法辅助函数签名改为
   `fn next_hash_join(state: &mut HashJoinState, build: &mut StreamingExecutor,
   probe: &mut StreamingExecutor, output_layout: &SlotLayout)`——
   self-state（memory_tracker/right_col_names/哈希表）经 self 字段直传，
   children 显式传参，上下文仅剩 output_layout（或同样经 self 字段，
   则函数无需第四参）。
3. **命名修正**：HashJoin/HashLeftJoin 的 `left_consumed` →
   `build_done`；Union/UnionAll 保留 `left_consumed`。
4. **生命周期规范化**：Union/Intersect/Except/Minus 的 `open()` 统一
   open 双侧子树；Except/Minus 的 right 缓冲仍 lazy（首次 `next` 消费），
   但删除 `right.open()/right.close()`（set_operator.rs:306）——由
   `open()`/`close_tree` 统一管理，消除双 close。
5. **签名裁剪**：`stop()`/`close()` 删除 children 参数（6 处
   `_left`/`_right` 消失）；`Minus` 的右子树从不推进的问题由
   `close_tree` 兜底（现状语义已正确，裁剪后签名如实表达）。

## 3. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| 1 | lifecycle 收归 executor：删全部算子侧 mark_* 写入，executor.open 补 mark_opened | `cargo test -p graphdb-query --lib` 全量（生命周期断言类单测） |
| 2 | 拆分：新增 `OperatorInfo` + `OperatorRuntimeHandle`；算子增 `output_layout`/`runtime` 字段；executor 变体改 4 元组；dispatch! 更新；`correlation_row` 迁移到 `SourceOperator::Argument.frame` | `cargo check -p graphdb-query`（编译器列出全部 match 站点）；子查询 e2e 28 例回归（关联帧注入时序） |
| 3 | 签名裁剪：`stop/close` 去 children；`next` 去上下文；删全部 `_base`/`_left`/`_right` | `cargo clippy -p graphdb-query` 无 unused 告警；lib 全量 |
| 4 | left/right 角色化：`BuildSide` + JoinSpec 字段 + next 按角色绑定 + debug_assert；JoinCtx 解包；`left_consumed`→`build_done` | hash join 单测（左/右各作 build 侧各一遍，断言结果等价）；join 集成测试全量 |
| 5 | Except/Minus 生命周期规范化：open 统一 open 双侧，删 next 内临时 open/close | set 算子单测 + e2e MINUS/EXCEPT 用例 |
| 6 | EXPLAIN `reset:fallback` 与 profile 回归（字段迁移到 executor 侧不变） | EXPLAIN 单测、profile 单测 |
| 7 | 全量回归 | `cargo test -p graphdb-query --lib`、`cargo test --test integration_e2e subquery`、`cargo test --test integration_session_variables`、clippy 全 features |

## 4. 风险与回退

| 风险 | 缓解 |
|------|------|
| executor 变体 4 元组波及 ~40 处 match | 编译器驱动逐一修；base()/base_mut() 访问器先改，算子层改动集中 |
| `Argument.frame` 注入时序（每 run 重新 inject） | 语义与 `take_correlation_row` 完全等价；子查询 e2e 28 例为硬回归 |
| `next` 无上下文后，未来算子新需求（如新运行时能力）无法传参 | `OperatorRuntimeHandle` 保留在 open 注入的 self 字段中，随时可经 self 访问；架构上不再需要 per-call 传参 |
| Except/Minus 删除 next 内 close 后右子树延迟到 close_tree 释放（内存驻留更长） | 语义正确性优先；右子树资源在查询结束统一释放，无泄漏（close_tree 幂等保证） |
| `debug_assert` 键宽校验误报（fallback 命名列 `right_i`） | 校验仅限列数等价（build 侧 = hash_keys 数，探测侧 = probe_keys 数），与列名无关 |
| 回退 | 分步实施，每步独立可回滚：步骤 1/2 为纯机械重构，步骤 4/5 行为等价（build_side 默认 Right） |
