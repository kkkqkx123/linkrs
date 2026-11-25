# `distinct_limit` 有序存在性扫描：与 Ladybug 的对比与瓶颈定位（2026-11-25，devs）

**目标查询**：

```cypher
MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(p:Person)
RETURN DISTINCT p.id ORDER BY p.id LIMIT 10
```

**代码基线**：本地 HEAD `eb736222`，devs 仓库按 `docs/dev/build/devs-sync.md`
同步；后续小改动以增量方式同步到
`/data/kkkqkx/linkrs/repo`。Ladybug 侧为 PyPI wheel `ladybug==0.22.3`，
数据库为 `/data/kkkqkx/linkrs/resource-aligned-parity/lbug-ops-db`。

## 1. 环境与口径

| 项 | 值 |
|---|---|
| 机器 | devs，NUMA node1 CPU `8-15,24-31`，内存绑定 node1 |
| 绑定命令 | `numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31` |
| 线程 | Linkrs / Ladybug 均 16 |
| Buffer | Linkrs / Ladybug 均显式 64 MB |
| 计时 | warmup 1 + run 5，去最大值均值；Linkrs 同时记录中位数 |
| Linkrs 口径 | `exec` 含编译与执行；`execute-stage` 仅执行 |
| Ladybug 口径 | `QueryResult.get_execution_time()`，不含 Python 侧编译/物化 |
| 数据集 | LSQB projected-fk sf0.1，结果基准 `[14]` |

最终验证期间 devs load average 约 35，属于外部负载窗口；同轮四用例结果稳定，
绝对耗时可与同批 JSON 比较，不作跨日小数点级对比。

## 2. Ladybug 实际执行计划

在 Ladybug 上执行 `PROFILE`，查询中的 `Comment_` 按 parity runner 的规则改写为 `Comment`：

```text
SCAN_NODE_TABLE[0]      Person                1700 行
SCAN_REL_TABLE[1]       (p)<-[h]-()           215488 行（反向 CSR）
PROJECTION[2]           p.id
AGGREGATE[3]            group by p.id         1774 组
AGGREGATE_FINALIZE[4]
AGGREGATE_SCAN[5]                             1669 行
PROJECTION[6]
TOP_K[7] / TOP_K_SCAN[8] / PROJECTION[9] / RESULT_COLLECTOR[10]   10 行
```

Ladybug 没有为主键 `ORDER BY ... LIMIT` 选择索引早停，而是扫描全部 Person
与全部反向边，再做 hash distinct 与 TopK。预热执行时间为
10.95 → 5.31 → 3.86 → 3.13 → 3.27 ms，基准记录 3.20 ms。

算子自耗时（预热后 `PROFILE`，ms）：

| 算子 | 自耗时 | 行数 |
|---|---:|---:|
| `SCAN_NODE_TABLE` | 0.063 | 1700 |
| `SCAN_REL_TABLE` | 0.143 | 215488 |
| `AGGREGATE` | 1.649 | 215488 → 1774 |
| `AGGREGATE_FINALIZE` | 2.229 | 1669 |
| `AGGREGATE_SCAN` | 0.061 | 1669 |
| `TOP_K` | 0.145 | 10 |

结论：Ladybug 的优势来自 packed 邻接读取与一次批量 hash 聚合，
主要成本在聚合而非扫描；它没有付出逐边对象物化代价。

## 3. Linkrs 改写后的计划与结果

改写后的物理计划：

```text
OrderedExistenceScan -> Distinct -> TopN -> Limit
```

计划的启用证明写在 CBO notes 中：

```text
ordered_existence:proven,reason=ordered unique primary key 'id' on 'Person'
across 1 contiguous shard(s); edge HASCREATOR_COMMENT from Comment_ to Person;
limit 10,limit=10,index=person_id_idx
```

最终指标（`prod3`，同绑定）：

| 指标 | 值 |
|---|---:|
| `exec`（含编译） | 1.80 ms |
| `execute-stage` | 1.70 ms |
| `compile` | 0.96 ms |
| `ordered_existence_candidates` | 10 |
| `ordered_existence_matches` | 10 |
| `ordered_existence_output_rows` | 10 |
| `ordered_existence_drift` | 0 |
| `spill_rows` | 0 |
| 结果 | `[14]` |

同日差分：关闭改写 25.92 ms，开启改写 1.88 ms，结果逐值一致。
相对 Ladybug 的内部执行 3.20 ms，Linkrs `execute-stage` 为 0.53×；
即使拿包含编译的 `exec`，也低于 Ladybug 的执行计时。

## 4. 瓶颈定位过程

定位手段是在源码中临时加入分阶段 `eprintln`，计时代码在结论后删除，
原始日志保留在 `perf-data/parity/results/2026-11-25/ordered-existence/`。

### 4.1 索引游标：早期 dominant，已修复

首次引入分页游标时，游标打开出现 3.5 秒（首次）与 145 ms（后续运行）
的耗时。拆分后发现 generation 链的 `forward_may_have_range` 每次做完整
`base.range(...)`，而每个 generation 都触发一次 chunk 加载；真正瓶颈是
bloom/range 预检查，不是入边存在性。

修复方式是用 `ChunkedIndex::may_have_range` 的 chunk 描述符做纯元数据区间
重叠判断，不加载 chunk。修复后同一路径游标打开约 0.11 ms。日志证据：
`gate-detail.log` 之前轮次的 `paged: iterators`，以及
`final-detail.log` 中的 `ordered open: 109us`。

### 4.2 存在性门：候选批次过大

