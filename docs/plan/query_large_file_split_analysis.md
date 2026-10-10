# linkrs-query 大文件职责分析与拆分建议

> 统计口径:`wc -l`,仅统计 `crates/linkrs-query` 下超过 800 行的 Rust 源文件(`tests/`、`benches/`、`#[cfg(test)]` 单元测试文件不计入拆分对象)。

## 一、统计结果

### 源码文件(拆分对象)

| 行数 | 文件 | 主要内容 |
|---:|---|---|
| 1985 | `src/executor/streaming/operators/graph_operator/common.rs` | Expand 算子全部执行逻辑 + 输出缓冲 + 单元测试 |
| 1408 | `src/executor/streaming/plan/arena_builder/partition.rs` | 分区执行计划构建 + 单元测试 |
| 1397 | `src/optimizer/heuristic/expand_pushdown.rs` | Expand 下推规则(标注 + 应用)+ 单元测试 |
| 1163 | `src/planning/plan/core/nodes/traversal/traversal_node.rs` | 5 个遍历类 PlanNode 定义 |
| 1158 | `src/executor/streaming/operators/ddl_operator/schema_executor.rs` | space/tag/edge/index 四类 DDL 执行 |
| 1125 | `src/executor/streaming/spill.rs` | 落盘(run 读写、哈希分区、配额、管理器) |
| 1121 | `src/executor/streaming/operators/join_operator/hash_join.rs` | HashJoin 算子 + build side + 单元测试 |
| 1110 | `src/planning/plan/core/nodes/base/plan_node_enum.rs` | PlanNode 枚举(多为生成式代码) |
| 1087 | `src/planning/join_order/plan_join_order.rs` | Join 顺序枚举器 + 单元测试 |
| 1084 | `src/executor/streaming/plan/arena_builder/metadata.rs` | Arena 元数据构建 |
| 1071 | `src/planning/plan/logical/conversion.rs` | 逻辑计划转换,单个 `convert_plan` 约 970 行 |
| 1061 | `src/executor/streaming/operators/source_operator.rs` | Source 算子 |
| 1054 | `src/optimizer/cost_based/join_order_rewriter/reorder.rs` | Join 顺序重写 |
| 1049 | `src/planning/statements/clauses/return_clause_planner.rs` | RETURN 子句规划 |
| 1046 | `src/executor/expression/functions/builtin/graph.rs` | graph 内置函数 |
| 1033 | `src/optimizer/heuristic/batch.rs` | 启发式规则批处理 |
| 1022 | `src/parser/parsing/parse_context.rs` | 解析上下文 |
| 1021 | `src/executor/streaming/plan/arena_builder/specs/ddl.rs` | DDL 算子 arena spec |
| 1017 | `src/executor/streaming/operators/blocking/aggregate_operator.rs` | 聚合算子 |
| 995 | `src/planning/vector_planner.rs` | 向量查询规划 |
| 987 | `src/planning/plan/explain/describe_visitor.rs` | DESCRIBE 访问器 |
| 985 | `src/binder/bound.rs` | Bound 结构定义 |
| 957 | `src/executor/expression/evaluator/compiled.rs` | 编译期表达式求值 |
| 954 | `src/executor/streaming/helpers/accumulator_states.rs` | 聚合累加器状态 |
| 950 | `src/optimizer/analysis/required_properties.rs` | 必需属性分析 |
| 949 | `src/optimizer/cost_based/join_order.rs` | CBO join order 入口 |
| 946 | `src/optimizer/cost/calculator.rs` | 代价计算 |
| 946 | `src/executor/streaming/operators/copy.rs` | COPY 算子 |
| 942 | `src/planning/fulltext_planner.rs` | 全文检索规划 |
| 918 | `src/executor/streaming/chunk/columnar_batch/operations.rs` | 列式批操作 |
| 916 | `src/optimizer/factorization/factorization_rewriter.rs` | 因子化重写 |
| 913 | `src/optimizer/cost_based/subquery_unnesting.rs` | 子查询去嵌套 |
| 912 | `src/executor/streaming/operators/apply_operator.rs` | Apply 算子 |
| 903 | `src/executor/streaming/operators/recursive_fragment_operator.rs` | 递归片段算子 |
| 897 | `src/optimizer/cost_based/aggregate_strategy.rs` | 聚合策略 |
| 891 | `src/executor/streaming/operators/ddl_operator/maintenance_executor.rs` | 维护类 DDL 执行 |
| 890 | `src/executor/expression/functions/builtin/container.rs` | 容器内置函数 |
| 883 | `src/executor/streaming/operators/vector_operator.rs` | 向量算子 |
| 876 | `src/executor/expression/evaluator/expression_evaluator.rs` | 表达式求值器 |
| 870 | `src/pipeline/compiler.rs` | Pipeline 编译器 |
| 856 | `src/executor/streaming/subquery.rs` | 子查询执行 |
| 855 | `src/planning/statements/dml/create_planner.rs` | CREATE 规划 |
| 854 | `src/executor/streaming/chunk/collector.rs` | 结果收集器 |
| 851 | `src/optimizer/stats/collector.rs` | 统计收集器 |
| 843 | `src/executor/streaming/operators/exchange_operator.rs` | Exchange 算子 |
| 838 | `src/optimizer/heuristic/scan_identity.rs` | Scan 恒等规则 |
| 831 | `src/executor/streaming/plan/materializer.rs` | 计划物化器 |
| 818 | `src/planning/statements/dql/group_by_planner.rs` | GROUP BY 规划 |
| 817 | `src/executor/expression/functions/builtin/utility.rs` | 工具内置函数 |
| 816 | `src/executor/streaming/operators/unary_operator.rs` | 一元算子基类 |
| 814 | `src/cache/cte_cache.rs` | CTE 缓存 |
| 813 | `src/executor/streaming/operators/blocking/materialize_operator.rs` | 物化算子 |
| 810 | `src/optimizer/cost/selectivity.rs` | 选择度估算 |
| 809 | `src/executor/streaming/operators/blocking.rs` | 阻塞算子基类 |
| 809 | `src/executor/expression/functions/builtin/aggregate.rs` | 聚合内置函数 |
| 806 | `src/executor/explain/format.rs` | EXPLAIN 格式化 |

