# Issue：简单 COUNT 查询显著慢于 Ladybug（LSQB-sf0.1 对比）

**日期**: 2026-11-12
**发现来源**: devs 三引擎性能对比（`docs/archive/perf/parity/2026-11-12-lsqb-sf0.1.md`）
**状态**: 已定位根因，待立项修复

## 1. 现象

LSQB-sf0.1 三方 COUNT 对齐测试中，Linkrs 对纯计数类查询比 Ladybug wheel 慢
40×–9000×（neug 为单线程 wall-clock，仅作参照）：

| query | linkrs exec ms | lbug exec ms | 慢倍数 |
|-------|---------------:|-------------:|-------:|
| q2 `MATCH (n:Comment_) RETURN count(n)`（21.5 万顶点） | 97.9 | 1.71 | **57×** |
| q4 `MATCH (:Person)-[k:KNOWS]->(:Person) RETURN count(k)`（1.8 万边） | 52.1 | 0.046 | **>1000×** |
| q5 `MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(:Person) RETURN count(h)`（21.5 万边） | 1336 | 0.045 | **>1000×** |
| q7 `MATCH (:Comment_)-[r:REPLYOF_POST]->(:Post) RETURN count(r)`（10.7 万边） | 1059 | 0.299 | **3549×** |
| q9 `MATCH (:Forum)-[c:CONTAINEROF]->(:Post) RETURN count(c)`（17.2 万边） | 540.6 | 0.060 | **>9000×** |

结果数值全部正确（COUNT 语义无误），纯粹是执行效率问题。

## 2. 复现与取证方法

- 机器：devs（`ssh devs`，Ubuntu 18.04，2×E5-2667 v3，绑核 `taskset -c 16-31 numactl --membind=1`）
- 数据：`/tmp/linkrs-parity/lsqb-sf0.1/db`（`linkrs-parity-bench copy` 导入的 projected-fk 数据）
- 取证工具：`crates/linkrs-api/examples/explain_probe.rs`（embedded 会话，先 `EXPLAIN` 后实测计时）
- 汇总测试：`linkrs-parity-bench run --warmup 1 --run 3 --threads 8`（三引擎对比报告）

## 3. EXPLAIN 证据

边 COUNT 查询的实际计划形态（`EXPLAIN MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(:Person) RETURN count(h)`）：

```
0 StorageScanVertices  est_rows:1, reason:storage_full_scan, mode:identity   <- 源顶点全扫
1 Filter               est_rows:0.5
2 ExpandAll            est_rows:0.5, mode:columnar, rows:skipped             <- 逐边展开
3 Project              est_rows:0.5
4 Aggregate            est_rows:0.05, blocking:true (HashAggregate)
```

顶点 COUNT（q2）同样走 `StorageScanVertices → Filter → Project → Aggregate(HashAggregate)`。

三个关键观察：

1. **执行模型单线程**：`Parallel Workers: requested=1, actual=1`，
   `fallback_reason="partitioning is disabled"`——全部数据单线程流过。
2. **基数估计严重失真**：源扫描 `est_rows:1`（实际 215,488），聚合输出
   `est_rows:0.05`。CBO 的 Hash/SortAggregate 选择、并行度决策都建立在
   `est_rows=1` 上，不可能选对。
3. **无 count 专用计划**：即使 RETURN 只有 `count(h)` 一个聚合，计划仍是
   "顶点全扫 → 逐行 Expand → 逐行聚合" 的通用流水线，没有
   "从 CSR 直接数边" 或 "从顶点表元数据直接数顶点" 的捷径。

## 4. 根因分析（按代码路径）

### 4.1 主因：边 COUNT 经 ExpandAll 逐行展开，未走 ScanEdges

计划器把 `(:Comment)-[h]->(:Person)` 编译为从源顶点扫描出发的
`ExpandAll`（`executor/streaming/operators/graph_operator/expand.rs`），
而不是独立的 `ScanEdges` 边扫描源。展开路径
（`expand_columnar.rs::read_destinations`）对每个批次的每个目标顶点调用
`reader.get_vertices_batch` / `get_vertices_projected_batch` 做端点存在性
校验——即使查询只数边、不读任何端点属性，也要把目标顶点批量解码一遍。

Ladybug 的同语义查询直接在 CSR 上数边条目（`MATCH` 绑定与否不影响
count 快捷路径），亚毫秒返回。这就是 q4–q9 千倍差距的主要来源。

边侧已有的 identity 注解规则（`optimizer/heuristic/scan_edge_identity.rs`）
只在计划含 `ScanEdges` 节点时生效（`annotate_mut` 只匹配
`PlanNodeEnum::ScanEdges`）；当前计划形态根本不生成 `ScanEdges`，
所以该优化链路（identity_only → topology_only 免解码）对这类查询完全
不触发。

### 4.2 次因：顶点 COUNT 虽有免物化路径，但聚合循环仍是逐行标量

