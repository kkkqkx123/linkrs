# 剩余路径的证明方法与后续修改方向

**日期**：2026-11-21  
**范围**：统计精确性、谓词与排序比较器、id-range/order-limit 下推、元数据计数、typed 聚合与回收失败计数。本文记录证明义务、已完成的验证和后续必须修改的位置，不包含完整代码。最终收敛版见 `docs/plan/remaining-paths-final-design.md`。

## 1. 统计精确性：pruning bounds 与 exact bounds

### 事实

`ColumnStatsSnapshot::min_value/max_value` 来自 zone-map 边界。写入会 widening，删除和 TTL 不会缩小边界；两个实现点分别是 `edge/csr_with_properties/encoding.rs` 和 `vertex/vertex_table/sharded/read.rs`。因此这些边界只适合裁剪，不能作为 `MIN`/`MAX` 聚合结果。

### 已完成的证明契约

- `ColumnStatsSnapshot` 增加 `bounds_are_exact`。
- `exact_min_value()` / `exact_max_value()` 仅在该标记为真时返回边界；结果消费者只能调用这两个访问器。
- `absorb()` 在两侧都精确时才保留精确标记；任一侧来自 pruning 边界，合并结果立即降级。
- 当前全部生产者写入 `bounds_are_exact: false`，即没有伪造精确证明。

### 精确的充分条件

要打开 min/max 结果下推，生产者必须同时证明：

1. 读取使用与查询一致的 snapshot timestamp；
2. 关系不含 TTL 过期行、无 staged write、无 pending write，且统计期间无并发 DML；
3. 关系是单标签/单边类型，不是多 shard 的近似合并；
4. 边界来自对可见行的完整扫描，而不是 zone-map 聚合；
5. 统计版本与数据版本绑定，DML 后立即失效。

### 后续修改

- 在 `ColumnStatsReader` 上增加显式的 exact snapshot API，而不是复用现有 pruning snapshot。
- 生产者只有在上述条件全部成立时设置 `bounds_are_exact=true`。
- 在聚合下推入口只消费 `exact_min_value/exact_max_value`，并在 EXPLAIN 记录 `pushed/unpushed,reason`。
- 单测必须包含删除后边界不回收、TTL、staged write、并发 DML 四种反例。

## 2. 谓词比较器证明

### 已发现的问题

`ScanPredicate` 使用自己的 `compare_scalar`，而表达式求值使用 `Value` 的 `PartialEq`/`Ord`。在 `Float(NaN)` 与整数比较时，两条路径不一致：求值认为不相等，旧存储比较器把 NaN 当作 Equal。zone-map 也有独立的数值提升逻辑，和求值器在 BigInt/Float 上不完全一致。

### 已完成的修改

- `cursor/predicates.rs` 的范围比较统一到 `Value::cmp`，即表达式 `<`
  / `<=` / `>` / `>=` 使用的同一顺序。
- `ColumnEqual` 改用 `PartialEq`，因为 Rust 浮点 `NaN != NaN`，不能用 `Ord::Equal` 代替相等。
- `vertex/column/zone_map.rs` 的 `compare_values` 同样委托给 `Value::cmp`，避免裁剪和求值分歧。
- 新增 `crates/linkrs-query/tests/predicate_comparator_proof.rs`，对 11 个代表值 × 5 个操作符 × 全部标量字面量做交叉验证，并覆盖 NULL/missing property。测试直接与 `BinaryOperationEvaluator` 对比。

### 证明边界

该证明只覆盖现有 pushable 形状：单列、标量字面量、等值或单边区间。列表、map、vertex/edge、NULL 字面量和多列谓词仍不进入存储预过滤。原始过滤表达式仍在扫描上层执行，因此预过滤只需要保守，不需要成为唯一权威。

## 3. id-range 下推

### 证明义务

- 区间边界与 `Value::cmp` 一致，NULL/missing 的取舍与求值器一致；
- 写入可见性与 TTL 必须与扫描一致；
- staged write 不得被元数据范围遗漏；
- 分区边界必须有序、不重叠，已被 `PartitionSpec::try_new` 校验；
- 下推只做预过滤，最终 Filter 仍执行。

### 当前结论

比较器证明完成后，id-range 的语义路径可以成立；但 `n.id > 0` 在固定门上覆盖全集，收益上限接近零。当前没有理由为这条形状增加计划标记和维护成本。保持关闭。

### 后续修改

若要开放，先在扫描 spec 上增加 `id_range_predicate` 与 `pushed/unpushed,reason`；仅允许单列 BigInt、非空、非 TTL、无 staged write；新增 pushed/unpushed 数值对拍后再考虑删除上层 Filter。

## 4. order-limit 下推

### 证明义务

必须同时证明：

1. 存储索引顺序等于排序使用的 `compare_values`；
2. NULL 排序位置一致（当前排序把 NULL 放最后）；
3. 并列值的 id tie-break 一致；
4. 索引可见行不含 TTL 过期或 staged write；
5. limit + offset 在 EXPLAIN 中可区分 pushed/unpushed。