### 不拆分的文件

- `tests/integration_management.rs`(1942)、`tests/integration_functions.rs`(1293)、`tests/integration_core.rs`(1089)、`tests/parallel_partition_execution.rs`(959)、`tests/integration_management_extended.rs`(832):集成测试,按场景分文件即可,不属职责拆分范畴。
- `benches/executor_bench.rs`(1087):基准测试。
- 纯测试模块文件:`parser/parsing/stmt_parser/tests.rs`(1602)、`executor/streaming/chunk/tests.rs`(1561)、`engine/tests.rs`(1154)、`optimizer/engine/tests.rs`(1115)、`parser/parsing/tests.rs`(1058)、`factorization_compute/tests.rs`(1040)、`executor/tests.rs`(1025)、`ast/stmt/tests.rs`(881)、`exists_planner/tests.rs`(952):已经是"大文件拆测试"的组织形态,无需再拆。

## 二、重点文件分析(职责过多,建议优先拆分)

### 1. `graph_operator/common.rs`(1985 行)—— 最优先

"common" 文件名本身就是坏味道:它聚集了 Expand 算子的所有执行路径,至少四种异构职责:

- 输出缓冲与行可见性:`ExpandOutputBuffer`、`VisibleRows`(L31-104)
- 种子解析与过滤:`parse_seeds`、`seed_vid`、`row_passes_filter`、`seed_slot`、`lightweight_seed_row`(L99-179)
- 行式执行:`expand_single_step`(L187-372,约 185 行)
- 列式执行:`expand_single_step_columnar`(L712-1005,约 290 行)
- 多跳/变长路径:`expand_multi_hop_frontier`、`expand_variable_frontier`、`expand_count_only`、`expand_variable_row_union`(L1014-1617,每段 250-300 行)
- chunk 分发入口:`expand_on_chunk`、`traverse_on_chunk_with_semantic`(L1619-1901)
- 闭环/旁路辅助:`is_closed_loop_storage`、`closed_loop_dst_tag`、`hop_bypass` 系列(L379-518)
- 文件尾部还内嵌 5 个单元测试(L1916-1984)

