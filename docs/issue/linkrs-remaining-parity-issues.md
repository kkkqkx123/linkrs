# Linkrs 剩余对齐问题清单

**日期**：2026-11-18，最新资源对齐重测 2026-11-25  
**状态**：本文件是 Linkrs 与 Neug / Ladybug 对齐剩余任务的单一真相源。历史方案与旧问题清单已删除，不再作为判断依据。分阶段修改方案见 `docs/plan/linkrs-remaining-parity-implementation-plan.md`，该方案不重新定义问题。当前状态以第 1、35、49 节为准；2026-11-25 之前基于跨节点绑定或未对齐 buffer 的三方倍率一律作废，只作为历史过程记录。阶段一、二的实施见 `docs/plan/stage1-stage2-current-gap-remediation.md`。

## 0. 范围与判定口径

本清单只记录尚未闭环的事项。已闭环事项仅在背景中列出，不再拆分任务。每项都绑定完成定义、验证方式和当前测试锚点；没有测试或基准证据的结论不算完成。

已闭环的背景事项：

- COPY 导入：单会话、组提交、批量会话、终态检查点跳过和分段计时均已落地；sf0.1 导入总耗时由约 163 秒降到 11.2–11.3 秒，最慢单边约 2.1 秒。表间并行导入与跨批量事务属于明确不做项。
- 纯计数：九条 LSQB 三方行数一致，中位 0.14–0.61 毫秒，编译 10–20 微秒；秒级瓶颈已消除。
- 编译/执行元数据：管线、API、embedded 结果和 parity runner 已接通双通道计时。EXPLAIN 文本中的独立阶段展示仍未完成，归入可观测性问题。
- embedded 纯读绑定：自动提交单语句复用管线分类器，纯读走只读绑定。
- 存活期计数：游标扫描 TTL 过滤与查询侧 TTL 中间计数路径已有测试；元数据继续显式拒绝 TTL，属于正确行为。
- 边计数静止快路径：无洞端点表且水位线以下时直接使用物理腿长度；相关语义由元数据计数单测锁定。
- 顶点装入：分段计时、锁存批量化、列柄按组记忆、纯插入跳过预检已落地；分片并行装入仍未做，归入存储持久化问题。

---

## 1. 算子与公开数据集基准覆盖不足

**现象**：三方 parity 原有范围只有 LSQB sf0.1 的九条纯计数。仓库里已有 54 条 LDBC、43 条 ClickBench、算法图查询源，但都没有接入 Linkrs parity runner；跳过清单此前为空，无法区分“不支持”和“未验证”。本轮已补十一道算子 smoke 用例，并把两条方言不兼容用例写入跳过清单。

**完成定义**：

- LSQB sf0.1/sf1 覆盖过滤、表达式求值、顺序扫描、聚合、分组、排序、分页、去重、连接、多关系、递归、最短路和 LDBC IC/IS 形态。
- ClickBench 有 schema、导入适配和查询映射；算法图有 PageRank、WCC、加权最短路用例，k-core/SCC/Louvain 进跳过清单。
- 每条用例有三方状态、行数或数值匹配、执行中位数、编译中位数、RSS 峰值和库体积；偏差达到 20% 时高亮。
- 跳过的每条记录写明原因和解禁条件，不再用空清单掩盖未覆盖。

**验证方式**：使用三方 runner 在同机、同数据、同 warmup/run 口径下逐规模跑；整数精确比对，浮点相对误差 1e-3，TopN 先做稳定排序再比对。

**当前测试锚点**：

- 新增 `benches/parity/queries/lsqb/operators/` 十一道可执行 smoke 用例，覆盖过滤、或条件、最小值/最大值、聚合过滤、排序分页、分组计数、去重、两跳连接、无方向计数、变长路径和最短路；`distinct_limit` 已补 `ORDER BY`，避免无界 LIMIT 造成三方首值不一致。
- `benches/parity/skip.txt` 已记录聚合过滤与最短路的方言不兼容原因及解禁条件。
- 新增 parity runner 自测：嵌套目录发现、上游 `-NAME/-QUERY/-PRERUN` 标记、数值期望块、跳过原因解析、精确/浮点/无序三种比较模式。
- 现有 `docs/archive/perf/parity/2026-11-12-lsqb-sf0.1.md` 只作为历史数据，不作为当前结论。

**明确不做**：不把 sf100 的 expected 数值套到 sf0.1/sf1；不把无法语义对齐的查询改成近似查询后上报。

### devs 实测指标与对比发现（2026-11-25，资源对齐重测）

环境：devs（E5-2667 v3，`powersave`，内核 4.15）；三方统一 `numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`，实际亲和性均为 `8-15,24-31`，参数线程数均为 16，warmup 1 + run 5 去最大值平均。Ladybug 0.22.3 wheel 使用 `buffer_pool_size=67108864`；Neug 0.3.1 使用 `M_FULL`，AP 路径为单线程 wall-clock，只作参照；Linkrs 64 MB buffer pool、引擎内计时。Linkrs 固定门为 16 workers / 10 partitions，生产默认门为配置默认 8 workers / 8 partitions。原始日志、EXPLAIN 与三方 JSON 在 `perf-data/parity/results/2026-11-25/resource-aligned/`。

| 用例 | Linkrs 生产默认 ms | Linkrs 固定门两轮 ms | Ladybug ms | Neug ms | 行数/首值 | 资源对齐后的判定 |
|---|---:|---:|---:|---:|---|---|
| `aggregate_filtered_count` | — | — | error | error | 1 / NA / NA | Linkrs 方言，双方拒绝，跳过 |
| `aggregate_min_max` | 1.22 | 1.28 / 1.28 | 0.95 | 0.09 | 1 / 14 / 14 | 对 Ladybug 1.29× |
| `distinct_limit` | 27.28 | 29.98 / 29.96 | 3.20 | 28.33 | 10 / 14 / 14 | 对 Ladybug 8.52×，对 Neug 同量级 |
| `filter_id_range` | 1.69 | 1.58 / 1.80 | 1.24 | 0.15 | 1 / 1700 / 1700 | 对 Ladybug 1.37× |
| `filter_or` | 1.53 | 1.42 / 1.62 | 1.21 | 0.32 | 1 / 1603 / 1603 | 对 Ladybug 1.27× |
| `group_by_count` | 25.92 | 32.05 / 32.53 | 8.16 | 12.28 | 10 / 14 / 14 | 对 Ladybug 3.18×，最大缺口 |
| `join_two_hop` | 5.38 | 5.94 / 5.93 | 1.54 | 20.14 | 1 / 382018 / 382018 | 对 Ladybug 3.50×，已快于 Neug |
| `order_limit` | 1.21 | 1.33 / 1.34 | 0.46 | 0.04 | 10 / 14 / 14 | 对 Ladybug 2.64× |
| `shortest_path` | — | — | error | error | 0 / NA / NA | Linkrs 方言，双方拒绝，跳过 |
| `undirected_count` | 4.53 | 5.16 / 5.03 | 1.59 | 1.51 | 1 / 36270 / 36270 | 对 Ladybug 2.85× |
| `variable_path_count` | 5.38 | 5.60 / 5.90 | 441.42 | 102.90 | 1 / 400153 / 400153 | 约 82× 快于 Ladybug |

两条阶段一用例在生产默认 EXPLAIN 中 `expand_output_rows=30536`、固定门为 30473，说明身份主键直通生效，不再输出逐边行；两轮固定门与生产默认均 `result_stable=true`、`spill_rows=0`、`actual=10/8`。Linkrs RSS 峰值约 323–326 MB，DB 约 46 MB；Ladybug 显式 64 MB buffer，Neug 全内存，三者缓存策略不同，Neug 数值只作参照。

**本轮结论**：

1. 资源对齐后，`group_by_count`、`distinct_limit`、`join_two_hop` 仍是主要差距；`variable_path_count` 已大幅领先三方。
2. 固定门 16 workers / 10 partitions 在 node1 上不再稳定优于生产默认；两条阶段一用例生产默认更快，说明旧“配置未受益”的判断已经过期。
3. 旧 `2026-11-18` 三方 JSON 及其倍率（Ladybug 7.10 / 3.755 / 2.422 ms 等）因跨节点绑定与 Ladybug 默认大 buffer 不再可比，devs 上旧目录与 JSON 已删除。
4. `aggregate_filtered_count` 与 `FIND SHORTEST PATH` 仍为 Linkrs 方言，继续跳过；不把无法语义对齐的查询改成近似查询。

---

## 2. 带谓词、分组与去重计数仍缺快路径证据

**现象**：无过滤全局计数已走元数据或向量化路径；带过滤聚合虽然语义正确，但逐行裁决后累加，行计数快捷路径和批量表达式路径关闭。分组空值语义缺少专门锁定，去重计数按设计保持慢路径。2026-11-18 原始实测中分组计数约 1.3–1.5 秒、去重加排序分页约 1.1 秒；资源对齐后已分别降到 25.92 / 27.28 ms，但仍是当前最突出的算子缺口，见第 1、49 节。

**完成定义**：

- 等值和范围谓词可下推到扫描谓词的形状复用计数快路径，下推失败保持慢路径，不做部分下推。
- 带过滤计数、分组计数、空输入分组、可空分组键有端到端数值一致性测试；性能基准同时覆盖可下推和不可下推两种形态。
- 去重计数继续保持慢路径，只用基准记录其成本，不为追求数字改语义。
- 任一快路径开启前后，整数结果必须完全相等。

**验证方式**：在 `linkrs-query` 现有基准中补充同语义快慢路径对照，记录中位数和计划标记；端到端以 DQL 聚合测试为正确性门。

**当前测试锚点**：

- `crates/linkrs-query/tests/dql/aggregation.rs` 新增两处测试：过滤聚合与 WHERE 过滤计数在非空输入上等价，且过滤后零命中仍返回单行零；空扫描上全局计数与过滤聚合均返回单行零。
- `crates/linkrs-query/tests/dql/metadata_count.rs` 已锁定纯计数元数据门限、回退与无方向慢路径语义。

**明确不做**：不实现多谓词复杂组合下推；不对去重计数做向量化。

---

## 3. 分区灰度只有机制，没有放量证据

**现象**：分区配置已提供纯计数链与全局聚合两个放行开关，并有单测；但总开关默认关闭，worker 与分区上限为一，配置文档未透出，也没有放行前后性能对比。

**完成定义**：

- 纯计数链先完成灰度：保持存储自证数据域和布局版本前置，数值不变，耗时下降。
- 全局聚合在分区聚合正确性与溢出论证完备后再放行；普通扫描与连接继续关闭。
- 经济门、实际分区数、回退原因在 EXPLAIN 与运行报告中可见。
- 未放行形状计划保持不变。

**验证方式**：按形状 × 放行开关 × worker 数跑矩阵，记录行数一致性、实际分区数、回退原因和中位耗时；使用安静窗口和对照组排除机器漂移。

**当前锚点**：`crates/linkrs-query/tests/parallel_partition_execution.rs` 与 `crates/linkrs-query/src/optimizer/partitioning/tests.rs` 已覆盖放行、未放行和形状不外泄；缺少性能基准与配置文档透出。

**明确不做**：不做跨分片事务和动态重分区。

---

## 4. 内存口径与资源报告不完整

**现象**：内存预算和索引缓冲池配置早已存在，runner 会打印进程 RSS 峰值，Profile 有每算子峰值内存和并行缓冲字节峰值；但 EXPLAIN 没有顶层峰值内存与库体积，也没有按预算扫参数的性能基准。

**完成定义**：

- EXPLAIN / PROFILE 输出顶层峰值内存与库体积；类型化布局只计数，不改变分配行为。
- parity 报告同时记录 RSS 峰值、库体积、当前缓冲池上限和溢出计数。
- 不同上限下同一基准的峰值报告单调有界，查询数值不变。

**验证方式**：在公开数据集导入与查询两侧按内存上限三档对照，记录峰值、溢出、耗时和行数。

**当前锚点**：`src/bin/linkrs_parity_bench.rs` 已读取 VmHWM；`PlanDescription` 已有并行缓冲峰值和每算子 Profile 峰值，但没有库体积字段。

**明确不做**：不新增内存超限时的查询降级策略。

---

## 5. 图算法与复杂查询未对齐

**现象**：递归片段算子、双向 BFS、Dijkstra 和 `pagerank` 内置函数存在；WCC/SCC/k-core/Louvain 不存在，算法图查询没有接入 parity runner。

**完成定义**：

- 固定长度递归、Trail、全路径、最短路先在现有算子上跑通并锁定数值，再谈优化。
- 纳入 PageRank、WCC、加权最短路；k-core、SCC、Louvain 进跳过清单并写明解禁条件。
- 优化递归不动点物化与受控并发；多源最短路保持明确语义，不静默截断。
- 数值门槛：整数精确，PageRank 等浮点相对误差 1e-3。

**验证方式**：在 soc-livejournal 或同等图数据上三方对齐；图500 数据目录当前只有 schema/copy 文件，需先补齐数据后再纳入。

**当前锚点**：`crates/linkrs-query/tests/dql/find_path.rs` 覆盖路径与最短路语义；`benches/parity/queries/lsqb/operators/shortest_path.benchmark` 只是可执行 smoke，且 Linkrs 专用 `FIND SHORTEST PATH` 方言已被 Ladybug/Neug 拒绝并写入跳过清单；变长路径计数在 2026-11-18 原始实测中约 1.1–1.35 秒；资源对齐后为 5.38 ms，已快于三方，见第 1、49 节。

