# `distinct_limit` 有序存在性扫描设计

**日期**：2026-11-25  
**目标**：以资源对齐后的当前数据为起点，优先消除 `distinct_limit` 相对 Ladybug 的主要差距；本文只定义下一阶段任务、可证明边界与验收门槛，不实现代码，也不改变已冻结的正确性契约。

## 1. 当前问题

资源对齐数据见 `perf-data/parity/results/2026-11-25/resource-aligned/` 与 `docs/issue/linkrs-remaining-parity-issues.md` 第 49 节。

| 用例 | Linkrs 生产默认 | Ladybug | Neug | 判定 |
|---|---:|---:|---:|---|
| `distinct_limit` | 27.28 ms | 3.20 ms | 28.33 ms | 对 Ladybug 8.52×，对 Neug 同量级 |
| `group_by_count` | 25.92 ms | 8.16 ms | 12.28 ms | 对 Ladybug 3.18× |
| `join_two_hop` | 5.38 ms | 1.54 ms | 20.14 ms | 对 Ladybug 3.50× |

优先处理 `distinct_limit` 的原因：

1. 它是当前对 Ladybug 倍率最大的用例，且绝对耗时仍为 27 ms 级。
2. 固定门 16 workers / 10 partitions 为 29.98 / 29.96 ms，没有优于生产默认；继续加并行度不是解法。
3. 查询语义允许算法级改写：它只需要“按主键有序的前 k 个存在入边的目的端”，不需要边属性、不需要路径 multiplicity、不需要遍历全部 Comment 顶点。
4. `group_by_count` 虽绝对耗时接近，但 `count` 必须保留每条边的 multiplicity，不能复用同一改写；`join_two_hop` 的绝对差值和收益空间都更小。

当前查询为：

```text
MATCH (:Comment_)-[h:HASCREATOR_COMMENT]->(p:Person)
RETURN DISTINCT p.id ORDER BY p.id LIMIT 10
```

现有执行形状是：按 Comment 顶点范围分区扫描 → 单跳展开 → 身份主键直通 → 每分区局部 TopK → Exchange → 全局 Distinct / TopN / Limit。EXPLAIN 中固定门 110 个父批、215488 个拓扑事件、30473 个唯一目的端、生产 `expand_output_rows=30536`。直写与身份直通消除了逐边输出，但仍未消除“扫描全部源点并展开全部边”。

## 2. 核心思路

在有可证明索引的情况下，把执行方向反过来：

- 目的端 `Person.id` 是唯一、非空、单列主键索引，索引游标按 key 全局有序。
- `HASCREATOR_COMMENT` 的边类型 schema 证明 `FROM Comment_ TO Person`，因此从 Person 方向检查入边即可覆盖原查询的全部匹配。
- 查询只要求 key 的存在性，`DISTINCT` 让重复边与 multiplicity 无关；一旦某个 key 被证明存在，就只输出一次。
- 按主键索引顺序枚举目的端候选，对每个候选执行“带可见性门的入边存在性检查”，只输出通过的候选；累计输出 k 个后停止。

该改写把工作量从“所有 Comment + 所有边”变为“按 key 顺序检查到找到 k 个存在入边的 Person 为止”。它不是近似，也不是截断全局结果：索引顺序保证前 k 个通过检查的 key 就是全局 `ORDER BY key LIMIT k` 的精确结果。

## 3. 启用证明

只有同时满足以下条件才允许改写；任一条不成立则保持现有分区局部 TopK 路径。

### 3.1 逻辑形状

- 单个 `MATCH`，单跳、单条边类型、方向为出边；无变长、无 path 语义、无 filter、无 sample、无 `include_empty_paths`。
- 目的端只需求一个属性，且该属性由 schema 证明是目的端标签的唯一主键；不能按列名猜测。
- 分组/去重列就是该主键；输出只投影该 key，不需求边属性、源端属性或其他目的端属性。
- `ORDER BY` 只有一个排序项，且就是该主键；初始实现只接受 ASC，DESC 必须先证明游标反向全序。
- `LIMIT` 为正的有限值；`offset=0`。若上层还有 `Limit`，必须是零偏移且不超过已证明的 k。
- `TopN` 之上只允许顺序无损的 `Project` 或前缀 `Limit`；`Filter`、`Sort`、另一个聚合/去重/窗口都禁止。
- 源端标签必须能由边类型声明证明，不能靠运行时猜测。

### 3.2 存储与索引证明

- 目的端存在唯一、非空、单列主键索引，索引字段类型与执行器键编码兼容。
- 索引游标必须提供跨 generation 的全局有序证明；多 shard 时必须按 key 归并，newest generation 同 key 优先，不能只按 shard 顺序拼接。
- 索引读取与存在性检查必须使用同一读集合/快照；游标 generation 漂移时放弃改写，回退原路径。
- 目的端可见性由索引游标按快照裁决；存在性检查只额外裁决源端顶点与边。

### 3.3 存在性门证明

新增的入边存在性检查必须与完整展开逐条等价：

