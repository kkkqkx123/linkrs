# Linkrs 剩余对齐问题清单

**日期**：2026-11-18  
**状态**：本文件是 Linkrs 与 Neug / Ladybug 对齐剩余任务的单一真相源。历史方案与旧问题清单已删除，不再作为判断依据。分阶段修改方案见 `docs/plan/linkrs-remaining-parity-implementation-plan.md`，该方案不重新定义问题。

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

### devs 实测指标与对比发现（2026-11-18）

环境：devs（E5-2667 v3，绑核 `16-31`、NUMA node 1，`powersave`），内核 4.15，rustc 1.100.0；Linkrs 为本地未提交变更同步后的 release 构建，Ladybug 为 0.22.3 wheel，Neug 为 0.3.1。口径：Linkrs warmup 1 + run 5 的中位数，Ladybug/Neug warmup 1 + run 3 去最大值平均；Neug 为 AP 单线程 wall-clock，只作参照。两轮 Linkrs 独立运行原始日志在 `perf-data/parity/results/2026-11-18/`。

| 用例 | Linkrs 两轮中位数 ms | 编译中位数 ms | Ladybug ms | Neug ms | 行数/首值 | 备注 |
|---|---:|---:|---:|---:|---|---|
| `aggregate_filtered_count` | 2.67 / 2.98 | 0.02 | error | error | 1 / NA / NA | 方言被两台对手拒绝，进跳过清单 |
| `aggregate_min_max` | 1.24 / 1.33 | 0.02 | 1.020 | 0.158 | 1 / 14 / 14 | 约为 Ladybug 的 1.2–1.3× |
| `distinct_limit`（修正后） | 1107.75 | 0.03 | 3.755 | 28.701 | 10 / 14 / 14 | 修正排序后首值一致；仍约为 Ladybug 的 295× |
| `filter_id_range` | 1.73 / 1.84 | 0.02 | 0.974 | 0.159 | 1 / 1700 / 1700 | 约为 Ladybug 的 1.8–1.9× |
| `filter_or` | 1.58 / 1.62 | 0.02 | 1.517 | 0.317 | 1 / 1603 / 1603 | 基本对齐 Ladybug |
| `group_by_count` | 1307.53 / 1498.62 | 0.03 | 7.104 | 13.762 | 10 / 14 / 14 | 约为 Ladybug 的 184–211×，最大缺口 |
| `join_two_hop` | 35.43 / 48.53 | 0.02–0.03 | 2.422 | 24.088 | 1 / 382018 / 382018 | 约为 Ladybug 的 15–20× |
| `order_limit` | 1.29 / 1.35 | 0.01–0.02 | 0.880 | 0.044 | 10 / 14 / 14 | 约为 Ladybug 的 1.5× |
| `shortest_path` | 1.78 / 1.96 | 0.01 | error | error | 0 / NA / NA | 方言被对手拒绝且当前端点对无路径，进跳过清单 |
| `undirected_count` | 4.59 / 5.75 | 0.02 | 1.926 | 1.450 | 1 / 36270 / 36270 | 约为 Ladybug 的 2.4–3.0×，慢路径预期内 |
| `variable_path_count` | 1102.97 / 1350.19 | 0.03 | 457.680 | 117.401 | 1 / 400153 / 400153 | 约为 Ladybug 的 2.4–3.0× |

原有九条纯计数仍为 0.15–0.60 ms，与历史结论一致；编译中位数稳定在 0.01–0.03 ms。Linkrs 进程 RSS 从 668 MB 升到 720 MB，库体积约 197 MB；Ladybug/Neug 同批数据库体积分别约 35 MB / 43 MB，库体积口径差异单独归类到资源报告问题。

**本轮新发现**：

1. `group_by_count` 与 `distinct_limit` 秒级耗时，而 Ladybug 在毫秒级；这是此前九条 COUNT 完全没有暴露的最大算子缺口，优先于分区放行与图算法。
2. `group_by_count` 的结果只有 10 组，却比返回 10 行的 `distinct_limit` 还慢，说明成本不在输出物化；需要按计划形状和每算子行数定位，而不是先改存储。
3. `DISTINCT ... LIMIT` 未带 `ORDER BY` 时三方首值不同（Ladybug 8796093023616、Neug 14），是基准定义不确定的问题，不是引擎数值错误；已给该用例补 `ORDER BY` 并复测三方首值一致。
4. `aggregate_filtered_count` 与 `FIND SHORTEST PATH` 均为 Linkrs 方言，Ladybug/Neug 直接解析失败；进入跳过清单，不能计入三方数值对比。
5. 测量期间 devs 上有他人 `wf-server` release 构建，两轮 Linkrs 绝对耗时存在 10%–20% 漂移；慢于毫秒级的量级结论不受影响，但细粒度比例需在安静窗口复核。

---

## 2. 带谓词、分组与去重计数仍缺快路径证据

**现象**：无过滤全局计数已走元数据或向量化路径；带过滤聚合虽然语义正确，但逐行裁决后累加，行计数快捷路径和批量表达式路径关闭。分组空值语义缺少专门锁定，去重计数按设计保持慢路径。devs 实测进一步显示分组计数约 1.3–1.5 秒、去重加排序分页约 1.1 秒，分别是 Ladybug 的 184–211 倍和约 295 倍，是当前最突出的算子缺口。

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

**当前锚点**：`crates/linkrs-query/tests/dql/find_path.rs` 覆盖路径与最短路语义；`benches/parity/queries/lsqb/operators/shortest_path.benchmark` 只是可执行 smoke，且 Linkrs 专用 `FIND SHORTEST PATH` 方言已被 Ladybug/Neug 拒绝并写入跳过清单；变长路径计数在 devs 上约 1.1–1.35 秒，是 Ladybug 的 2.4–3.0 倍，需要在纳入算法图前先定位。

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
| `perf-data/parity/results/2026-11-18/` | devs 两轮 Linkrs 原始日志、Ladybug/Neug 原始 JSON、`distinct_limit` 修正后复测结果（本地忽略目录） |
| devs `/data/kkkqkx/graphdb-ben/results/2026-11-18-linkrs-ops/` | 同一批结果的远端落盘，含数据库目录与运行日志 |
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