**明确不做**：不做近似算法和截断采样。

---

## 6. 无方向元数据计数仍未放行

**现象**：元数据路径对 `Both` 方向显式拒绝；慢路径公式已锁定为种子侧出腿加种子侧入腿减存活自环。该公式是未来放行的唯一 oracle。

**完成定义**：仅在单边类型、源宿同标签、无截断采样、端点无 TTL 的窄前置下放行；其余形状继续拒绝。放行前后数值必须逐例相等，且性能基准证明有收益。

**验证方式**：复用 `metadata_count.rs` 的无方向固件，补充 hole、pending write、TTL、跨标签四种反例；新增加快路径前后的基准对照。

**当前锚点**：`crates/linkrs-storage/src/engine/graph_storage/reader/edge.rs` 的拒绝分支；`crates/linkrs-query/tests/dql/metadata_count.rs` 的无方向公式测试。

**明确不放宽**：存活期元数据拒绝、多类型同构、带采样的无方向计数。

---

## 7. 存储持久化与顶点并行装入

**现象**：WAL 组提交、批量会话、专用检查点线程池和显式容量错误已在位；io_uring 受内核限制不可用，冷检查点第二阶段、批量回放和分片并行顶点装入未做。

**完成定义**：

- 组提交与检查点分阶段计时覆盖顶点装入、WAL 追加、同步点、时间戳四段。
- 分片并行装入前完成身份分配器锁粒度、预留释放并发安全、跨分片补偿顺序、提交映射合并四项审计。
- 崩溃恢复、容量超限与并发写入用例全绿；同步点作为下限保留，不通过牺牲持久性换速度。

**验证方式**：全量导入两轮以上、崩溃恢复用例、WAL/恢复/回滚基准；结果须与当前 11.2–11.3 秒基线对比。

**当前锚点**：`tests/e2e/perf_bulk_load.rs`、`crates/linkrs-storage/benches/wal_bench.rs`、`crash_recovery_bench.rs`、`rollback_bench.rs`。

**明确不做**：不做跨机复制、在线迁移和静默截断。

---

## 8. 优化器复杂度纪律

**现象**：单遍快形状链仍然成立，但规则数量持续增加，跨批震荡风险没有消失。

**完成定义**：每个新规则先补反例测试再合入；规则注册按形状裁剪；恒真消除、无操作投影消除、身份注解三者的交互用例继续通过；不新增替代模块。

**验证方式**：优化器全量单测与计划等价语料；EXPLAIN 计划标记变化只涉及新增断言，不改变查询数值。

---

## 9. 本轮已补的测试与基准资产

| 资产 | 作用 |
|---|---|
| `benches/parity/queries/lsqb/operators/*.benchmark` | 十一条可执行算子 smoke 用例，作为后续补 expected 和三方对比的载体 |
| `benches/parity/skip.txt` | 记录两条 Linkrs 方言专用用例的跳过原因与解禁条件 |
| `src/bin/linkrs_parity_bench.rs` 内联单测 | 锁定嵌套目录发现、上游标记解析、跳过原因解析、浮点/无序比较 |
| `crates/linkrs-query/tests/dql/aggregation.rs` 两处新增用例 | 锁定过滤计数与 WHERE 计数等价、空输入单行零 |
| `perf-data/parity/results/2026-11-18/` | 历史过程记录；旧三方 JSON 因资源口径不可比，devs 上原始目录与 JSON 已删除 |
| `perf-data/parity/results/2026-11-25/resource-aligned/` | 当前权威三方数据：Linkrs gate 日志/JSON/EXPLAIN，Ladybug/Neug 同绑定 JSON 与 runner 日志 |
| devs `/data/kkkqkx/linkrs/resource-aligned-parity/` | 当前 Ladybug/Neug 原始 JSON、日志与数据库目录 |
| `docs/issue/linkrs-remaining-parity-issues.md` | 本文件，剩余任务与验收口径的唯一入口 |

新增 parity 用例当前不带 `----expected`，只作为可执行与行数记录载体；expected 必须由同规模参考实现生成后回填。运行命令与报告格式仍以 `benches/parity/README.md` 为准。

---

## 10. 实施顺序

1. 先定位 `group_by_count` 与 `distinct_limit` 的秒级成本，按每算子行数和计划形状拆分；这两个缺口远大于其余算子。
2. 补齐 parity runner 的 compare 脚本，把现有两轮结果固化为可重复的三方对比产物，并在安静窗口复核比例。
3. 为同一批用例生成同规模 expected，按算子组扩大 LDBC 覆盖；方言专用用例继续留在跳过清单。
4. 补过滤/分组计数的快慢路径对照基准，再实现窄前置下的快路径。
5. 用矩阵基准验证分区灰度，随后更新配置文档和默认开关。
6. 补顶层资源报告与内存预算扫描。
7. 最后处理图算法、无方向元数据放行和存储层深水优化。

---

## 11. 阶段一实测回填（2026-11-18）

本轮在 devs 上编译本地工作树（HEAD `d814ab92` 加阶段一改动），绑核 `16-31`，口径仍为 warmup 1 + run 5。原始日志、逐用例 JSON、验证日志与 PROFILE 输出落在 `perf-data/parity/results/2026-11-18/stage1-final/`，远端同名目录在 `/data/kkkqkx/linkrs/stage1-gate/`。

改动落点：聚合算子增加单列定长键的分组计数路径；identity 去重与 blocking `Distinct` 增加单列 `BigInt` 键集合；单步与变长展开改用一次解析 schema 的批量投影边读取，目的地读取先按 id 去重再恢复行顺序。NULL/可空键、多列、非计数聚合和聚合过滤全部保留原路径。

固定门两轮中位数（ms）：

| 用例 | 基线两轮 | 本轮两轮 | 变化 |
|---|---:|---:|---|
| `distinct_limit` | 1154 / 1218 | 521 / 515 | 约 2.2–2.4× |
| `group_by_count` | 1308 / 1499 | 645 / 656 | 约 2.0–2.3× |
| 九条纯计数 | 0.14–0.60 | 0.14–0.60 | 无回归 |

PROFILE 显示聚合自身约 49 ms、blocking `Distinct` 自身约 23 ms；剩余时间主要落在 `ExpandAll`（约 0.40–0.43 秒）及其后的行物化（分组约 0.16 秒）。因此阶段一把目标算子的直接成本压进几十毫秒，但端到端未达到方案要求的至少一个数量级，**按硬门槛阶段一尚未闭环**。下一轮应先切断 ExpandAll 到 Project 之间的行物化，而不是继续调聚合。

正确性门：devs 上 `cargo test -p linkrs-query --test dql` 为 319/319，`cargo test -p linkrs-query --lib -- streaming` 为 414/414；两轮固定门用例状态与行数一致。

---

## 12. 行物化切断实测回填（2026-11-18）

阶段一之后 PROFILE 显示算子自身已降到几十毫秒，端到端瓶颈前移到 `Project` 的行物化与 `ExpandAll`。本轮设计是让规划器在「passthrough Project 的直接消费者是 typed-capable 的 `Aggregate`/`Dedup`，且输入子树含 rowless hop」时给 Project 打标；Project 收到 rowless chunk 后直接产出 typed-only chunk，不再转置成 `Vec<Vec<Value>>`。`count(h)` 的 identity 参数在 rowless Project 中被替换为非空单位列，而聚合只观察它的 NULL 性。Plain `Fallback` 标量列在同构时转成 typed batch；异构列仍回退行路径。`Aggregate` 的单列分组计数和 blocking `Distinct`/identity `Dedup` 都补上了 rowless 消费。

标记必须落在优化器克隆后的计划节点上，且字段需要随逻辑计划互转传播，否则会在 CBO 重建时丢失；这一点已用规划器单测锁定。曾尝试把 rowless identity 扫描的 morsel 放大到 8192，实测无收益，已回退。

固定门两轮中位数（ms）：

| 用例 | 阶段一 | 本轮 | 相对基线 |
|---|---:|---:|---|
| `distinct_limit` | 521 / 515 | 480 / 498 | 约 2.4–2.5× |
| `group_by_count` | 645 / 656 | 472 / 488 | 约 2.8–3.2× |
| 九条纯计数 | 0.14–0.60 | 0.14–0.60 | 无回归 |

PROFILE 显示 Project 已输出 `rows: 0`，单算子成本从约 0.16 秒（分组）降到约 18 ms；聚合约 15 ms，blocking `Distinct` 约 26 ms。剩余约 0.40–0.41 秒全部在 `ExpandAll`，主要是每批投影边的点读与行组装。因此**十倍门槛仍未达成**。下一阶段需要针对 `ExpandAll` 本身：按边类型一次性拉取拓扑列、跳过 `Edge` 对象构造，并评估更大的扫描 morsel 与自适应并行；单纯继续优化聚合/去重已无收益。

正确性门：devs 上 DQL 319/319、streaming lib 414/414；原始日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage2-final/`。

---

## 13. 拓扑批量读取实测回填（2026-11-18）

设计见 `docs/plan/expand-topology-view-plan.md`。本轮参考 Ladybug 的 CSR 区间顺序读取与 Neug 的零拷贝邻居视图，只借鉴读取形态，不改拓扑/属性布局、边身份与外部 MVCC 权威。存储层新增按批返回拓扑字段的原语：一次解析空间与边类型，直接把 CSR 记录转成共享边类型名加端点与 rank，不构造 `Edge`、不解码属性。查询层仅在拓扑-only 的单步展开使用它；有边属性需求、变长展开和 TTL 相关语义保持原路径。

固定门两轮中位数（ms）：

| 用例 | 阶段二 | 本轮 | 相对基线 |
|---|---:|---:|---|
| `distinct_limit` | 480 / 498 | 421 / 414 | 约 2.8–2.9× |
| `group_by_count` | 472 / 488 | 402 / 398 | 约 3.3–3.8× |
| 九条纯计数 | 0.14–0.60 | 0.14–0.60 | 无回归 |

PROFILE 中 `ExpandAll` 从约 0.46 秒降到约 0.39–0.40 秒，`Project` 仍为 `rows: 0`，聚合与去重仍在几十毫秒以内。端到端十倍门槛仍未达成；当前 `ExpandAll` 剩余时间主要落在每批邻接区间读取与目的地存在性判定上。后续应按设计文档继续做“单次调用内的 CSR 顺序投影 + 存在性半连接”合并，并在完成分区聚合正确性论证后再启用扫描并行。

正确性门：devs 上 DQL 319/319、streaming lib 414/414；日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage3-final/`。

---

## 14. 内部句柄透传实测回填（2026-11-18）

本轮继续收紧展开读取：存储层在热 CSR 槽上直接构造拓扑，不再逐边构造 `EdgeRecord`、解码空投影属性或分配属性向量；同一批处理内共享端点翻译结果。拓扑-only 顶点扫描把内部行号和标签通过 `VertexHandle` 列透传给展开算子，展开调用按句柄直连 CSR，标签不匹配或句柄缺失时才回退到外部 ID 解析。`BatchColumn`/`TypedColumn` 的通用 gather、比较、累积与截断同步支持新列，保持 rowless 与 rowful 语义一致。

固定门两轮中位数（ms）：

| 用例 | 阶段三 | 本轮 | 相对基线 |
|---|---:|---:|---|
| `distinct_limit` | 421 / 414 | 299 / 296 | 约 3.7× |
| `group_by_count` | 402 / 398 | 283 / 284 | 约 4.6–5.3× |
| 九条纯计数 | 0.14–0.60 | 0.14–0.61 | 无回归 |

PROFILE 中 `ExpandAll` 从约 0.39–0.40 秒降到约 0.27 秒，`Project` 仍为 `rows: 0`，聚合与去重仍在几十毫秒以内。十倍硬门槛仍未达成。剩余成本按量级依次是目的地属性读取、CSR 邻接遍历、端点翻译与源扫描。下一步需要把目的地存在性与投影属性并入同一次拓扑订单调用，并在完成分区聚合正确性论证后再启用扫描并行；不应建立跨调用可见性缓存，也不应把属性永久内联进 CSR。

正确性门：devs 上 DQL 319/319、streaming lib 414/414；日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage5-final/`。

---

## 15. 目的端半连接原型结论（2026-11-18）

设计见 `docs/plan/expand-destination-semijoin-plan.md`。原型把目的端存在性与属性读取并入拓扑调用，顶点读复用 CSR 内部行号。正确性门通过，但固定门中位数回退到 `distinct_limit` 346/349 ms、`group_by_count` 330/330 ms；PROFILE 中 `ExpandAll` 从约 0.27 秒升到约 0.31 秒。回退来自每条边新增的目的端标签与共享顶点引用，以及边顺序与唯一目的端之间的双映射维护。

原型已撤回，当前代码与指标回到阶段五状态：`distinct_limit` 302/295 ms、`group_by_count` 283/286 ms，九条纯计数无回归。目的端半连接方向没有证伪，但下一次应先改成拓扑与目的端顶点分别返回按边顺序对齐的数组，避免扩大每条边的拓扑结构；没有实测收益前不保留该复杂度。

正确性门：devs 上 DQL 319/319、streaming lib 414/414；日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage6-final/`。

---

## 16. 目的端半连接二版与阶段收敛（2026-11-18）