**建议拆分**(同目录模块化):

```
graph_operator/
  expand_buffer.rs      # ExpandOutputBuffer、VisibleRows
  expand_seeds.rs       # parse_seeds、seed 解析、row 过滤
  expand_row_path.rs    # expand_single_step、行式辅助
  expand_columnar.rs    # expand_single_step_columnar、PendingEdge
  expand_frontier.rs    # multi_hop / variable / count_only
  expand_dispatch.rs    # expand_on_chunk、traverse_on_chunk_with_semantic、闭环/bypass 辅助
```

### 2. `traversal/traversal_node.rs`(1163 行)

一个文件定义 5 个独立 PlanNode:`ExpandNode`、`ExpandAllNode`(约 500 行,最大)、`TraverseNode`、`AppendVerticesNode`、`BiExpandNode`/`BiTraverseNode`。这些节点互相无依赖,仅因"都是遍历节点"而同文件。

**建议**:按节点拆为同目录子模块 `expand.rs`、`expand_all.rs`、`traverse.rs`、`append_vertices.rs`、`bi_traverse.rs`,`traversal_node.rs` 保留 `mod` 声明与 re-export。多数 getter/setter 属于字段访问器,可借 `impl` 分块减少样板,但本次不动 API,仅拆文件。

### 3. `schema_executor.rs`(1158 行)

四个大函数各自 200-330 行,对应四类 DDL:`execute_space_manage`(300 行)、`execute_tag_manage`、`execute_edge_manage`、`execute_index_manage`(330 行)+ `execute_delete_index`。

**建议**:按 DDL 种类拆为 `schema_executor/{space.rs, tag.rs, edge.rs, index.rs}`,共享的 `endpoint_rows`、`parse_vid_type_str` 放 `mod.rs` 或 `common.rs`。与 `maintenance_executor.rs`(891 行)同属 ddl_operator,可一并审视。

### 4. `spill.rs`(1125 行)

一个文件塞下 5 个正交子系统:run 文件格式(`RunHeader`/`encode_section`/FNV 哈希)、`RunWriter`/`RunReader`、`DiskQuota`、哈希分区(`hash_*_partition`、`HashPartitionSpiller`)、`SpillManager`。

**建议**:拆为 `spill/{mod.rs, config.rs, run_format.rs, run_io.rs, quota.rs, hash_partition.rs, manager.rs}`。各部分耦合仅通过 `SpillConfig`/`SpillManager`,拆分成本低。

### 5. `logical/conversion.rs`(1071 行)

单一 `convert_plan` 函数约 970 行(L47-1017),是典型的巨型 match 分发函数。

**建议**:按节点类别拆为 `conversion/{mod.rs, scan.rs, traversal.rs, aggregate.rs, join.rs, setop.rs, ddl.rs}` 等,`convert_plan` 保留顶层 match,各分支函数分文件。与 `partition.rs` 的 arena 构建风格类似,风险主要在可见性调整。

### 6. `partition.rs`(1408 行)与 `expand_pushdown.rs`(1397 行)

两者结构相同:实现逻辑 + 尾部大量单元测试混在一个文件。

- `partition.rs`:`build_partitioned*` 系列(L74-563)+ join 构建(L574-820)+ `push_global_op`/`decompose`/`collect_chain`/`split_aggregate`(L823-1093)+ 测试(L1110-1407,约 300 行)。
- `expand_pushdown.rs`:标注规则(L50-370)+ needs 分析(`expand_prop_needs`、`collect_expr_needs`,L376-652)+ skip_rows 决策(L672-785)+ `apply_decisions`(L788-913)+ 测试(L994-1396,约 400 行)。