- 同一边类型与同一 leg；
- 同一方向映射：原出边 `src -> dst` 等价于从 `dst` 读反向 CSR 后检查 `src`；
- 源端顶点存在、标签匹配且在快照下可见；
- 边可见性与 TTL/staged write 规则与原展开一致；
- 悬空边必须跳过，不能把目的端 key 当作存在；
- 平行边、重复边只影响存在性布尔值，不影响输出行数；
- 不把 `frontier_degree_batch` 直接当作存在性证明：它当前没有源端可见性与悬空边的完整门。

计划期给不出证明时，写入 EXPLAIN 诊断；运行时证明失效时也回退，不能静默使用旧计划。

## 4. 实施任务

### 任务一：逻辑识别与诊断

在查询层加入窄形状识别，输出可审计的“有序主键存在扫描”证明：边类型、源/目的标签、主键字段、索引、排序方向、limit、下游算子白名单。证明结果进入 `EXPLAIN`，未命中形状给出明确原因。

落点在计划链拆解、逻辑规则与物理 spec 构建，不新增替代执行器模块。

### 任务二：有序存在源算子

在现有 source spec / source operator 中增加有序主键存在扫描模式：

- 打开目的端主键索引游标，不把 k 直接下推成原始候选 limit；原始游标必须持续读取直到输出 k 个通过存在性检查的 key。
- 小批读取候选 key，批次内逐 key 做存在性检查；通过者按索引顺序输出，未通过者丢弃。
- 记录候选检查数、通过数、输出数、generation 与回退原因，供 EXPLAIN/ANALYZE 证明早停真实发生。
- 保持 chunk 布局与现有 packed key 下游兼容；不重新引入逐边行物化。

### 任务三：批量入边存在性读取

在存储层提供窄原语：按一批目的端内部 ID 与边类型，返回每个目的端是否存在满足可见性门的入边，并保留输入顺序。该原语复用反向 CSR 与现有可见性门，不构造 `Edge`、不解码属性、不读取边属性列。

如果无法在同一快照下证明源端存在与边可见性，禁止把该原语用于改写。

### 任务四：消除冗余全局算子

命中改写后，源算子已经保证 key 有序且最多 k 行；保留最终 `Project` 作为列别名，但 `Distinct` / `TopN` / `Limit` 必须退化为证明检查或移除。任何保留的算子都不得再次截断或重排结果。

### 任务五：差分测试

对同一小图分别运行慢路径与改写路径，逐值比较：

- 空图、无边、不足 k 个匹配；
- 重复边、平行边、同一目的端多个源；
- 主键非连续、稀疏、负值、NULL 约束拒绝；
- TTL 过期边、staged edge、已删除源端、悬空入边；
- ASC/DESC、limit 0/负数/无界、offset 大于零；
- 多边类型、多跳、源端或边属性需求、非主键排序键；
- filter 位于 TopN 上方、其他聚合或 Sort 位于上方；
- 索引 generation 漂移与快照不一致。

所有反例必须回退现有路径并保留正确结果。

## 5. 验证门槛

### 5.1 正确性门

- 现有 partitioning、typed group、local topk、parallel、DQL、storage 全量门保持全绿；
- 新增差分测试全部通过；
- 固定门与生产默认结果仍为 `distinct_limit [14]`、`group_by_count [14|59]`、`result_stable=true`、`spill_rows=0`；
- EXPLAIN 中命中形状显示有序存在扫描证明与候选检查数；不安全形状显示回退原因。

### 5.2 性能门

沿用资源对齐口径：

- `numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31`；
- 三方同批数据，Linkrs warmup 1 + run 5；
- Linkrs 生产默认与固定门都跑；固定门仅作上限口径，不用于替换默认配置。

验收目标：

- `distinct_limit` 生产默认 ≤15 ms 且相对 Ladybug ≤4×；达到 ≤10 ms 且 ≤3× 时才视为接近闭环；
- `group_by_count`、`join_two_hop`、`variable_path_count` 与九条纯计数不得回归；
- 候选检查数显著小于 215488，且 EXPLAIN 证明早停不是靠放宽语义获得；
- 不通过放宽并行、近似、截断、放宽 buffer 或改小数据集换取门槛。

若存在性检查的候选数没有明显下降，或为了正确性必须检查绝大多数 Comment 边，则本改写不成立，删除新分支，不保留未证明的复杂度。

## 6. 不做的方向

- 不把原始 `IndexScan` 的 limit 直接设为 k 后再做存在性过滤；那会漏掉被过滤候选之后的有效 key。
- 不按列名推断主键或源端标签。
- 不为追平 Ladybug 打开 `parallel.enabled`、`allow_global_aggregate`、`allow_pure_count_chain` 或 `allow_parallel_count_frontier`。
- 不把 CSR 预取候选重新引入生产；已有 A/B 没有达到 15% 门槛。
- 不把该改写用于 `group_by_count` 或任何需要边 multiplicity 的查询。
- 不在缺少跨 generation 有序证明时启用多 shard 索引早停。

## 7. 后续顺序

先完成任务一的证明与反例测试，再实现任务二、三；任务四只有在源算子输出契约稳定后接入。每个任务独立可验证，任何一步无法给出精确结果或稳定收益时停止并回退该步，不扩大到其他查询形状。