按设计文档建议把拓扑与目的端顶点拆成两个按边顺序对齐的数组，去掉 `EdgeTopology` 的额外字段，并复用 CSR 内部行号批量读顶点。正确性门通过，但固定门中位数回退到 `distinct_limit` 371 ms、`group_by_count` 353 ms，比第一版更差。由此确认回退不来自 `Arc<Vertex>` 字段，而是目的端唯一化 + 专用批量读路径本身在当前小基数目的顶点场景下不划算。

两版原型均已撤回，最终代码保持在阶段五状态。撤回后最终门两轮中位数为 `distinct_limit` 305 / 313 ms、`group_by_count` 283 / 306 ms，九条纯计数无回归；DQL 319/319、streaming lib 414/414。

结论：目的端半连接方向在当前规模下证伪。后续优化应转向执行层有序并行与分区预聚合，而不是继续改目的端读取接口。日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage7-final/`。

设计文档见 `docs/plan/ordered-parallel-expand-plan.md`，作为下一阶段独立任务。

## 17. 有序并行展开实测回填（2026-11-19）

设计见 `docs/plan/ordered-parallel-expand-plan.md`，细化落点见该文档“落地细化”。本轮把扫描到 Project 的前段放进既有 morsel 工作池，以分区序号在 `Exchange` 恢复顺序；聚合与去重保留原有局部/最终合并语义。总开关默认关闭，新增 `allow_ordered_parallel_expand` 只放行单跳、rowless、无聚合 FILTER 的窄形状；`count_only` 快路径、多跳、无最终排序的 DISTINCT 继续串行。

实现中修正了三处让并行形状无法闭环的问题：rowless typed chunk 在 `Exchange` 被当成空块并强制物化、有界缓冲按字节记账时忽略 typed 列、分区树未写入 `partition_id` 且状态 arena 未按计划分区数扩展。嵌入式装配改为共享调度器，避免每条语句重新拉起 worker 线程而拖慢纯计数。

devs 上 release 编译通过；DQL 319/319、streaming lib 414/414、`parallel_partition_execution` 25/25。固定门用 `--parallel-workers 16` 两轮，warmup 1 + run 5，绑核 `16-31`：

| 用例 | 阶段七基线两轮中位数 ms | 本轮两轮中位数 ms | 相对基线 |
|---|---:|---:|---|
| `distinct_limit` | 305 / 313 | 139 / 130 | 约 2.2–2.4× |
| `group_by_count` | 283 / 306 | 128 / 149 | 约 2.0–2.4× |
| 九条纯计数 | 0.14–0.60 | 0.14–0.58 | 无回归 |

`EXPLAIN ANALYZE` 显示 `requested=16, actual=12`，并行 wall_time 约 125 ms、work_time 约 1.2 s、缓冲块峰值 11、缓冲字节峰值约 1.31 MB；PROFILE 中每个分区的扫描/展开/局部聚合独立可见且无 spill。零 worker 对照运行证明纯计数回归只是 worker 线程启动开销，共享调度器已消除。

结论：本轮达成“至少 2 倍级别提升”的验证目标，但未达到问题清单中的十倍硬门；剩余成本主要是分区内单跳展开和局部聚合的行重建，继续下探需要先解决 typed 局部聚合的直接消费，而不是再加并行度。下一阶段设计见 `docs/plan/typed-partition-aggregation-plan.md`。原始日志与 JSON 在 `perf-data/parity/results/2026-11-18/stage8-final/`，远端同名目录在 `/data/kkkqkx/linkrs/repo/perf-data/parity/results/2026-11-18/stage8-final/`。

## 18. Typed 分区局部聚合实测回填（2026-11-19）

设计见 `docs/plan/typed-partition-aggregation-plan.md`。本轮把固定宽度分组计数状态抽成 `Aggregate` 与 `PartialAggregate` 共用的模块；局部聚合在 typed-only chunk 上直接累计 I64/I32/Bool 键，遇到 nullable 键、多列分组、聚合 FILTER、selection 或预算不足时先把计数转入通用累加表，再让该 chunk 整体走逐行路径。最终合并与 Distinct 未改动，生产默认并行配置也未改动；benchmark 增加仅用于校准的 `--min-rows-per-partition` 覆盖参数。

devs 上 release 编译通过；DQL 319/319、streaming lib 419/419（新增四个 typed 状态单测与一个 rowless multiplicity 回归测试）、`parallel_partition_execution` 25/25。固定门先在 `operators` 子目录做了两轮快速对照，随后按阶段八同口径（LSQB 根目录 + 两份方言跳过项）补跑六轮；下表以同口径六轮为准，每格为 warmup 1 + run 5 的中位数 ms：

| 用例 | 阶段八两轮 | 本轮六轮 | 相对均值 |
|---|---|---|---|
| `group_by_count` | 128 / 149 | 118 / 97 / 113 / 116 / 114 / 113 | 约 −19% |
| `distinct_limit` | 139 / 130 | 123 / 146 / 146 / 126 / 142 / 124 | 约 −0.1%，落在噪声范围 |
| 九条纯计数 | 0.14–0.60 | 0.140–0.588（单轮噪声上探 0.606） | 无回归 |

`EXPLAIN ANALYZE` 中 `Project` 后一行仍为 `rows: 0`，`PartialAggregate` 输出约 790–1180 行；分区 spill 为 0。两次 PROFILE 缓冲峰值为 10 块 / 1.18 MB 与 6 块 / 0.71 MB，`requested=16, actual=12`，wall_time 约 105–130 ms、work_time 约 1.04–1.38 s；把 `min_rows_per_partition` 从默认 100000 降到 50000 后实际分区数升到 16，但 wall_time 反而升到约 179 ms、work_time 约 2.47 s，说明收益来自 typed 消费而不是分区数，故不调整默认值。

结论：`group_by_count` 达到并超过 15% 的改进门槛，`distinct_limit` 与九条纯计数无回归，保留本次改动。原始日志、逐用例 JSON 与校准 PROFILE 落在 `perf-data/parity/results/2026-11-18/stage9-typed-aggregation/`，远端同名目录在 `/data/kkkqkx/linkrs/repo/perf-data/parity/results/2026-11-18/stage9-typed-aggregation/`。

## 19. 方案阶段二：两跳连接与变长路径计数回填（2026-11-19）

按 `docs/plan/linkrs-remaining-parity-implementation-plan.md` 的阶段二执行。两跳计数在优化器里把「同标签、同边类型、同方向、无过滤与属性需求」的相邻两跳折叠成一次固定深度 `2` 的 count-only 展开；中间跳只作种子来源，不再物化目的地。`Flatten` 是行保持算子，折叠时穿过它。变长计数把无路径语义的 `*1..2` walk 收进同一套度数前沿：逐层用批量邻接读取推进前沿，用批量出度累加尾边数；TTL、开放 schema、受限重复规则与空路径语义一律回退到物化行数统计。EXPLAIN 增加 `steps` 标记，固定形状显示 `steps: 2` 或 `steps: 1..2`。

正确性护栏：属性计数、Trail 语义、不同边类型链、带过滤形状全部保持慢路径；自环、重复边、空输入、TTL 过期边与小图物化对照均由 DQL 测试锁定；`count_only` 注解要求 `include_empty_paths` 关闭，避免 OPTIONAL 的 NULL 行被误计。

devs 上 release 编译通过；`expand_pushdown` 单测 22/22、DQL 322/322、streaming lib 419/419、`parallel_partition_execution` 25/25。并行测试中原先断言「hop1 id_only + hop2 count_only」的计划形状已随折叠改为「单个 count-only 二跳前沿」，数值与并行一致性断言不变。固定门沿用 LSQB 根目录、`--parallel-workers 16`、绑核 `16-31`、warmup 1 + run 5，共四轮；每格为单轮执行中位数 ms：

| 用例 | 阶段九中位数（六轮范围） | 本轮四轮 | 相对阶段九均值 |
|---|---:|---:|---|
| `join_two_hop` | 41（历史两轮） | 13.4 / 14.0 / 14.3 / 14.2 | 约 3× |
| `variable_path_count` | 97–146 | 13.8 / 14.5 / 14.6 / 13.8 | 约 70–80× |
| `group_by_count` | 95–118 | 95.6 / 99.2 / 122.8 / 95.6 | 无回归，噪声内 |
| `distinct_limit` | 114–146 | 114.3 / 124.5 / 136.2 / 115.0 | 无回归，噪声内 |
| 九条纯计数 | 0.140–0.606 | 0.146–0.654 | 无实质回归，单轮漂移 |

`variable_path_count` 已从秒级降到十几毫秒，超过与 Ladybug 同数量级的目标；`join_two_hop` 当时约 14 ms，相对旧跨节点口径下 Ladybug 的 2.42 ms 仍有约 5–6 倍差距，但已消除中间路径行物化；资源对齐后的当前倍率见第 1、49 节。后续收益需来自更窄的度读取或并行前沿，而不是继续改聚合。原始日志与四轮 JSON 落在 `perf-data/parity/results/2026-11-18/stage10-two-hop-variable/`，远端在 `/data/kkkqkx/linkrs/stage2-results/` 与 `/data/kkkqkx/linkrs/stage2-gate3.log`。

## 20. 两跳计数读取合并实测回填（2026-11-19）

设计见 `docs/plan/two-hop-count-read-fusion-plan.md`。第 19 节后 `EXPLAIN ANALYZE` 显示两个固定门的顶点扫描仅约 0.5 ms，剩余时间全部在 `ExpandAll`。本轮在存储查询 trait 上增加按批返回「邻居度数之和」的只读原语：原生引擎对单类型、单方向形状先暂存内部端点，再按内部行号读尾边度数；外部 ID 只在一次批量解析中用于保持悬空邻居可见性和读集合记录，不进入查询层行。多类型、`Both` 与非原生适配器沿用原两次调用组合。查询层固定深度 `2` 与范围 `1..2` 直接消费新原语，不再构造中间邻居 ID 向量；TTL、开放 schema 与受限路径语义仍回退物化行数。

原生实现与可移植组合的等价性由新存储测试锁定，覆盖自环、平行边、悬空端点与 `Out`/`In` 双方向；DQL 自环、TTL 与物化对照测试继续通过。devs 上 release 编译通过；存储新测试 2/2、`expand_pushdown` 单测 22/22、DQL 322/322、streaming lib 419/419、`parallel_partition_execution` 25/25。固定门沿用第 19 节口径，四轮中位数 ms：

| 用例 | 第 19 节四轮 | 本轮四轮 | 相对上轮均值 |
|---|---:|---:|---|
| `join_two_hop` | 13.4–14.3 | 6.31 / 6.73 / 6.72 / 6.78 | 约 2.1× |
| `variable_path_count` | 13.8–14.6 | 6.75 / 7.16 / 7.21 / 7.21 | 约 2.0× |
| `group_by_count` | 95.6–122.8 | 104.0–107.1 | 无回归 |
| `distinct_limit` | 114.3–136.2 | 121.7–128.2 | 无回归 |
| 九条纯计数 | 0.146–0.654 | 0.155–0.686 | 噪声内 |

`EXPLAIN ANALYZE` 中 `ExpandAll` 从约 15.3 ms 降到约 7.8 ms，顶点扫描仍约 0.6 ms。实现中曾采用逐邻居解析外部 ID 的版本，两跳约 10 ms；改为一次批量解析后保留相同悬空邻居语义并恢复到约 7 ms。结论：阶段二剩余改进达成，两个目标用例相对上一轮各再降约一半；`join_two_hop` 相对当时的 Ladybug 2.42 ms 仍约 2.6–2.8 倍；该倍率已被资源对齐重测取代，当前为 3.50×，见第 1、49 节。后续若继续下探需转向并行前沿或分区计数，属于另一阶段任务。日志与 JSON 落在 `perf-data/parity/results/2026-11-18/stage11-read-fusion/`，远端在 `/data/kkkqkx/linkrs/readfusion-results/` 与 `/data/kkkqkx/linkrs/readfusion-gate3.log`。

## 21. 并行计数前沿漂移修复与放行恢复（2026-11-19）

设计见 `docs/plan/parallel-frontier-partition-counting-plan.md`。同二进制 A/B 曾发现并行前沿结果在不同执行间波动：串行稳定返回 382018 / 400153，前沿开启时局部计数在 381895–382070 与 399086–400382 之间变化。逐层取证显示，同一批种子解析出的端点内部 ID 相同、边表分片相同、读时间戳与 pending gate 相同，但同一端点的度数不同；最终定位到 CSR 分组的共享无锁路由缓存：并发 worker 可能把一个新 `vid` 与旧槽位残留的 `(gid, local)` 组合起来，从而读到错误的行。

修复移除该可变路由缓存，路由直接由不可变的 group bits 与 group map 推导；同时删除对应的阈值项与缓存并发测试。`EXPLAIN ANALYZE` 保留 `partition_count_check`，benchmark 在每次计时后校验结果值稳定性，脚本遇到 `result_stable=false` 即失败。配置开关仍默认关闭，元数据计数因缺少 shard-local 存储证明保持不生效。

devs 上 release 编译通过；partitioning 47/47、streaming 419/419、DQL 322/322、`parallel_partition_execution` 27/27。冻结 golden DB、`threads=1`、每例 8 轮的同二进制 A/B 中，所有 case 的 `join_two_hop` / `variable_path_count` 均稳定返回 382018 / 400153，未再出现 `result_stable=false`。8 worker 下前沿中位数约 5.84 / 5.67 ms，对应关闭时约 5.80 / 6.16 ms；4 worker 下约 5.75 / 5.81 ms，对应关闭时约 5.69 / 6.05 ms；2 worker 因额外启动开销约 7.20 / 7.10 ms，仍不建议。默认保持关闭；若要启用，建议 `max_partitions` 不超过 8，并保留同轮结果稳定性检查。日志、EXPLAIN 样本与 JSON 落在 `perf-data/parity/results/2026-11-18/count-frontier/`，远端在 `/data/kkkqkx/linkrs/count-frontier-results/`、`/data/kkkqkx/linkrs/count-frontier-ab/` 与 `/data/kkkqkx/linkrs/count-frontier-{gate,matrix}-v9.log`。

## 22. 过滤聚合向量化与谓词下推边界回填（2026-11-20）

按修改方案的阶段三执行。聚合算子为全局 `count` 的逐聚合 `FILTER` 增加批内掩码路径：一次批量求值过滤谓词和必要的计数参数，只遍历布尔结果与 NULL 性累加，不再为每个输入行构造 `ValueRowContext` 与逐聚合裁决。任一聚合不是 `Count`、表达式不在批量求值集合内、或输入为 rowless 时，整批交回通用行路径。分组键、去重计数、并行局部聚合和 spill 路径均未改动。

新增两个 DQL 门：NULL 分组列上的 `FILTER` 与 `WHERE` 在 `>`、`=` 上逐值相等；带函数谓词的过滤计数与同批未过滤计数结果一致。同时新增两条 LSQB 算子基准，`filter_count_pushed` 使用可下推范围谓词，`filter_count_unpushed` 追加一个不可下推的 `<>` 合取，作为快慢路径对照与后续扫描谓词工作的基线。

完整谓词替换的尝试已撤回并保留原计划形状：identity/flat 投影扫描、TTL 表和跨数值类型谓词上，扫描侧谓词与上层 `Filter` 并不等价，删除残余 `Filter` 会造成 14→20、TTL 1→0 等数值回归。缩小 `<`/`<=` 下推白名单的尝试也已撤回：它改变了扫描算子的实际行数与反馈样本，`feedback_does_not_learn_partition_bias_from_range_slices` 会从并行退回串行。NULL 语义差异改由存储侧修复，见第 23 节；`filter_id_range` 等形状继续保留扫描谓词 + 残余 `Filter` 的双层结构。

devs 上 release 编译通过（BUILD_EXIT=0）；`scan_predicate` 9/9、下推规则 2/2、DQL 324/324、streaming lib 419/419、`parallel_partition_execution` 27/27。固定门沿用 LSQB 根目录、`--parallel-workers 16`、绑核 `16-31`、warmup 1 + run 5，两轮；跳过文件只保留方言专用的 `shortest_path`，使 `aggregate_filtered_count` 也进入本地测量。每格为单轮中位数 ms：

| 用例 | 上一轮同口径 | 本轮两轮 | 变化与判定 |
|---|---:|---:|---|
| `aggregate_filtered_count` | 2.67 / 2.98（问题清单历史基线） | 1.23 / 1.24 | 约 2.2–2.4×，批内掩码生效 |
| `filter_id_range` | 1.85（阶段十一） | 1.65 / 1.65 | 噪声内改善，计划标记未变 |
| `filter_or` | 1.69（阶段十一） | 1.49 / 1.50 | 不可下推形状保持残差过滤 |
| `filter_count_pushed` / `filter_count_unpushed` | 新增 | 1.67 / 1.65 与 1.67 / 1.66 | 当前下推形状无差异，作为后续 A/B 基线 |
| 九条纯计数 | 0.14–0.61 | 0.140–0.581 | 无回归 |

`EXPLAIN ANALYZE` 中 `aggregate_filtered_count` 的 `Aggregate` 自身从约 2457 µs 降至约 959 µs，`Project` 与扫描耗时不变；`filter_id_range` 仍显示扫描谓词 `est_rows:1` 与上层 `Filter`，`filter_or` 仍显示 `selection_attached=1` / `selection_pushed=1`，数值分别稳定为 1700 与 1603。结论：过滤计数的向量化目标已闭环；扫描谓词完整替换仍不放行，`<`/`<=` 的 NULL 语义修复见第 23 节。

原始构建日志、测试日志、两轮 JSON、EXPLAIN 与 runner 输出落在 `perf-data/parity/results/2026-11-18/filter-aggregate-final/`，远端同名目录在 `/data/kkkqkx/linkrs/filter-aggregate-results/`，总控日志为 `/data/kkkqkx/linkrs/filter-aggregate-gate.log`，编译日志为 `/data/kkkqkx/linkrs/filter-aggregate-build.log`。

## 23. 扫描谓词 NULL 语义修复回填（2026-11-20）

分析与方案见 `docs/plan/scan-predicate-null-semantics-plan.md`。问题根因不是优化器写法，而是 `ScanPredicate` 与 evaluator 对 NULL 的排序语义不同：NULL 排在标量之前，`<`、`<=` 以及 `literal > property`、`literal >= property` 在 evaluator 下选中 NULL，而存储谓词一律拒绝 NULL。上界区间归一化后就是 `<` / `<=`，因此修复集中在存储侧谓词语义，而非收紧查询层白名单。

改动落点：`ScanPredicate` 新增统一 NULL 判定，上界区间匹配 NULL 与缺失属性；`matches`、`matches_column`、`matches_value` 共用该判定；`PredicateRange::overlaps` 对上界区间不再按非 NULL zone 边界剪枝；列式边过滤把缺失列与 NULL 单元交给同一判定，索引候选路径遇到上界区间退回段扫描。查询层下推规则、`Filter`、并行反馈与分区配置均未改，因此物理行数和计划形状保持稳定。

只做“逻辑统计 / 物理过滤”拆分的替代方案已实测撤回：停止物理下推会改变扫描算子实际行数，范围反馈用例再次退回串行。保留物理下推并修正其 NULL 语义才能同时满足正确性与分区门控。

devs 上 release 编译通过；存储谓词单测 3/3、storage lib 1406/1406、`scan_predicate` 9/9、下推规则 2/2、DQL 324/324、streaming lib 419/419、`parallel_partition_execution` 27/27。DQL 门扩展到 `<`、`<=`、`15 > n.score` 四种 NULL 选中形状，`FILTER` 与 `WHERE` 计数逐值相等；NULL 在 `>`、`=` 上继续被两边同时拒绝。

固定门沿用第 22 节口径，两轮中位数 ms：`aggregate_filtered_count` 1.18 / 1.18，`filter_id_range` 1.64 / 1.62，`filter_or` 1.50 / 1.49，`filter_count_pushed` 1.67 / 1.64，`filter_count_unpushed` 1.72 / 1.66，九条纯计数 0.140–0.583。数值与计划标记不变，过滤计数收益保持在约 2.3×；完整谓词替换仍关闭，解禁条件写入方案文档。

本轮日志、JSON、EXPLAIN 与新增存储测试日志覆盖 `perf-data/parity/results/2026-11-18/filter-aggregate-final/`，远端同名目录在 `/data/kkkqkx/linkrs/filter-aggregate-results/`。

## 24. 下一阶段差距分析与设计

三方指标对照、有序并行 A/B、ref 实现参照与下一阶段任务已写入 `docs/plan/next-stage-parity-gap-design.md`。当前主要差距集中在 `distinct_limit`、`group_by_count`、少量微扫描算子和库体积；两项最大缺口在当时的默认测量中未启用有序并行，恢复可比基线后曾分别约为 Ladybug 的 14× 与 31×；该倍率已被资源对齐重测取代，当前为 8.52× 与 3.18×，见第 1、49 节。下一阶段先冻结测量基线，再做分区扫描/展开/聚合融合、精确局部 TopN、窄前置存储下推与 WAL/checkpoint 体积回收。补齐 `ref/ladybug` 与 `ref/neug` 后，方案文档第 7 节进一步给出按源码定位的借鉴点：packed extend、multiplicity 加权聚合、typed TopN、按度数预取、活动/frozen 双 WAL 等。

## 25. 下一阶段首轮实现与回测

本轮按方案先冻结基线，再落地低风险的融合与观测改动：

- 基准入口在 `parallel-workers > 1` 时默认请求有序并行展开，生产配置默认值不变；新增 `--require-parallel-explain`，对 `group_by_count` / `distinct_limit` 校验 EXPLAIN 的 `actual` 与 `actual_partitions`，不满足时把结果标为 `invalid_parallel` 并返回失败。
- 共享 typed 分组计数直接消费 `chunk.multiplicity()` 作为权重，窄形状下不再展开符号重复行；其他形状仍在 opaque 边界展开后走通用表。
- `DISTINCT` 单列键且 TopN 按同一键排序时，在每个分区 Exchange 前加局部 Distinct/TopK，全局算子保留，避免交换全量去重键；并列、多列键与聚合排序形状不进入该路径。
- 完成检查点后按保留数 2 回收旧 checkpoint 目录；WAL 回收增加安全 LSN 判据，并保留一个 retired 段作为回退点。
- parity JSON 增加 `db_size_bytes`、`rss_peak_kb`、`buffer_pool_bytes`、`spill_rows`，固定门脚本输出资源表。

devs release 编译通过；typed group 单测 5/5、并行分区 27/27、DQL 324/324、checkpoint 保留 2/2、WAL writer 24/24、storage lib 全量通过。两轮固定门 EXPLAIN 均为 `actual=10`、`actual_partitions=10`，结果稳定：`group_by_count` 中位数 107.17 / 93.49 ms，`distinct_limit` 114.13 / 107.22 ms；微扫描算子 1.16–1.82 ms，全部 `spill_rows=0`。资源口径为库体积约 900 MB、RSS 峰值约 949 MB、buffer pool 上限 64 MB。

分区局部 Distinct/TopK 已生效（每分区先收敛到 10 行再交换），但本轮墙钟收益不稳定，尚不能据此宣称达到 20 ms / 15 ms 门槛；主要耗时仍在分区扫描与单跳展开。现有 golden DB 的 WAL 段头均带当前 checkpoint seq，安全 LSN 未覆盖旧段，因此本轮回测中 WAL 文件数仍由 169 增至 171、库体积未下降；库体积目标需继续审计 manifest 的 safe LSN 写入与段头刷新时机。

日志、两轮 JSON、EXPLAIN 与资源表已回填 `perf-data/parity/results/2026-11-18/next-stage-final/`，远端结果在 `/data/kkkqkx/linkrs/next-stage-results/`，固定门脚本为 `scripts/devs-next-stage-gate.sh`。

## 26. 剩余任务设计首轮回测

剩余任务设计写入 `docs/plan/next-stage-remaining-tasks-design.md`。本轮完成其中可安全冻结的基线与资源收尾：

- 固定门新增 `--max-partitions 10`，把基准的经济分区数冻结为 10；生产配置不变。实测 16 分区受内存带宽与交换开销影响反而更慢，10 分区是本机稳定点。固定门仍校验 `actual > 1`。
- 基准进程在计时与 EXPLAIN 后显式释放 session 并调用 `database.close()`；这是库体积问题的根因，此前 `checkpoint_on_close` 从未触发，每个基准进程只会多留一个 WAL 段。
- checkpoint 保留数 2 同时约束快照保留数；新增 `DatabaseConfig::with_snapshots`，基准固定门关闭自动快照，避免每次 checkpoint 再复制一份全量目录。
- WAL 回收保留活动段加一个 retired 段；checkpoint、快照与 WAL 目录回收都只在新检查点发布后执行。

devs release 编译通过；typed group 5/5、并行分区 27/27、DQL 324/324、checkpoint 保留 2/2、WAL writer 24/24、embedded config 通过、storage lib 全量通过。两轮固定门均为 `actual=10`、`actual_partitions=10`：`group_by_count` 中位数 93.45 / 105.87 ms，`distinct_limit` 123.80 / 146.83 ms。微扫描算子仍在 1.2–2.6 ms 量级，全部 `spill_rows=0`。

库体积从回收前的约 900 MB 降到 33.5 MB；目录收敛为 2 个 checkpoint、0 个快照、2 个 WAL 文件，达到 50 MB 目标。回收前后查询结果逐值一致，EXPLAIN 并行标记有效。

日志、JSON、EXPLAIN 与资源表已回填 `perf-data/parity/results/2026-11-18/next-stage-remaining-final/`，远端结果仍为 `/data/kkkqkx/linkrs/next-stage-results/`。

## 27. 精确局部 TopK 合并与回测

按剩余任务设计完成第 3 项的算子级合并：分区内的 Distinct + TopN 替换为单一 `LocalTopKDistinct`。单列定长键直接维护 k 个最优键；遇到 NULL、非定长键或带 selection/multiplicity 的输入时精确退化为全量去重集合，仍由 Exchange 之上的全局 Distinct/TopN 决定最终顺序。新增重复键、并列、k 大于局部行数及退化路径单测。

devs release 编译通过；local topk 4/4、typed group 5/5、并行分区 27/27、DQL 324/324、checkpoint 保留 2/2、WAL writer 24/24、embedded config 通过、storage lib 全量通过。两轮固定门均 `actual=10`：

- `distinct_limit`：中位数 87.83 / 84.26 ms，相对首轮 114 / 107 ms 再次下降；
- `group_by_count`：中位数 95.38 / 92.39 ms；

两轮结果稳定，整数逐值相等，`spill_rows=0`。库体积保持约 33.7 MB，目录为 2 个 checkpoint、0 个快照、2 个 WAL 文件。微扫描算子与查询结果不变。

日志、两轮 JSON、EXPLAIN 与资源表回填 `perf-data/parity/results/2026-11-18/next-stage-localtopk-final/`。分区/聚合进一步融合与微扫描下推仍按设计文档的精确性门槛执行，未在当前数据中开放。

## 28. 模块设计审查后的收敛修改

完整分析写入 `docs/plan/next-stage-module-design-review.md`。本轮按审查结论做了局部调整，未重写模块：

- `LocalTopKDistinct` 改为首块决定模式：typed I64 键走 O(k) 堆路径；其他形状从首块起 pass-through，由全局 Distinct/TopN 负责去重排序。删除完整 `seen_values` 和基于 `Debug` 的本地排序。typed 模式支持 selection gather，rowless chunk 不再因 `visible_indices` 依赖 rows 长度而丢行。
- `build_topn_spec` 对 limit 做 checked conversion，避免 `i64 -> u32` 回绕。
- WAL 回收保留 checkpoint seq 与 safe LSN 两条判据：前者因 `set_checkpoint_seq` 会刷新活动文件头而安全；只使用 LSN 的实测会让 WAL 从 2 个增至 6 个、库体积超过 50 MB。仍保留一个 retired 段作为回退。
- 拆分 checkpoint 与快照保留配置：新增 `snapshot_retention_count` 与 `DatabaseConfig::with_snapshot_retention`，快照策略不再被 checkpoint 恢复点数量隐式约束。
- typed group count 复用 identity 列的非空证明，`count(h)` 不再仅因参数不是标量 typed 列而回退通用表。

devs release 编译通过；local topk 3/3、typed group 5/5、并行分区 27/27、DQL 324/324、checkpoint 保留 2/2、WAL writer 24/24、embedded config 通过、storage lib 全量通过。两轮固定门 `actual=10`：`distinct_limit` 中位数 98.35 / 127.76 ms，`group_by_count` 110.06 / 131.36 ms；结果逐值一致，`spill_rows=0`。本轮数值波动比上一轮更大，后续应结合每分区偏斜指标判断，不据单轮下结论。

库体积约 38 MB，目录为 2 个 checkpoint、0 个快照、3 个 WAL 文件。日志与指标回填 `perf-data/parity/results/2026-11-18/design-review-final/`。

## 29. 剩余验证与修改方向

补充验证已完成，方向文档写入 `docs/plan/next-stage-verification-and-directions.md`：

- `LocalTopKDistinct` 新增重复键、并列、limit 大于局部行数、rowless typed、selection gather、NULL/字符串 pass-through、零 limit 回归测试；typed group count 新增 identity `COUNT` 参数测试。devs 上 6/6 与 6/6 通过。
- 额外执行 4 轮固定门，加上此前 2 轮共 6 轮：`actual=10`、`actual_partitions=10`、`result` 逐值稳定、`spill_rows=0`；`distinct_limit` 中位数范围 81.88–127.76 ms，`group_by_count` 83.45–131.36 ms。波动不能由结果差异解释，先补偏斜和系统负载口径再判断性能。
- 资源收敛约 42.1 MB、2 个 checkpoint、0 个快照、2 个 WAL 数据文件加 `checkpoint.meta`；gate 脚本的 checkpoint 计数已排除 `manifests`。
- 明确修改顺序：计划期类型/nullable 证明与 EXPLAIN 原因 → 可观测性字段 → typed `SUM(_expand_count)` → CBO 分区成本 → 存储 exactness token 与微扫描下推 → 回收失败指标。未证明的下推保持关闭，全局 Distinct/TopN 保留为最终权威。

原始指标：`perf-data/parity/results/2026-11-18/design-review-final/`、`perf-data/parity/results/2026-11-18/design-review-repeat/`。

## 30. 分析项落地与最终回测

按 `docs/plan/next-stage-verification-and-directions.md` 的修改方向完成以下实现：

- 计划期类型守卫：已知非 `BigInt` 去重键不再下发 `LocalTopKDistinct`；EXPLAIN 输出 `local_topk:pushed:typed|runtime`，10 个分区均可读。
- 分区 work time 进入 ProfileBoard/EXPLAIN，示例分区范围约 77.8–106.3 ms，偏斜比约 1.37，可解释跨轮波动。
- `typed_group_count` 复用 identity validity proof，并支持 `SUM(_expand_count)` 定点累加，deopt 按模式生成 `Count` 或 `Sum`。
- `desired_partition_count` 用固定成本与每分区成本按 `saving - overhead` 选择分区数；零成本配置保持原结果。
- checkpoint 计数排除 `manifests` 目录。

重新收集的固定门结果：`distinct_limit` 105.05 / 108.39 ms，`group_by_count` 95.71 / 103.66 ms；`actual=10`、`actual_partitions=10`、结果逐值一致、`spill_rows=0`。库体积约 38.4 MB，2 个 checkpoint、0 个快照、2 个 WAL 数据文件。指标目录：`perf-data/parity/results/2026-11-18/design-review-implemented/`。

仍未开放：存储 exactness token、id-range/order-limit 下推、近似元数据计数、typed 命中与回收失败计数。这些项缺少消费者或精确性证明，不应在未证明前改变结果路径。

## 31. 剩余路径证明与比较器统一

证明设计写入 `docs/plan/remaining-paths-proof-design.md`。已完成并验证：

- `ColumnStatsSnapshot` 增加 `bounds_are_exact` 与 exact-only 访问器；zone-map 生产者一律标记非精确，`absorb` 任一侧非精确即降级。新增反例测试确认 pruning bounds 永不作为结果边界。
- 修复比较器分歧：存储范围谓词和 zone-map 统一到 `Value::cmp`；`ColumnEqual` 改用 `PartialEq`，避免 Float NaN 被当作相等。
- 新增 `predicate_comparator_proof`：11 个代表值 × 5 个操作符 × 全部标量字面量与 `BinaryOperationEvaluator` 交叉对拍，并覆盖 NULL/missing property。
- gate 增加 `PREDICATE_PROOF_EXIT`。

全量验证：storage lib 1409 passed、predicate proof 2 passed、DQL 324 passed、parallel 27 passed；gate 两轮 `actual=10`、`actual_partitions=10`、结果逐值一致、`spill_rows=0`。固定门为 `distinct_limit` 107.70 / 101.88 ms，`group_by_count` 113.90 / 98.77 ms，库体积约 38.5 MB。指标目录：`perf-data/parity/results/2026-11-18/proof-validation/`。

后续修改方向：id-range/order-limit 下推仍关闭，需先提供比较器之外的 snapshot/TTL/staged/索引顺序 token；元数据计数继续不是强一致结果；typed 命中与回收失败计数按证明文档指定的模块暴露。

## 32. order-limit 下推验证

新增 `crates/linkrs-query/tests/order_limit_pushdown_proof.rs`，直接对拍 `OrderedCodec` 字节序与执行器 `compare_values`；gate 增加 `ORDER_LIMIT_PROOF_EXIT`。验证结论：

- 单类型有限样本（Bool、整数、有限浮点、字符串、常规时间日期、Decimal、Uuid）索引序与排序序一致。
- NULL 使用 `encode()` 时排在标量之前，与排序的 NULL-last 不一致；`encode_null_last()` 才一致，但当前索引键未使用。
- NaN、跨物理整数类型、跨年份 999/1000 的 Date 都不满足比较器一致性。
- `key_builder` 新增测试确认同值索引键按 entity id 排序；执行器稳定排序按输入顺序处理并列，LIMIT 不能交给索引切分。
- TTL/staged write 与 offset 语义仍无 token。

因此 order-limit 仍保持关闭。后续必须先提供包含类型、nullable/编码、NaN、日期范围、tie-break、TTL/staged-write 的 `IndexOrderProof`，并在 EXPLAIN 记录 pushed/unpushed；全局 TopN 继续作为最终权威。

验证：order proof 5 passed、key_builder 5 passed、predicate proof 2 passed、storage lib 1409 passed、DQL 324 passed、parallel 27 passed。固定门 `distinct_limit` 111.84 / 94.31 ms，`group_by_count` 105.83 / 100.98 ms，`actual=10`、结果逐值一致、`spill_rows=0`，库体积约 38.5 MB。指标目录：`perf-data/parity/results/2026-11-18/order-limit-proof/`。

## 33. 最终设计收敛与证明 token 落地

完整分析与最终设计写入 `docs/plan/remaining-paths-final-design.md`。本轮新增：

- `IndexOrderProof`：唯一、单列、非空、无 partial index、codec 兼容标量类型才生成证明；TopN 规则要求单列排序键、方向一致、无 residual filter、正 limit；Limit 规则拒绝溢出、负 offset/count、residual filter 与已有 order_by；proof reason 进入 EXPLAIN。
- 重要发现：物理转换从未把逻辑 IndexScan 的 limit/order_by 传入 `IndexScanPlan`，当前没有执行期 order-limit 下推；cursor 跨 shard 顺序串联也没有全局有序证明。因此本轮只收敛逻辑改写，不开放物理截断。
- `ColumnarStats` 增加 typed group hit/fallback 计数并进入 columnar summary。
- `PersistenceCoordinator` 增加 `reclaim_failures`，checkpoint retention 失败时自增，`PersistenceDiagnostics` 暴露。

验证：query lib 2313 passed、storage lib 1409 passed、gate 全绿；固定门 `distinct_limit` 101.13 / 98.25 ms，`group_by_count` 100.49 / 92.46 ms，`actual=10`、结果逐值一致、`spill_rows=0`，库体积约 38.6 MB。指标目录：`perf-data/parity/results/2026-11-18/final-design/`。

后续仅在完成 cursor 跨 shard 全局有序证明、物理 limit/offset 接线、pushed/unpushed 对拍后才开放 order-limit；bench JSON 接入 `reclaim_failures` 需要先补 embedded API accessor。

## 34. 代码实现缺陷单独修复

新增独立分析文档 `docs/plan/current-code-implementation-defects.md`，把实现缺陷与改进项立项判断分开。本轮修复：

- 打通 IndexScan 的 limit/offset 物理链路：SourceSpec、SourceOperatorKind、IndexScanPlan 均消费 pushed 值；
- 修正 Limit 规则：offset 与 emit count 分离，拒绝 UNBOUNDED、负数、零 count 与溢出；
- `ChainForwardIterator` 改为跨 generation 的 k 路归并，保证全局 key 顺序，同时保留 newest 优先的 MVCC 语义；
- TopN 规则在写入 limit 时清零 offset 并要求 IndexOrderProof；
- 新增跨 generation 有序、newest 优先、cursor offset/limit 与规则守卫测试。

实现缺陷修复不改变改进项立项结论。验证：query lib 2313 passed、storage lib 1412 passed、gate 全绿；固定门 `distinct_limit` 93.22 / 93.56 ms，`group_by_count` 99.41 / 98.79 ms，`actual=10`、结果逐值一致、`spill_rows=0`，库体积约 38.7 MB。指标目录：`perf-data/parity/results/2026-11-18/implementation-defects/`。

## 35. 当前状态核对（2026-11-25，资源对齐后）

依据 `perf-data/parity/results/2026-11-25/resource-aligned/`：Linkrs 两轮固定门与生产默认日志、逐用例 JSON、EXPLAIN，以及同绑定同 buffer 的 Ladybug/Neug JSON。口径为三方 node1 绑定、16 线程参数、warmup 1 + run 5 去最大值平均；Ladybug 显式 64 MB buffer，Neug 全内存且 AP 单线程 wall-clock，只作参照。第 1 节与本节为当前权威数据；2026-11-23 及更早的三方倍率作废。

| 原阶段 | 原始问题 | 当前生产默认 | 当前固定门两轮 | 对手（Ladybug / Neug） | 当前判定 |
|---|---|---:|---:|---|---|
| 阶段一 | `group_by_count` 1.31–1.50 秒，`distinct_limit` 约 1.11 秒 | 25.92 / 27.28 ms | 32.05 / 32.53 ms；29.98 / 29.96 ms | 8.16 / 3.20 ms；12.28 / 28.33 ms | 秒级瓶颈消除；对 Ladybug 3.18× / 8.52×，未闭环 |
| 阶段二 | `join_two_hop` 35–49 ms，`variable_path_count` 1.10–1.35 秒 | 5.38 / 5.38 ms | 5.94 / 5.93 ms；5.60 / 5.90 ms | 1.54 / 441.42 ms；20.14 / 102.90 ms | 变长计数大幅领先；两跳对 Ladybug 3.50×，快于 Neug |
| 阶段三 | `aggregate_filtered_count` 2.67 ms，微扫描秒级隐患 | 1.22 / 1.69 / 1.53 / 1.21 ms | 同量级 | 0.95 / 1.24 / 1.21 / 0.46 ms | 过滤聚合已闭环；微扫描对 Ladybug 1.27–2.85× |

逐用例证据与判定：

1. `EXPLAIN ANALYZE` 中 `group_by_count` 的 `typed_group_hits=112`（生产默认）/`110`（固定门），`distinct_limit` 的 8/10 个分区均显示局部 TopK 生效；两条用例 `expand_output_rows` 为 30536 / 30473，不是旧的逐边 215488 行。结果仍为 `[14|59]` / `[14]`，`result_stable=true`、`actual=8/10`、`spill_rows=0`。
2. 生产默认自耗时 `scan_read_us` 约 41–43 ms、`expand_read_us` 约 77–81 ms，其中拓扑读取约 65–69 ms、目的端读取约 12 ms。阶段一剩余瓶颈已转为分区扫描与单跳拓扑区间读取，而不是聚合、TopK、Exchange 或配置。
3. 分区点 10 仍是固定门的经济点；但在 node1 的 8 个物理核上，16 workers / 10 partitions 不再稳定优于生产默认 8/8，两条阶段一用例生产默认更快。默认值不改，固定门只作为上限口径。
4. 生产配置为 `enabled=false`、`workers=8`、`max_partitions=8`、`allow_ordered_parallel_expand=true`；benchmark 固定门显式传 16/10 只为验证并行计划，不代表生产默认。旧的“workers=1、配置未受益”描述已过期。
5. 编译中位数在两条阶段一用例约 1.5–2.5 ms，纯计数仍为 0.01 ms 级；对 ms 级短查询不利，但不改变本轮结果判定。
6. `filter_count_pushed` 与 `filter_count_unpushed` 仍约 1.0–1.2 ms，完整谓词替换继续因 TTL、跨类型和 NULL 语义保持关闭，残余 `Filter` 保留。
7. 资源口径已对齐：三方 CPU 亲和性均为 `8-15,24-31`，Ladybug buffer 显式 64 MB 与 Linkrs 相同；Neug `M_FULL` 无法等价，只作参照。Linkrs RSS 约 323–326 MB、DB 约 46 MB；旧“Ladybug 90% 内存 buffer”描述属于上游 runner，不适用于本批 JSON。
8. 参考源码差距仍在读取形态：Ladybug 的 packed child slices/count extend 与 Neug 的 POD CSR 迭代比当前区间读取更少对象与元数据访问；但 packed 直写、身份直通、typed 聚合与精确局部 TopK 已落地，预取候选无 ≥15% 证明，不进入生产。

当前结论：阶段二和阶段三的过滤聚合部分可以视为已解决；阶段一需要在不改变精确语义的前提下继续降低单次拓扑区间读取与分区扫描成本。生产配置保持当前默认值，任何变更都需要同一资源口径的 A/B 与容量证据。后续方案见 `docs/plan/stage1-stage2-current-gap-remediation.md`。

## 36. 后续方案入口

`docs/plan/stage1-stage2-current-gap-remediation.md` 只针对阶段一、二给出当前实施顺序、落点、生产配置更新、护栏与验收，不重新定义本清单的问题。`docs/plan/remaining-bottleneck-microbench-and-design.md` 记录首轮回测后的剩余瓶颈归因、Rust 微基准与后续改进设计；`docs/plan/scan-visibility-and-decode-remediation.md` 给出扫描可见性枚举与属性解码的验证和改动顺序。阶段三的微扫描下推、阶段四至七的资源/分区/图算法/存储深水项仍按原方案和后续回填执行；如无新的三方证据，不因本轮阶段一、二调整而扩大放行范围。

## 37. 阶段一、二收敛方案首轮回测（2026-11-23）

按 `docs/plan/stage1-stage2-current-gap-remediation.md` 完成首轮代码与配置修改，devs release 构建通过。改动包括：扫描读取与展开读取自耗时进入 `ColumnarStats` / EXPLAIN / parity JSON；拓扑批量读取改为父批 + 孩子偏移的 packed 形态，rowless 单跳展开直接消费；`typed_group_count` 用一次 `entry` 累加替代预插入后的二次哈希；`RETURN DISTINCT` 与 `YIELD DISTINCT` 在计划期传播投影列类型；固定深度一加二的前沿度数合并为一次批量读取；生产默认放行走列展开灰度形状。

验证结果：配置默认值 1/1、partitioning lib 49/49、typed group 8/8、local topk 10/10、`parallel_partition_execution` 27/27、DQL 324/324、storage lib 1415/1415 全绿。`linkrs-config` 的 lib 单测因 devs 离线缺少 `serial_test v3.5.0` 无法启动，因此 gate 改用 partitioning lib 中的默认值断言覆盖同一配置契约。

计划期类型传播已接入 `RETURN/YIELD DISTINCT` 的逻辑与物理 Dedup 节点，但当前 LSQB 形状的 EXPLAIN 仍为 `local_topk:pushed:runtime`；运行时 typed 判定继续生效，不能把计划期证明记为已完成。

固定门两轮（warmup 1 + run 5，绑核 16-31，`--parallel-workers 16 --max-partitions 10`，`actual=10`、`actual_partitions=10`、结果逐值一致、`spill_rows=0`）：

| 用例 | 上轮两轮中位数 ms | 本轮两轮中位数 ms | 判定 |
|---|---:|---:|---|
| `group_by_count` | 103 / 119 | 96.01 / 94.66 | 无明显退化，未达 ≤40 ms 中间门 |
| `distinct_limit` | 89 / 137 | 91.90 / 81.91 | 无明显退化，未达 ≤30 ms 中间门 |
| `join_two_hop` | 5.94 / 6.19 | 6.24 / 10.31 | 无回归，未达 ≤4 ms 中间门 |
| `variable_path_count` | 6.54 / 6.34 | 6.09 / 6.36 | 无回归，满足 ≤6 ms 量级 |
| 九条纯计数 | 0.14–0.61 | 0.15–0.66 | 无实质回归 |

生产默认配置单轮（不加 benchmark 并行参数）中 `group_by_count` 99.64 ms、`distinct_limit` 85.41 ms，EXPLAIN 为 `requested=8, actual=8`，九条纯计数仍为 0.15–0.66 ms。这说明生产默认从串行切换到窄形状灰度后确实获得收益；上轮量级约 280–290 ms 的默认执行不再是当前行为。

新增自耗时字段给出了下一步方向：`group_by_count` 两轮 `scan_read_us=519581/423718`、`expand_read_us=168737/160310`；`distinct_limit` 为 `548506/552796` 与 `185000/183901`。墙钟约 90–100 ms 是 10 个分区的聚合，扫描读取与展开读取远超其余算子。packed 拓扑和 typed 状态调整没有在端到端上形成可测收益，但取消了查询层逐边对象路径并保持结果一致；`1..2` 前沿合并也未在 sf0.1 上体现收益。下一轮应直接优化扫描枚举/可见性与目的端属性读取，而不是继续改聚合、去重或拓扑 DTO。

编译中位数仍为约 2.1–3.1 ms（九条纯计数 0.01 ms 级），未达本方案中间门；其来源尚未收敛，不能把当前并行形状当作短查询无成本。原始日志、逐用例 JSON、EXPLAIN 与 gate 日志在 `perf-data/parity/results/2026-11-23/stage1-stage2-final/`。

结论：生产配置更新已生效且正确性门全绿；阶段一、二的性能中间门未达成，按方案不扩大放行面、不改近似语义，研究重点转入扫描与目的端读取。

随后按 `docs/plan/remaining-bottleneck-microbench-and-design.md` 完成第二轮修改：packed 拓扑改为事件流 + 计数散射 + 紧凑边类型 ID，目的端 flat 需求改用属性列/存在性批量读取，两跳度数去掉每父容器。微基准中 packed 读取由 3985 us / 15.5 MB 降到 3280 us / 8.68 MB，目的端 `id` 读取由 1361 us 降到 1090 us；固定门中 `expand_read_us` 由约 160–210 ms 降到 110–135 ms，`join_two_hop` 为 5.78 / 5.94 ms，`variable_path_count` 为 5.77 / 5.45 ms。端到端 `group_by_count` 88.25 / 98.20 ms、`distinct_limit` 84.29 / 135.52 ms，仍未稳定跨越中间门；剩余主因是扫描自身的可见性枚举与属性解码。原计划中的 vid 哈希交换策略未改变目标查询的 EXPLAIN，已回退，不进入最终改动集合。完整结果见 `perf-data/parity/results/2026-11-23/stage1-stage2-remediation-final/`。

## 38. 扫描可见性枚举与属性解码归因回测（2026-11-23）

按 `docs/plan/scan-visibility-and-decode-remediation.md` 在 devs 完成三轮固定门与微基准：仅计时基线、保留切片一与枚举单锁优化的最终版，以及额外试过切片二后回退的中间版。子计时先进入 `ColumnarStats` / EXPLAIN / parity JSON，五个字段之和占 `scan_read_us` 的 96%–98%，归因成立。基线两轮中 `scan_enumerate_us` 为 262–463 ms，`scan_visibility_us` 为 102–262 ms；目标查询的 `scan_decode_us` 为 0，说明阶段一固定门走拓扑-only 扫描，属性解码不是这两条用例的端到端主因。

最终采纳改动：游标为已枚举 ID 增加来源标记，源自在同 guard 下枚举的 ID 跳过 sharded 表内的二次行可见性复核；点读与 allowlist 路径继续复核。分片切片改为在单个身份锁下枚举连续 ID 区间，消除逐行锁获取。切片二（列按 chunk 批量解码、去掉候选预分配后 select）在中间版实测中仅把一列投影扫描从 651 us 降到 611 us，可见性+解码从 372 us 降到 304 us，分配却从 355 B/行升到 511 B/行；按“无实测收益不叠加复杂度”回退，不进入最终代码。

固定门两轮仍满足 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`。最终版 `group_by_count` 为 53.34 / 58.04 ms（基线 80.30 / 102.96），`distinct_limit` 为 47.67 / 39.84 ms（基线 69.35 / 96.84）；对应 `scan_read_us` 从 492–700 ms 降到 87–120 ms，降幅约 82%。生产默认单轮为 55.53 / 38.38 ms（基线 80.08 / 73.56），`actual=8`；九条纯计数仍在 0.15–0.68 ms，无实质回归。微基准中分片拓扑-only 从 298 us 降到 277 us（较基线快约 7%），可见性复核计时从 91 us 降到 62 us；一列投影扫描未达 30% 门槛，属性解码相关改动已全部回退。

