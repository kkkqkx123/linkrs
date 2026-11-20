# 剩余路径完整分析与最终设计方案

**日期**：2026-11-21  
**范围**：统计精确性、谓词/排序比较器、id-range 下推、order-limit 下推、元数据计数、诊断计数。本文汇总此前分散文档中的分析结论，并记录本轮实际落地和仍需后续修改的事项。

## 1. 总体判断

当前代码库的结果正确性依赖两条原则：

1. 任何下推都只是预过滤或候选裁剪，最终权威算子仍保留；
2. 下推必须携带可检查的证明 token，不能只依赖形状匹配。

本轮之前存在的最大风险是 order-limit 规则只检查“TopN 的直接孩子是 IndexScan”，既没有证明索引顺序等于排序顺序，也没有验证物理执行是否消费了 limit/order_by。本轮把该路径收敛为：规则只有在携带 `IndexOrderProof` 时才允许改写；物理层仍不消费该 limit，因此不会产生错误的提前截断。

## 2. 统计精确性

### 2.1 事实

`ColumnStatsSnapshot::min_value/max_value` 来自 zone-map 聚合。写入会让边界 widening，删除和 TTL 不会回收边界，因此它只能用于裁剪。

### 2.2 已完成

- 快照增加 `bounds_are_exact`；
- 增加 `exact_min_value()` / `exact_max_value()`，只有证明标记为真才返回边界；
- `absorb()` 任一侧非精确即降级；
- 所有现有生产者写 `bounds_are_exact: false`；
- 反例测试确认 pruning bounds 永不作为结果边界。

### 2.3 后续修改

结果下推必须读取 exact accessor。生产端只有在无 TTL、无 staged/pending write、单关系单列、同 snapshot timestamp、完整扫描时才能设置 token。当前没有这样的生产者，min/max 结果下推保持关闭。

## 3. 谓词比较器

### 3.1 已发现并修复

原存储比较器与表达式求值器在 `Float(NaN)` 上不一致；zone-map 的数值提升也与求值顺序不完全一致。

已完成：

- 范围比较统一到 `Value::cmp`；
- `ColumnEqual` 改用 `PartialEq`；
- zone-map `compare_values` 委托 `Value::cmp`；
- 新增 `predicate_comparator_proof`，对全标量矩阵与 evaluator 交叉对拍。

该路径已具备作为 id-range 下推前置证明的条件。

## 4. id-range 下推

比较器证明已完成，但固定门里的 `n.id > 0` 覆盖全集，收益上限接近零。保持关闭。后续若开放，需要：

- scan spec 增加 `id_range_predicate`；
- 只允许单列 BigInt、非空、非 TTL、无 staged write；
- EXPLAIN 输出 pushed/unpushed；
- 新增 pushed/unpushed 对拍。

## 5. order-limit 下推

### 5.1 完整结论

order-limit 曾同时存在两类问题：一类是代码实现缺陷（规则声称下推但物理层不消费、offset/limit 语义错误、generation 游标不保证全局有序），已单独记录在 `docs/plan/current-code-implementation-defects.md` 并修复；另一类是优化项是否值得立项，与本轮代码修复无关。修复后：规则只有在携带 `IndexOrderProof` 时才允许改写，物理层已正确消费 limit/offset，游标保证全局 key 顺序。

比较器层面的立项边界仍包括：NULL 放置、NaN、跨物理整数类型、宽年份 Date 和并列值。proof 生成器直接拒绝这些形状，不把它们误判为已证明。

### 5.2 本轮已落地

- 新增 `IndexOrderProof`，携带列、方向、证明原因；
- `IndexScanNode` 持有可选 proof；
- `decide_index_scan` 仅在唯一、单列、非空、无 partial index、codec 兼容标量类型时生成 proof；
- TopN 规则要求 proof 存在，且单列排序键与 proof 列和方向完全一致；拒绝 residual filter、非正 limit、多列排序；
- Limit 规则拒绝 residual filter、已有 order_by、负 offset/count、溢出和 LIMIT 0；
- proof reason 进入 SourceSpec 与 EXPLAIN，显示 `order_proof:<reason>` 或 `order_unproven`；
- limit/offset 贯穿 SourceSpec、SourceOperatorKind 与 IndexScanPlan，规则测试覆盖无 proof、方向不匹配、溢出和正常下推；
- generation 游标改为 k 路归并，新增跨 generation 有序与 newest 优先测试。

实现缺陷修复使 order-limit 的已声明语义完整；是否把它作为高优先级优化项继续投入，需要另立设计评估，不受实现缺陷修复结论影响。

### 5.3 立项级边界

以下不是实现缺陷，而是优化项范围问题：

- Desc 方向没有反向游标证明，proof 只生成 Asc；
- Float/Double、Date/DateTime、NULL、跨类型数值和并列值仍由 proof 生成器拒绝；
- 更宽形状的 pushed/unpushed 对拍与收益评估属于立项后的验收工作。

全局 TopN 仍保留为最终权威。

## 6. 元数据计数

`metadata_count_below` 已有完整 gate：纯全局 count、value-only Project、无 Filter/TTL/递归/写入、只读或 autocommit 无 staged、未知 tag/type 拒绝、执行期复核。分区路径另有 storage 证明和 allowlist 双门。继续不把元数据计数当作强一致查询结果。

## 7. 诊断计数

### 7.1 已落地

- `ColumnarStats` 增加 `typed_group_hits` / `typed_group_fallbacks`，在 aggregate 与 partial_aggregate 的 typed 尝试点记录；
- `ColumnarStatsSnapshot::summary()` 输出新字段；
- `PersistenceCoordinator` 增加 `reclaim_failures`，checkpoint retention 失败时自增；
- `PersistenceDiagnostics` 暴露该计数；
- 新增对应单元测试。

### 7.2 后续修改

`linkrs_parity_bench.rs` 的资源 JSON 尚未接入 `reclaim_failures`，因为当前 embedded API 没有直接暴露 persistence diagnostics。后续应先在 API 层增加只读 accessor，再由 bench 写入 JSON；不要绕过 API 直接访问 storage 内部。

## 8. 验收记录

本轮完整验证：

- `cargo check -p linkrs-query -p linkrs-storage`；
- `cargo test -p linkrs-query --lib`：2313 passed；
- `cargo test -p linkrs-storage --lib`：1409 passed；
- predicate proof、order proof、DQL、parallel、checkpoint retention、WAL writer、storage lib 全量；
- 固定门两轮，要求 `actual=10`、逐值一致、`spill_rows=0`。

固定门结果：

| 用例 | 第一轮 | 第二轮 |
|---|---:|---:|
| `distinct_limit` | 101.13 ms | 98.25 ms |
| `group_by_count` | 100.49 ms | 92.46 ms |

`actual=10`、`actual_partitions=10`、`local_topk:pushed:runtime`、结果逐值一致、`spill_rows=0`。库体积约 38.6 MB，2 个 checkpoint、0 个快照、2 个 WAL 数据文件。最终指标与日志目录：`perf-data/parity/results/2026-11-18/final-design/`。

## 9. 执行顺序

1. 保持所有未证明下推关闭；
2. 接好 API 层 persistence diagnostics，再写入 bench JSON；
3. 解决 cursor 跨 shard 全局有序证明；
4. 再考虑物理 order-limit 下推；
5. exact min/max 生产端只有在完整可见性证明后开放。