### 已完成的验证

新增 `crates/linkrs-query/tests/order_limit_pushdown_proof.rs`，直接比较 `OrderedCodec` 字节序与执行器 `compare_values`：

- Bool、SmallInt、Int、BigInt、有限 Float/Double、String、FixedString、常规 Date/Time/DateTime、Decimal128、Uuid 的单类型有限样本，索引序与排序序一致。
- `encode()` 把 NULL 放在标量之前，而排序要求 NULL 最后；只有 `encode_null_last()` 与排序一致，但当前索引键构建使用 `encode()`。
- Float/Double NaN 在排序比较器中与任意值相等，无法与全序字节编码一致。
- 跨物理整数类型（Int vs BigInt）在排序比较器中走字符串回退，与索引的数值序相反。
- Date 走 `to_string()` 回退，年份 999 与 1000 的字典序与年代序相反。
- `key_builder` 新增测试确认同值索引键按 entity id 排序；执行器 sort 是稳定排序，并列顺序由输入到达顺序决定。两者不同，不能把 LIMIT 切分给索引。

### 当前结论

order-limit 仍不开放。单类型、非 NULL、无 NaN、有限日期范围的列虽然有比较器一致性，但 NULL 放置、并列 tie-break、TTL/staged write 和 offset 语义仍缺证明。

### 后续修改

- 在 storage 暴露 `IndexOrderProof` token，至少包含：逻辑类型、单物理数值类型、nullable=false 或 `null_last` 编码、无 NaN、Date 年份范围、tie-break 策略、TTL/staged-write 状态。
- 在 planner 中只有 token 全真时才生成 `IndexTopN` spec，并在 EXPLAIN 输出 `pushed/unpushed,reason`。
- 保留全局 TopN 为最终权威；索引只读候选边界，不直接决定并列顺序。
- 对 NULL、NaN、跨类型数值、宽年份 Date、并列值、TTL、staged write 逐项做 pushed/unpushed 数值对拍。

## 5. 元数据计数

### 当前证明

`metadata_count_below` 已经覆盖：纯全局 count、value-only Project、无 Filter/TTL/递归/写入、只读或 autocommit 无 staged、未知 tag/type 拒绝、执行期重复检查。分区路径另有 `allow_partition_metadata_count` 与 storage 证明双门。DQL 和并行测试已有对应反例。

### 后续修改

不把元数据计数当作强一致查询结果。若要开放新的计数形状，必须扩展同一组 gate 并新增对拍，而不是绕过 `metadata_count_below` 直接读取 catalog 计数。

## 6. typed 聚合命中与回收失败计数

这两项不是正确性前置，而是诊断缺口。明确修改位置：

- `runtime/columnar_stats.rs`：增加 `typed_group_hits`、`typed_group_fallbacks`，在 `try_accumulate` 的调用点由 operator runtime 传入计数器，并加入 `ColumnarStatsSnapshot::summary()`。
- `pipeline/diagnostics.rs`：把新计数附到 `columnar_summary`，不新增第二套统计通道。
- `engine/persistence_coordinator.rs`：增加 `reclaim_failures: AtomicU64`，在 checkpoint/snapshot/WAL 删除失败处自增；`diagnostics.rs` 暴露快照值。
- `linkrs_parity_bench.rs`：把失败计数写进运行资源 JSON。

未暴露前不要用日志猜测回收是否成功。

## 7. 验证记录

固定门已把 `predicate_comparator_proof` 与 `order_limit_pushdown_proof` 加入 gate。最终两轮结果：

| 用例 | 第一轮 | 第二轮 |
|---|---:|---:|
| `distinct_limit` | 111.84 ms | 94.31 ms |
| `group_by_count` | 105.83 ms | 100.98 ms |

`actual=10`、`actual_partitions=10`、`local_topk:pushed:runtime`、结果逐值一致、`spill_rows=0`；库体积约 38.5 MB，2 个 checkpoint、0 个快照、2 个 WAL 数据文件。结果在 `perf-data/parity/results/2026-11-18/proof-validation/`。

- `cargo test -p linkrs-storage --lib`：1409 passed。
- `cargo test -p linkrs-query --test predicate_comparator_proof`：2 passed。
- `cargo test -p linkrs-query --test order_limit_pushdown_proof`：5 passed。
- `cargo test -p linkrs-storage --lib -- key_codec::key_builder`：5 passed。
- `cargo test -p linkrs-query --test dql`：324 passed。
- `cargo test -p linkrs-query --test parallel_partition_execution`：27 passed。
- 固定门仍要求 `actual=10`、结果逐值一致、`spill_rows=0`。

精确性 token 和比较器修复先落地；其余下推在证明与 pushed/unpushed 对拍齐全前保持关闭。