正确性门全绿：配置默认值 1/1、partitioning 49/49、typed group 8/8、local topk 10/10、`parallel_partition_execution` 27/27、DQL 324/324、storage lib 1415/1415，新增 `column_batch_test` 13/13（含“枚举后、解码前提交”的快照回归）。`linkrs-config` lib 单测仍因 devs 离线缺少 `serial_test v3.5.0` 未启动，配置默认值继续由 partitioning 断言覆盖。

判定：扫描自耗时与端到端中位数均显著下降，但 `group_by_count` ≤40 ms、`distinct_limit` ≤30 ms 的最终门尚未同时达成；剩余主因已转为展开读取与固定门抖动（`expand_read_us` 在两轮间为 160–230 ms）。不扩大并行放行面、不引入近似，也不保留未达标的解码改动。下一阶段以展开读取拆分为主线，设计见 `docs/plan/expand-read-and-gate-convergence-design.md`。原始日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-23/scan-visibility-decode-remediation/`。

## 39. 展开读取拆分与门收敛回测（2026-11-23）

按 `docs/plan/expand-read-and-gate-convergence-design.md` 在 devs 完成仅计时基线、排序去重实验版与最终版三轮固定门。子计时与计数进入 `ColumnarStats`、EXPLAIN、parity JSON；五字段与事件计数解释了 `expand_read_us` 的量级与波动：基线两轮中 `expand_topology_read_us` 为 88.57–120.54 ms（约 59%–67%）、`expand_destination_read_us` 为 41.62–60.42 ms、`expand_view_us` 为 15.99–17.04 ms、`expand_output_us` 为 12.29–13.39 ms，`expand_tag_resolve_us` 低于 0.1 ms。两条用例均为 110 个父批、215488 个事件/输出行、30473 个唯一目的端，缺失目的端为 0；拓扑读取确定为主导项。

采纳的改动有三项：目的端标签解析在单次执行的 `ExpandCtx` 内缓存；`read_destinations` 的计时拆成存储读取与视图装配；`packed_node_edge_headers_batch` 的 Out/In 分支按父直接写入紧凑数组，取消 `Event` 中转、计数数组与散射游标，端点翻译仍只在本次调用内复用。排序聚合替代 `HashMap` 的实验版使固定门 `expand_view_us` 从 16–17 ms 升到 26–28 ms、F3=10% 唯一率的视图装配从 116 us 升到 217 us，`expand_read_us` 没有下降；按“视图装配无收益则回退”恢复原有逐边哈希去重。目的端身份绕行未实现：目的端读取不是主导项，且当前计划没有主键身份证明，不为缩短代码路径引入按列名猜测。父批大小也未改默认值。

F1–F5 微基准在同一 2 万顶点数据集上记录。与设计文档基线 3395 us / 8.68 MB（2048 父）相比，最终 F1 为 2927 us / 6.55 MB，时间下降 14%、分配下降约 25%，未达 25% 时间门槛；512 / 4096 父分别为 757 us / 1.64 MB 与 5844 us / 13.10 MB。F2 中 Out/In 为 7.57 / 7.99 ms，Both 仍为 36.63 ms 并保留去重事件路径；F3 的 100% / 50% / 10% 唯一率视图装配为 337 / 223 / 121 us，F4 的存在性、单列、全实体为 622 / 1133 / 1485 us，F5 连续调用为 4076 us / 7.96 MB。

最终固定门两轮 `group_by_count` 为 49.76 / 45.66 ms（基线 48.55 / 47.12），`distinct_limit` 为 38.85 / 44.39 ms（基线 42.71 / 42.04）；两轮中位数分别为 47.71 / 41.62 ms，基线为 47.83 / 42.38 ms。`expand_read_us` 两轮中位数分别由 164.92 降到 160.65 ms（-2.6%）、由 177.16 降到 171.96 ms（-2.9%），远未达 30% 门槛；`group_by_count ≤40 ms`、`distinct_limit ≤30 ms` 仍未通过。`actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0` 保持成立。生产默认单轮 `group_by_count` 为 48.36 ms（基线 47.37），`distinct_limit` 为 42.03 ms（基线 38.02）属轮间波动；九条纯计数与 q1–q9 无实质回归。

正确性门全绿：配置默认值 1/1、partitioning 49/49、typed group 8/8、local topk 10/10、`parallel_partition_execution` 27/27、DQL 324/324、storage lib 1415/1415、`column_batch_test` 13/13。判定：保留可验证收益的计时、标签解析复用与父批直写，最终门未达成；剩余成本集中在每批父端点的重复解析与可见性/分区调度抖动，不再叠加视图去重或身份绕行复杂度。原始日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-23/expand-read-convergence/`。

