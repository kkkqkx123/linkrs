# Streaming 模块冗余分析

对 `crates/graphdb-query/src/query/executor/streaming/` 模块进行冗余扫描的结果。

## 高优先级

| # | 问题 | 位置 | 改动量 |
|---|---|---|---|
| **A** | `contextual_to_expression` 私有函数在 4 个文件中重复定义 | `relational.rs:295` `control.rs:10` `graph.rs:12` `writes.rs:26` | 提取到 `operator_plan_builder/mod.rs` 作为 `pub(super)` |
| **C** | 7 个 Join 变体的构建代码（8 行/个）仅 `JoinSpec` 构造不同 | `operator_plan_builder/mod.rs:116-275` | 提炼 `build_join_core` / `build_join_with_keys` 辅助函数 |
| **F** | `open/advance/stop/close` 各 15 个分支的匹配分发共 60 个几乎相同的调用臂 | `executor.rs:531-654` | `lifecycle_dispatch!` 宏将每个方法缩减为~3 行 |
| **G** | `set_chunk_size`/`set_runtime` 在 `materialize` 的 14 个分支中重复——它们是递归的，只需在根节点调用一次 | `physical_node.rs:152-349` | 每个分支删除最后 3 行，在 match 后统一调用 |

## 中优先级

| # | 问题 | 位置 | 备注 |
|---|---|---|---|
| **B** | `join_keys_to_expr` 闭包与 A 的逻辑重复 | `physical_builder.rs:325-333` | 修复 A 后替换为 `contextual_to_expression` 调用 |
| **D** | `Source(0, Start, single_streaming())` 叶子节点出现 19 次 | `ddl.rs` `txn.rs` `fulltext.rs` `vector.rs` `writes.rs` | 提取辅助函数 |
| **E** | DDL/Txn/FT/Vector 构建器结构重复 | 同上 4 个文件 | 可在 D 的基础上进一步提炼 |
| **K** | `ExecutionRuntime::new(...)` 构建出现 2 次 | `factory.rs:55-68, 98-111` | 相同的带 feature 门控的配置代码 |

## 低优先级

| # | 问题 | 位置 |
|---|---|---|
| **I** | Gather 节点 ID 分配重复 4 次 | `physical_builder.rs` |
| **J** | "not partition-local" 检查重复 4 次 | `physical_builder.rs` |
| **M** | `record_profile_rows` 在 `operator_base.rs` 和 `executor.rs` 各有一份 | `operator_base.rs:137` `executor.rs:410` |
| **N** | `compare_values_for_minmax` 是 `compare_values` 的子集 | `helpers/accumulator_states.rs:164` |
| **O** | `new_with_layout` 是 `new` 的无用别名（60+调用点） | `context.rs:41` |
| **P** | "Internal routing error" 在 9 个构建器中重复 | 9 个 builder 文件 |
| **Q** | `base.rs` 为空 | `base.rs` |
| **R** | `scans.rs` 中未使用的 `context` 参数被 `#[allow]` 隐藏 | `scans.rs:11` |
| **S** | `configure_parallel_partitions` 是废弃的死方法 | `executor.rs:121` |
| **V** | `SetSpec::Minus` 从未被使用 | `operator_spec.rs:372` |

## 修复状态

- [x] **A**: `contextual_to_expression` 提取到 `mod.rs` 为 `pub(super)`，4 个副本删除
- [x] **B**: `join_keys_to_expr` 闭包替换为 `contextual_to_expression` 调用
- [x] **C**: `build_join_core` / `build_join_with_keys` 辅助函数，7 个 Join 分支简化
- [x] **F**: `lifecycle_dispatch!` 宏，4 个生命周期方法各缩减为~3 行
- [x] **G**: `set_chunk_size`/`set_runtime` 在 `materialize` 的 match 后统一调用一次
- [x] **D**: 提取 `single_start_source()` 辅助函数，20处 `Source(0, Start, single_streaming())` 替换
- [x] **E**: `build_leaf_command` 辅助函数，消除 DDL/Txn/FT/Vector 构建器结构重复
- [x] **K**: `runtime_from_context` 辅助函数，消除 `ExecutionRuntime::new()` 重复构建
- [x] **I**: `allocate_gather_node_id` 辅助函数，消除 4 次重复的 `checked_add` 分配模式
- [x] **J**: `require_partition_local` 辅助函数，消除 4 次重复的 `is_partition_local` 检查
- [x] **M**: `executor.rs` 中 `record_profile_rows` 委托给 `operator_base.rs` 实现
- [x] **N**: `compare_values_for_minmax` 替换为 `comparison.rs` 的 `compare_values`
- [x] **O**: 删除 `new_with_layout`，21 处调用点替换为 `new`
- [x] **P**: `internal_routing_error` 辅助函数，统一 7 个构建器的错误消息
- [x] **Q**: 删除空的 `base.rs`
- [x] **R**: 移除 `scans.rs` 的 `#[allow(unused_variables)]`
- [x] **S**: 删除废弃的 `configure_parallel_partitions` 方法
- [ ] V: `SetSpec::Minus` 已在 `set_operator.rs` 中使用（文档已过时）