顶点扫描有 topology-only 快路径（`source_operator/storage_scan.rs`：
identity_only + 空投影 + 无谓词 → 免属性解码），聚合算子也有
count 快路径（`aggregate_operator.rs::next_aggregate`：
`count_identity_arg` 免参数求值，`rowless_units` 免行解引用）。
q2 的 97.9ms / 215,488 行 ≈ **455ns/行**，全部花在
`next_aggregate` 的累加循环：

- 每行构造 `group_key: Vec<Value>` + `arg_values: Vec<Value>`（分配 + clone）；
- 每行一次 `HashMap<Vec<Value>, ...>` 哈希探测；
- 每行 `Value::Int(1)` 单元值逐个 `accumulate`。

列式批量进来后立刻退化为逐行 `Vec<Value>` 操作，没有"按 chunk 直接
累加行数"的向量化短路（无 GROUP BY 的全局 count 完全可以
`sum(chunk.len())` 一步完成）。

### 4.3 放大器：单线程 + 分区被禁用

`Parallel Workers: requested=1` 与 `partitioning is disabled` 说明
执行层并行/分区通道未启用。即使 4.1/4.2 修复前，对百万级行数的
全扫流水线这也是可观的放大系数（devs 三方对比中 lbug 也按 8 线程
连接执行）。

### 4.4 放大器：CBO 基数估计失真

`est_rows:1` 对 21.5 万行的全扫，说明统计信息（`Statistics:
sample_limit=10000` 路径）没有为顶点/边表提供基础行数，或
`reason:storage_full_scan` 路径未回填基数。这使得所有 cost-based
决策（聚合策略、连接顺序、并行度）默认失效。

### 4.5 单行成本对比汇总

| 查询形态 | 行数 | 总耗时 | 每行成本 | 主要构成 |
|----------|-----:|-------:|---------:|----------|
| 顶点 count（identity 扫描） | 215,488 | 97.9ms | ~455ns | 聚合循环逐行开销 |
| 边 count（Person 源，1.8 万边） | 18,135 | 58.3ms | ~3.2µs | Expand + 端点解码 |
| 边 count（Comment 源，21.5 万边） | 215,488 | 1378ms | ~6.4µs | Expand + 端点解码 |
| lbug 边 count（CSR 直数） | — | 0.046ms | — | 无逐行工作 |

边 count 每行 ~6µs 中，大头是 Expand 的端点批量解码 + 每 row 的
header/typed column 组装；顶点 count 每行 ~0.45µs 全部是聚合循环标量开销。

## 5. 优化方向（建议优先级）

1. **count 专用计划（收益最大）**：当聚合只有无 GROUP BY 的
   `count(*)/count(var)` 且 var 绑定于单一边类型时，改写为
   `ScanEdges`（identity_only + topology_only）直连 Aggregate，
   或更进一步直接从 storage 元数据（CSR 边计数、顶点表行数）取数——
   这是 lbug 0.046ms 的来源。顶点侧同理：`MATCH (n:T) RETURN count(n)`
   可直接读顶点表元数据计数。
2. **Expand 端点校验免解码**：当目标顶点仅用于模式匹配（无属性/无
   输出引用）时，用顶点 id 存在性位图/半连接掩码替代
   `get_vertices_batch` 全量解码。
3. **聚合向量化**：无 GROUP BY 的全局 count 在 chunk 层直接累加
   `visible` 行数，跳过 per-row Vec/HashMap 路径。
4. **基数估计修复**：顶点/边表扫描节点从 storage 元数据回填真实行数，
   让 CBO 决策（含并行度）有正确输入。
5. **启用并行/分区**：大行数全扫 + 全局聚合的并行通道（当前被禁用）。

## 6. 复现命令（devs）

```bash
# 导入数据（一次性）
cd /data/kkkqkx/linkrs/repo
./target/release/linkrs-parity-bench copy \
  --dataset lsqb-sf0.1 \
  --csv-dir /data/kkkqkx/graphdb-ben/data/social-network-sf0.1-projected-fk \
  --script benches/parity/schema/lsqb.cypher --db /tmp/linkrs-parity/lsqb-sf0.1/db

# 取证（EXPLAIN + 计时）
taskset -c 16-31 numactl --membind=1 \
  ./target/release/examples/explain_probe /tmp/linkrs-parity/lsqb-sf0.1/db lsqb

# 编译探针（注意：走 linkrs-api crate，根 crate 会拖入 devs 缓存中没有的
# aws-lc-rs；CARGO_NET_OFFLINE=true）
cargo build --release -p linkrs-api --features embedded --example explain_probe
```

## 7. 相关文件

- `crates/linkrs-query/src/executor/streaming/operators/graph_operator/expand_columnar.rs`（read_destinations）
- `crates/linkrs-query/src/executor/streaming/operators/blocking/aggregate_operator.rs`（next_aggregate）
- `crates/linkrs-query/src/executor/streaming/operators/source_operator/storage_scan.rs`（topology-only 快路径）
- `crates/linkrs-query/src/optimizer/heuristic/scan_edge_identity.rs`（ScanEdges 注解规则，当前计划形态不触发）
- `crates/linkrs-api/examples/explain_probe.rs`（取证工具）
- 三方基线数据：`docs/archive/perf/parity/results/2026-11-12/`