## 40. 父端点解析复用与分区偏斜复测（2026-11-23）

按 `docs/plan/parent-endpoint-and-partition-jitter-design.md` 完成第二轮收敛。`identity_seed_ids` 现在直接读取 `VertexHandleColumn.ids`，rowless 与 row 两种种子入口都不再逐值把父端点转成 `Value::VertexId` 再解析；`Expand` 与 `ExpandAll` 的上下文都按算子生命周期持有缓存。新增只读 `TopologySchema`：space、vid 类型、布局版本与各边类型的标签/ID 在查询开始时解析一次，packed 批量读取直接消费该 DTO，布局版本不匹配才回退到存储层本地解析。parity JSON 与 gate 表格新增 `parallel_wall_us`、`parallel_work_us`、`partition_skew_ratio`，用于区分总工作增加与尾部分区变慢。

devs release 编译通过，正确性门全绿。F1（2048 父）由上一轮 2927 us / 6.55 MB 降到 2722 us / 6.55 MB，约 7%；相对最初 3395 us / 8.68 MB 下降约 20% / 25%。F2 Out/In、F3 唯一率与 F4 读取形状无语义变化。

固定门共记录两次完整 gate 与一轮三次直接回测，但 devs 外部负载期间 load average 为 5–12，`scan_read_us` 在 61–257 ms 间波动，`partition_skew_ratio` 在 1.14–1.49 间波动。`expand_read_us` 为 150–197 ms，与上一轮 160–215 ms 区间重叠，未取得设定的 10% 下降；`group_by_count ≤40 ms`、`distinct_limit ≤30 ms` 仍未同时通过。结果逐值一致、`actual=10`、`spill_rows=0`，生产默认单轮无稳定退化。判定：保留有微基准收益且无语义变化的解析复用与观测字段；偏斜主要受外部负载放大，暂不调整分区数、worker 数或通道容量。日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-23/endpoint-skew-convergence/`。

## 41. 父端点身份批量解析（2026-11-24）

按 `docs/plan/parent-identity-batch-resolution-design.md` 实施三项收敛：`IdIndexer` 增加一次持锁的批量反查；分片拓扑-only 扫描在枚举父端点时同步解析外部 ID，不再逐行加锁 clone；packed 拓扑读取把唯一目的端按 label 合并后调用一次 `external_ids_by_internal_batch`，后者也改为批量反查。TTL、staged、allowlist、row 路径与悬空端点语义保持原状。

devs release 编译通过，正确性门全绿。固定门两轮 `group_by_count` 为 43.71 / 42.36 ms，`distinct_limit` 为 37.93 / 37.82 ms，结果逐值一致、`actual=10`、`spill_rows=0`，`partition_skew_ratio` 为 1.18–1.33。扫描收益明确：`scan_read_us` 由上一轮 90–257 ms 降到 40–45 ms，`scan_visibility_us` 由 67–234 ms 降到 15–19 ms；`scan_enumerate_us` 升到 23–27 ms，说明原可见性计时的主体是逐行身份解码。微基准 F2 Out 从 7161 us 降到 5840 us，F3/F4 小幅改善，F1 从 2722 us 升到 3176 us，属分配与噪声混合结果。

`expand_read_us` 仍为 172–193 ms，已经是当前唯一主导项，两条固定门仍未同时通过。判定：保留批量身份解析；下一步转向 packed expand 与局部聚合/TopK 直连，不再在身份解析上叠加改动。原始日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-24/parent-identity-batch/`。