最初 `batch_size=64`，一次把 64 个候选交给存在性门。实测这 64 个候选有
10868 个反向邻居，而最终只需前 10 个主键。热态耗时：

```text
gate visit:      ~1.46 ms   10868 个邻居的反向 CSR 门遍历
gate resolve:    ~3.10 ms   每个邻居单独解析 external id
gate visibility: ~2.65 ms   10868 个源点的批量可见性
ordered gate:    ~7.30 ms
execution:       ~8.9 ms
```

此时瓶颈不是索引，也不是 `Distinct/TopN`，而是“为了早停一次读了过多候选”。
其中逐个邻居调用 `with_vertex_tables` 解析 external id 占了一半以上。

证据：`gate-detail.log`。

### 4.3 缩小批次

把批次改为 `min(limit, 16)` 后：候选 16 个、邻居 1747 个，
存在性门约 1.0 ms，执行阶段约 2.23 ms。

```text
gate visit:      ~0.15 ms
gate resolve:    ~0.44 ms
gate visibility: ~0.38 ms
```

证据：`batch16-detail.log`、`source-detail.log`。

### 4.4 批次贴合 limit + 批量 external id

最终两处修改：

- `crates/linkrs-query/src/executor/streaming/plan/arena_builder/partition/chain.rs`
  使用 `batch_size: proof.limit.clamp(1, 16)`，本查询即 10。
- `crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs`
  改用 `external_ids_by_internal_batch`，在同一 vertex table 锁内解析整批
  internal id，替代逐邻居获取锁。

热态分阶段（`final-detail.log`）：

```text
ordered open:        109 us
ordered cursor:       31 us
gate visit:          127 us   （1243 个邻居）
gate resolve:         73 us   （批量 external id）
gate visibility:     310 us   （1243 个源点可见性）
ordered gate call:   524 us
ordered next:        573 us   （cursor + gate）
execute-stage:      1700 us
compile:             960 us
```

存在性门从约 7.3 ms 降到约 0.52 ms；源算子自身约 0.57 ms，执行阶段剩余
约 1.13 ms 位于源算子之外。

## 5. 与 Ladybug 的剩余差异

1. **算法方向不同**：Ladybug 全量反向扫描 + hash distinct；Linkrs 按主键有序
   做存在性早停。Ladybug 单次邻接读取更快，但处理 215488 条边；Linkrs 的
   存储原语更重，但只处理 10 个候选和 1243 个邻居。
2. **访问形态不同**：Ladybug 是 packed child slice；Linkrs 需要 MVCC 快照、
   pending gate、源点可见性与 external id 解析。当前这些成本已被限制在
   1243 个邻居内，不再是主导项。
3. **计时口径不同**：Ladybug 上报不含编译的内部执行时间；Linkrs 的 `exec`
   含约 0.96 ms 编译。比较执行阶段时 Linkrs 为 1.70 ms，已经低于 Ladybug。
4. **剩余开销位置不同**：Ladybug 主要在聚合；Linkrs 主要在源算子之外的
   执行器装配/结果收集与编译。
5. **正确性机制不同**：Linkrs 的改写受严格形状证明、快照与 generation 守卫，
   任一条件失效即回退原路径；Ladybug 没有专用早停路径。

## 6. 观测异常与环境注记

- EXPLAIN 中 `ordered_existence_open_us` 多轮显示 7.5–13.1 秒，但同点直接
  计时只有约 0.11 ms，且查询总耗时 1.8 ms。该字段未用于本报告的瓶颈归因，
  需单独排查 profiling 路径的计时来源。
- 最终全门运行期间，数据库重建曾两次因后台 checkpoint/snapshot 抢占导致
  COPY 失败；重试成功后结果稳定。这是高负载下的导入竞争，不属于查询改写路径。
- Ladybug 的 `PROFILE` 单次算子计时含采样开销，绝对和与 `get_execution_time()`
  不完全相等；计划形状和相对主导项可靠。

## 7. 验证

| 门 | 结果 |
|---|---|
| `linkrs-storage --lib` | 1417 passed |
| 入边存在性单测 | 1 passed |
| `linkrs-query --lib ordered_existence` | 2 passed |
| partitioning | 49 passed |
| local_topk | 13 passed |
| `parallel_partition_execution` | 27 passed |
| `dql` | 324 passed |

差分关闭/开启均返回 `[14]`，`result_stable=true`、`spill_rows=0`。
生产默认 `group_by_count [14|59]`、`join_two_hop [382018]`、
`variable_path_count [400153]`，无结果退化。

## 8. 证据与复现

原始 JSON、EXPLAIN 和日志：

`perf-data/parity/results/2026-11-25/ordered-existence/`

关键文件：

- `ladybug/profile.txt`：Ladybug `PROFILE` 与预热时间
- `gate-detail.log`：64 候选批次的存在性门拆分
- `batch16-detail.log`：16 候选批次对照
- `final-detail.log`：最终 10 候选批次拆分
- `diff2-off.log` / `diff2-on.log`：同库 A/B
- `prod3/`、`explain-prod3/`：生产默认逐用例 JSON 与 EXPLAIN
- `gate-final2.log`、`copy-retry2.log`：全门测试与成功的数据库重建

复现要点：使用 `/data/kkkqkx/linkrs/parity/lsqb-sf0.1-v5/db`，
通过 `LINKRS_ORDERED_EXISTENCE_SCAN=off|on` 切换路径，所有 Linkrs 运行
均加 node1 CPU/内存绑定。Ladybug 需先
`source /data/kkkqkx/graphdb-ben/bench-env.sh`，再用同一 64 MB buffer
与 16 线程连接打开上述数据库执行 `PROFILE`。