**建议**:仅按项目既有约定把测试移到同目录 `tests.rs`(`partition.rs` 已有 `IndependentBranchOp` 等可测边界),实现部分可再按"标注/决策/应用"分文件。`expand_pushdown.rs` 处于当前未提交改动中,拆分应等当前工作落地后进行。

### 7. `hash_join.rs`(1121 行)

三种职责:分区模式执行(`enter_partitioned_mode`、`next_partitioned`)、普通/左连接执行(`next_hash_join`、`next_hash_left_join`)、`HashJoinBuildSide` 数据结构 + 键求值,尾部 8 个单元测试。

**建议**:测试移至 `hash_join/tests.rs`;`HashJoinBuildSide` + `JoinKeyValue` 拆到 `build_side.rs`;分区逻辑拆到 `partitioned.rs`。

### 8. `plan_join_order.rs`(1087 行)

`JoinOrderEnumerator` 的 impl 占 730 行(L113-846),混合了:级别枚举、基表扫描规划、WFO/哈希连接规划、schema 计算、hint 处理、`solve_tree_node`(L621-845,约 220 行 DP 核心)。尾部测试约 240 行。

**建议**:测试移出;`solve_tree_node` 及 DP 辅助拆到 `dp.rs`,hint 与编码(`encode_plan`、`mix_overflow`)拆到 `encoding.rs`。

## 三、中低优先级(800-1000 行,模式相同)

这批文件行数刚过阈值,多为"单一职责 + 大实现函数"或"算子 + 测试",可延后处理:

- **算子类**(模式:一个算子一个文件,尚属合理):`aggregate_operator.rs`、`source_operator.rs`、`apply_operator.rs`、`copy.rs`、`exchange_operator.rs`、`recursive_fragment_operator.rs`、`vector_operator.rs`、`materialize_operator.rs`、`unary_operator.rs`、`blocking.rs`。仅当继续增长时再拆。
- **规划器类**:`return_clause_planner.rs`、`group_by_planner.rs`、`create_planner.rs`、`vector_planner.rs`、`fulltext_planner.rs`、`subquery_unnesting.rs`——每个对应一个语句/子句,职责单一,暂不拆。
- **内置函数**:`graph.rs`、`container.rs`、`utility.rs`、`aggregate.rs`——按命名空间分文件本就是拆分结果,行数超标但内聚,不拆。
- **arena_builder**:`metadata.rs`(1084)、`specs/ddl.rs`(1021)——与 `partition.rs` 同目录,如拆 partition 可顺带审视。
- **optimizer 其余**:`batch.rs`、`scan_identity.rs`、`required_properties.rs`、`calculator.rs`、`selectivity.rs`、`aggregate_strategy.rs`、`factorization_rewriter.rs`、`stats/collector.rs`——单一规则/分析,暂不拆。

## 四、执行顺序建议

1. `graph_operator/common.rs` —— 超阈值最严重,职责混杂最多,收益最大。
2. `spill.rs` —— 职责正交、拆分风险最低。
3. `schema_executor.rs` —— 函数边界即模块边界,机械拆分。
4. `traversal_node.rs` —— 按节点拆文件,无逻辑改动。
5. `hash_join.rs`、`plan_join_order.rs`、`partition.rs` —— 先移测试,再按需拆实现。
6. `conversion.rs` —— 巨型函数拆分,需回归验证 EXPLAIN/执行结果一致性。
7. `expand_pushdown.rs` —— 已有未提交改动,待其落地后再拆。

拆分原则:纯移动代码与可见性调整,不改行为;每步保持 `cargo test --lib` 与相关集成测试通过;子模块间通过 `pub(crate)`/`pub(super)` 最小化暴露面。