## 42. 拓扑扁平化与目的端 handle 复用（2026-11-24）

按分析依次实施两项。`out_topology_batch()` / `in_topology_batch()` 改为扁平 `entries + offsets`，先收集带父编号的 event，再按 prefix-sum scatter，消除每个父点一个 `Vec` 的分配形状；目标查询每父点仅一条边，约减少 215,488 次小堆分配。`PackedEdgeTopology` 增加 child-aligned 目的端 handle，`read_destinations()` 通过新增的 `get_vertex_property_columns_with_handles_batch()` 复用该 handle，跳过 external→internal 反查；目的端视图改为 `values + slots` 索引结构，不再逐边 clone `Vertex`。另外验证了 stable-sort 扁平化变体，虽然分配更低，但 F2 Out 从 6659 us 升到 11243 us、固定门也变差，已撤回。

devs release 编译通过，正确性门全绿。固定门两轮 `group_by_count` 为 38.80 / 38.73 ms，`distinct_limit` 为 31.14 / 32.61 ms，逐值一致、`actual=10`、`spill_rows=0`。`expand_read_us` 从 172–193 ms 降到 124–161 ms，其中 topology 从 111–130 ms 降到 79–101 ms，destination 从 39–47 ms 降到 28–44 ms，view 从 18–20 ms 降到约 16 ms。`group_by_count ≤40 ms` 已达成；`distinct_limit ≤30 ms` 仍差 1–3 ms，三次直接回测为 32.88 / 34.20 / 33.41 ms，且 devs 负载仍为 5–12。

判定：保留两项改动。packed expand 与局部聚合/TopK 直连涉及计划形状和 Exchange 契约，本轮不与身份收敛混合；在安静窗口确认 `distinct_limit` 仍不达标后再单独实施。原始日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-24/expand-topology-flatten/`。

## 43. Packed expand 与局部阻塞算子直连（2026-11-24）

按 `docs/plan/packed-expand-local-sink-design.md` 实施直连。`DataChunk` 增加 `PackedExpandPayload`，计划层仅在单键 `COUNT` 和既有单列 `Dedup + TopN` 窄形状上设置下游键名；展开在过滤悬空目的端后直接输出投影后的键值序列，`Project` 仅替换 layout 并透传，`PartialAggregate` / `LocalTopKDistinct` 直接消费该序列，仍按原契约输出 row batch，Exchange 不变。非定长或不支持的键类型回退为普通 row 路径。首轮编译缺少 trait import、第二轮 payload 未匹配 `p.id` 与 `id` / identity-only 两种来源，均已修正。

devs release 编译通过，正确性门全绿；最终轮 EXPLAIN 显示 `expand_output_us=0`、`typed_group_hits=110`、`local_topk:pushed`、`actual=10`、`result_stable=true`、`spill_rows=0`。固定门两轮 `group_by_count` 为 40.17 / 40.58 ms，`distinct_limit` 为 34.84 / 35.30 ms；生产默认单轮为 41.06 / 30.73 ms。devs 负载在复测期间达到 9–10，visibility 微基准与拓扑自耗时均显著放大，不能把端到端波动归因于直连本身。

判定：保留直连，因为可测的展开输出自耗时被消除且正确性稳定；但本轮未在噪声窗口内证明两条固定门同时达标。`group_by_count` 的 Exchange 前局部 TopK 需要保留 count payload 的新算子，单独验证后再决定。结果与编译/回退记录见 `perf-data/parity/results/2026-11-24/packed-expand-local-sink/`。

## 44. group_by_count 的 Exchange 前局部 TopK（2026-11-24）

按 `docs/plan/group-by-count-local-topk-design.md` 实施 `LocalTopKRows`。当 split aggregate 是单键 `COUNT`、紧随的全局 `TopN` 按同一分组键排序时，在每个分区的 `PartialAggregate` 之后插入局部行 TopK：按 key 去重、保留最好 k 行、使用与全局排序一致的 NULL 顺序，输出完整 `[key, count]` row batch。全局 `FinalAggregate`、`TopN`、`Limit` 与 Exchange row 契约不变。首轮编译暴露返回引用生命期和调用参数顺序错误，均已直接修正。

devs release 编译通过，正确性门全绿。EXPLAIN 显示每分区 `LocalTopKRows` 输出 10 行，Exchange 从约 11,554 行降到 100 行，`actual=10`、`result_stable=true`、`spill_rows=0`。固定门两轮 `group_by_count` 为 39.59 / 33.89 ms，`distinct_limit` 为 37.95 / 32.80 ms；生产默认单轮为 31.86 / 29.51 ms。结果逐值一致，无额外近似或配置放开。

判定：保留 `LocalTopKRows`。原始日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-24/group-by-local-topk/`。

## 45. packed 拓扑直写与身份键直通回测（2026-11-24）

按 `docs/plan/packed-topology-direct-write-and-key-passthrough.md` 依次实施三项改动。`out_topology_batch` / `in_topology_batch` 改为按父直接写 `entries + offsets`，调用方复用一个 `HotNbr` 缓冲，删除 event 收集与散射；`visit_out_nbrs_batched` / `visit_in_nbrs_batched` 同样按父遍历，`frontier_degree_batch` 在邻居回调里直接写父连续 endpoint staging 并累计度数，删除 `NeighborEvent` 与散射游标。计划期身份证明落在现有分区链构建处：只有单一边类型、单跳、无过滤/path 语义、分组键映射回目的端唯一被需求的属性、且该属性是目的端标签声明的第一个属性（本存储的主键、等于外部身份）并属于整数类型时才放行；不按列名猜测，`local_topk:pushed:runtime` 不作为证明。身份成立时 packed 展开直接把目的端 `VertexId` 转成声明类型的键值，目的端读取退化为存在性检查；同一唯一键的若干边折叠为 `PackedExpandPayload` 的权重，`PartialAggregate` 一次累加，`LocalTopKDistinct` 只取唯一键。非身份形状保持原属性列读取与逐边路径。

devs release 编译通过，正确性门全绿：配置默认值 1/1、partitioning 49/49、typed group 8/8、local topk 10/10、`parallel_partition_execution` 27/27、DQL 324/324、storage lib 1415/1415、`column_batch_test` 13/13。结果逐值一致，`group_by_count` 仍为 `[14|59]`、`distinct_limit` 仍为 `[14]`，两轮固定门均 `actual=10`、`actual_partitions=10`、`result_stable=true`、`spill_rows=0`，生产默认 `actual=8`。EXPLAIN 中 `expand_output_rows` 由 215488 降到 30473，`expand_output_us=0`，`typed_group_hits=110`；全局 `Distinct` / `TopN` 与 Exchange 契约未变。

固定门两轮 `group_by_count` 为 35.08 / 27.78 ms（中位 31.43 ms，基线 39.59 / 33.89），`distinct_limit` 为 30.97 / 28.06 ms（中位 29.51 ms，基线 37.95 / 32.80）；两轮都下降，但都未同时满足“两轮均 ≤30 ms”的中间门，第一轮 `distinct_limit` 仅超 0.97 ms。生产默认单轮为 27.59 / 25.10 ms（基线 31.86 / 29.51），九条纯计数仍在 0.15–0.66 ms。`join_two_hop` 为 9.90 / 5.52 ms（第一轮高值未复现，第二轮与生产默认 5.57 ms 接近基线），`variable_path_count` 为 5.15 / 5.55 ms，结果分别为 `382018` / `400153` 且逐值不变。`expand_read_us` 固定门两轮由 131–197 ms 降到 101–109 ms，生产默认降到 73–77 ms；`expand_topology_read_us` 仍为 61–95 ms，是展开内部主导项。

同数据集微基准相对本切片前基线：F1（2048 父）3821 us / 11.65 MB 降到 3042 us / 6.32 MB（时间 -20%、分配 -46%），F2 Out 6638 us / 15.37 MB 降到 5863 us / 10.05 MB（-12% / -35%），F5 5182 us / 13.06 MB 降到 4149 us / 7.74 MB（-20% / -41%）。时间降幅没有达到切片一设定的 25%，但分配与端到端均有一致收益，故保留直写；不把它描述为已跨过门槛。

条件性 CSR 预取与微批排序没有进入本轮：前三项改动后拓扑读取仍是主导项，但本轮没有该预取变体的 F1/F5 ≥15% 与固定门 ≥10% 的 A/B 证据，按方案不引入无证明的局部性复杂度。判定：保留三项有可测收益且结果稳定的改动；`group_by_count ≤30 ms`、`distinct_limit` 两轮均 ≤30 ms 与 `join_two_hop ≤4 ms` 仍未达成，不用放宽并行、近似或截断换取门槛。日志、逐用例 JSON、EXPLAIN 与微基准见 `perf-data/parity/results/2026-11-24/packed-direct-write-key-passthrough/`。

## 46. 条件性 CSR 预取的证明方法与语义复核（2026-11-24）

上一节没有采纳 CSR 预取，因为缺少证明。本节补齐证明口径与验证代码：预取只能作为只读缓存提示，先在同一二进制的微基准里做 A/B，再决定是否进入生产路径。性能门沿用 F1/F5 至少 15% 时间下降且分配不增加、固定门端到端至少 10% 下降；语义门要求提示前后 `PackedEdgeTopology` 的 `parents`、`child_offsets`、`src`、`dst`、`edge_type_ids`、共享类型名、`rankings` 与 `destination_handles` 逐字段相等，CSR 的 `dump()` 不变，且 `visit_hot` 顺序不变。微批排序若被引入，还必须用原槽位映射恢复父顺序与行顺序；本轮没有引入排序，因此不存在重排风险。

验证代码落在原模块：`MutableCsrTrait::prefetch_head` 默认无操作，`MutableCsr` 用 `black_box` 读 offset/degree 两列预热头部，`MappedFrozen` 预热行长度与偏移头；`CsrVariant` / `CsrShardSet` 只做分派，`EdgeStore` 增加带 leg 存在性判断的包装。首版 benchmark 通过只读 `StorageReader::prefetch_topology_heads` 在 packed 读取之前单独发提示，但父端点被解析两次、边表锁被获取两次，分配也随之增加；该 A/B 测到的是“重复解析 + 提示”，不是提示本身，因此已撤掉该 API，改为在 `out_topology_batch` / `in_topology_batch` 内复用本次扫描已解析的端点与已持有的表锁，以 `prefetch_ahead` 参数实现 A/B。生产入口仍传 0。`topology_read_bench.rs` 新增 F1 512/2048/4096 的密集与分散父批、F5 的“packed + head hint”对照，并逐字段断言预取前后 packed 输出相等；`csr_variant/tests.rs` 新增 `prefetch_head_is_read_only`，覆盖多行、多边的 `visit_hot` 序列与 `dump()` 不变。

首版外置提示的退化（F1 2048 为 3026→3304 us、F5 为 4356→4631 us，分配也上升）主要来自重复端点解析与重复表锁，不能归因于预取。改为扫描内复用后分配完全相同，但结果仍不支持采纳：F1 密集 2048 为 3248→3295 us（+1.4%，噪声）、F5 为 4303→4276 us（-0.6%，噪声），而 F1 分散 2048 为 8496→10169 us（+19.7%）。原因是当前提示用 `black_box` 阻塞读两列头部，分散父批上每个提示都是真实 load，且与紧随其后的行读取没有形成有效重叠；标签判断与 lookahead 循环也增加了指令数。这只能否定当前黑盒读候选，不能否定真正的非阻塞预取指令，但后者尚未实现，也没有 ≥15% 证据。判定：拒绝该候选，生产拓扑读取路径保持上一节的按父直写；验证 API 与基准保留，用于后续有真实预取指令或局部性证据时重测。构建、storage 原有 1415/1415 与新增 `prefetch_head_is_read_only` 1/1、微基准均通过。

随后实现了 x86_64 非阻塞 `_mm_prefetch(_MM_HINT_T0)` 候选，只提示 offset/degree 两个头部 cache line，其他目标空实现；unsafe 边界与语义在 `docs/archive/unsafe.md` 中记录，生产入口仍为 0。`topology_read_bench.rs` 对 lookahead 4/8/16/32 做扫描，逐字段 packed 输出断言与 `prefetch_head_is_read_only` 均通过。devs 结果显示没有可采纳收益：F1 密集 2048 基线 2983 us，四个距离分别为 3018/3020/3016/3017 us；F1 分散 2048 基线 5698 us，分别为 5701/5702/5704/5783 us；F5 基线 4222 us，hint=4/8/16/32 为 4170/4477/4520/4204 us，唯一轻微正向也在噪声范围内，远未达到 15% 门。分配量与基线完全一致。按预设门槛，已删除 unsafe intrinsic，把 `prefetch_head` 恢复为有文档的空实现，仅保留 A/B 钩子与验证；不需要修改生产读取入口，也不存在因预取引入的语义变更。

当前保留实现仍有三项可量化但已被收益覆盖的额外开销：`PackedExpandPayload` 每键多带一个 `u64` 权重（Distinct 路径不用，约 244 KB/批），身份快路径仍需一次按目的端 ID 的哈希去重与一次存在性读取（存在性结果仍为 `Vec<Option<Vec<Value>>>`，空投影产生空 Vec），按父直写与父优先邻居遍历会在整批父点上持有边表读锁。三者分别换来一次累加、展开输出从 215488 行降到 30473 行，以及取消 event/scatter；固定门与微基准均显示净收益，且没有结果漂移，因此不回退第 45 节的三项改动。两跳的 `join_two_hop` 9.90 ms 是单轮高值，第二轮 5.52 ms、生产默认 5.57 ms 与基线同量级，也不构成回退依据。

语义复核另发现并修正一处真实问题：身份直通在运行时 `resolved_dst_tag()` 返回 `None` 时仍可能回退到计划期 `dst_tag`，若目录在计划与执行之间漂移，目的端存在性检查会落在不同标签上，身份证明随之失效。现在只有在运行时标签解析成功且等于计划标签、并且计划标签非空时才允许直通，否则继续走原属性列读取。`linkrs-query` partitioning 49/49 通过；修正后固定门单轮 `group_by_count` 29.87 ms、`distinct_limit` 26.46 ms，结果仍为 `[14|59]` / `[14]`，`result_stable=true`、`actual=10`、`spill_rows=0`，EXPLAIN 仍显示 `expand_output_rows=30473` 且身份直通未被关闭。结论：预取候选本身无语义问题但未证明有收益；当前保留的身份直通与直写路径在补齐标签守卫后未发现语义漂移。日志、EXPLAIN 与逐用例 JSON 见 `perf-data/parity/results/2026-11-24/csr-prefetch-proof/`。

## 47. 三方差距收敛实施（2026-11-24）

完整设计与门槛写入 `docs/plan/parity-remaining-gap-convergence-design.md`。按任务顺序处理：

1. `LocalTopKDistinctState::insert_key` 把 ASC 的 `key >= worst`、DESC 的 `key <= worst` 边界判断前移到 `seen_keys` 查询之前， rejected key 不再做哈希查找或状态修改；`limit=0` 仍直接拒绝。`PackedExpandPayload.weights` 在全为 1 的批上省略分配，空 `weights` 统一解释为每键权重 1；显式非 1 权重仍逐键写入。
2. 非主键整数属性直通按计划实现后，发现当前 LSQB 四个目的端标签都只有主键整数列，无法构造可激活证明的查询。没有 A/B 证据，已撤回该代码，不保留无收益的上下文字段与分支。
3. 两跳存在性折叠依赖“无悬空边”证明；当前 schema 不能提供该证明，跳过会改变计数语义，拒绝。
4. `max_partitions=10` 的固定门已经跑满，`order_limit` 缺物理 limit/offset 与跨 shard 全局有序证明，微扫描与 morsel 改动没有可验证收益面，暂缓。
5. 分片锁粒度与 RAM 容量属于架构取舍，缺少偏斜/容量微基准，暂缓。

devs release 全门通过：配置默认值、partitioning 49/49、typed group 9/9、local topk 11/11、parallel 27/27、DQL 324/324、storage 全量、column batch 全绿；两轮固定门 `actual=10`、`result_stable=true`、`spill_rows=0`，结果仍为 `group_by_count [14|59]`、`distinct_limit [14]`。两轮耗时 30.54 / 37.91 ms 与 30.11 / 36.69 ms，生产默认 29.50 / 28.51 ms；与上一切片同量级，单轮波动不视为独立收益。两跳 5.0–5.6 ms、变长路径 4.8–5.8 ms 未退化。判定：只保留第 1 项；它移除每批一条确定性分配且不改变语义，其余项按证明门槛拒绝或暂缓。日志、JSON 与 EXPLAIN 见 `perf-data/parity/results/2026-11-24/parity-convergence/`。

## 48. 结果语义优先审计与修复（2026-11-24）

本轮先审计会改变结果、破坏契约或让验证失去意义的问题，再谈性能。结论写入 `docs/plan/semantic-correctness-remediation.md`：历史记录中的会话键、401 双触发、SSE 尾帧、order-limit 逻辑/物理脱节、index cursor 跨 generation 不有序等问题在当前源码中已修，不重复处理；跨源 ATTACH、`LOAD FROM (query)`、`SET a = {map}`、`CALL option=value` 属于功能缺口，不影响已支持语句的结果正确性。

发现并修复两项当前问题：

1. 分区局部 TopK 只看 `Dedup/Aggregate` 后的第一个 `TopN`，没检查其上方算子。若上方存在 `Filter`、`Sort`、另一个 `Aggregate/Window/TopN` 等，分区内提前截断会丢掉全局结果需要的行。现在要求 `TopN` 之上只能是 `Project`，或 `offset=0` 且 `count <= TopN.n` 的前缀 `Limit`；不安全形状返回 `local_topk:unpushed,reason=unsafe-global-tail`，并同时关闭 packed 身份直通。补了 `Filter` 禁用与前缀 `Limit` 保留两个分区物理计划单测。
2. packed 键直通只输出分组键列，原证明只检查目的端主键与旁路列，未检查边需求。若下游同时需要边属性，该路径会丢列。现在运行时身份快路径与 rowless payload 都要求 bypass 边列为空；`count(h)` 的整边变量需求只参与行数累加、不产生 bypass 列，因此仍可用。计划期不按 `edge_required_props` 的字符串编码猜测，因为那会误关当前 `distinct_limit` 的形状。

中途尝试用 `edge_required_props == Some([])` 或 `skip_rows` 做计划期/运行时证明，实测把 `distinct_limit` 的 `expand_output_rows` 从 30473 推回 215488、耗时升到 50–79 ms；说明证明过窄会关闭合法直通，已撤回。最终证明以“是否产生需要物化的边列”为准。

该轮语义修复在旧 `16-31` 跨节点绑定下 release 全门通过：partitioning 49/49、typed group 9/9、local topk 11/11、parallel 27/27、DQL、storage、column batch 全绿，结果仍为 `[14]` / `[14|59]`。资源对齐重测后同一修复仍全绿，local topk 13/13；固定门 `distinct_limit` 29.98 / 29.96 ms、`group_by_count` 32.05 / 32.53 ms，生产默认 27.28 / 25.92 ms，`result_stable=true`、`spill_rows=0`、`actual=10/8`。EXPLAIN 显示 `expand_output_rows=30473/30536` 且无 `local_topk:unpushed`，语义修复没有误伤合法优化。旧日志见 `perf-data/parity/results/2026-11-24/semantic-correctness/`，当前权威数据见 `perf-data/parity/results/2026-11-25/resource-aligned/`。

## 49. 三方资源对齐证明与重测（2026-11-25）

此前三方数字不可严格比较：Linkrs 固定门使用 `taskset -c 16-31`，该集合横跨两个 NUMA 节点，文档却写成“node1 独占”；Ladybug/Neug 的 JSON 没有亲和性字段，且 Ladybug Python runner 使用 wheel 默认约 80% 系统内存作 buffer，Linkrs 只配置 64 MB。本节先对齐资源，再重采指标；此前基于旧 JSON 的倍率全部作废。

环境与改动：

- 三方统一 `numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`，实际亲和性写入 JSON，均为 `8-15,24-31`。
- 三方参数线程数均为 16；Ladybug 为 16 connection threads，Neug 的 AP 路径仍为单线程 wall-clock，结果只作参照。
- 三方均 warmup 1 + run 5 去最大值平均；Linkrs 同时上报 drop-max mean 与中位数。
- `graphdb-ben/neug_runner.py` 改为扫描 `*.benchmark`，可直接读取 `benches/parity/queries/lsqb/operators/`；新增 `--warmup`、`--run`、`--ladybug-buffer-mb`、`--neug-buffer-strategy`、`--cpu-binding`、`--numa-policy`，并把亲和性、NUMA 策略、线程模型、buffer、计时来源与引擎版本写进 JSON。Ladybug 显式传 `buffer_pool_size=67108864`。
- `scripts/devs-stage1-stage2-gate.sh` 与新增 `scripts/devs-resource-aligned-parity.sh` 统一使用同一绑定；Linkrs parity JSON 新增 `cpu_binding` 与 `numa_policy` 字段。
- devs 上旧的 `2026-11-18-linkrs-ops` 目录与旧三方 JSON 已删除，避免继续引用旧口径。

重测结果（Linkrs 固定门 / 生产默认 / Ladybug / Neug，ms）：

| 用例 | Linkrs 固定门两轮 | Linkrs 生产默认 | Ladybug | Neug |
|---|---:|---:|---:|---:|
| `group_by_count` | 32.05 / 32.53 | 25.92 | 8.16 | 12.28 |
| `distinct_limit` | 29.98 / 29.96 | 27.28 | 3.20 | 28.33 |
| `join_two_hop` | 5.94 / 5.93 | 5.38 | 1.54 | 20.14 |
| `variable_path_count` | 5.60 / 5.90 | 5.38 | 441.42 | 102.90 |
| `filter_or` | 1.42 / 1.62 | 1.53 | 1.21 | 0.32 |
| `order_limit` | 1.33 / 1.34 | 1.21 | 0.46 | 0.04 |

结论：资源对齐后，`group_by_count` 为 Ladybug 的 3.18×、`distinct_limit` 为 8.52×、`join_two_hop` 为 3.50×；可变长路径计数仍显著领先三方。固定门 16 workers / 10 partitions 在 node1 上不再稳定优于生产默认，生产配置因此保持不变。所有正确性门与微基准门重跑通过；证据在 `perf-data/parity/results/2026-11-25/resource-aligned/`。后续任何 A/B 必须复用本节绑定与 JSON 字段，不得再与旧跨节点或未对齐 buffer 的数字混用。
