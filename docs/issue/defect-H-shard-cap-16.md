# 问题：点表分片数硬上限 16、运行期不可变更，且读路径互斥

- 状态：新建（已验证，待修复）
- 类型：可扩展性缺陷（分片设计 / 读并发）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 H
- 关联：`docs/plan/storage-concurrency-correctness-rework-design.md` P2-H、`docs/plan/parallel-extension-and-storage-rework-design.md`（并行分区计划）

## 问题描述

`crates/graphdb-storage/src/storage/vertex/vertex_table/sharded.rs:15-16`：

```rust
const DEFAULT_NUM_SHARDS: usize = 8;
const MAX_SHARDS: usize = 16;
```

分片数有 16 硬上限、不随 CPU 核数/数据量自适应、运行期不可变更，且分片内读路径拿独占 `Mutex`。

## 根因分析（已代码级确认）

1. **不自适应**：不随 `available_parallelism()`、数据量、冲突率变化。64 核机器上单点标签写并发上限被钉死在 16；
2. **不可扩容**：分片数同时决定 **ID 编码布局**（`sharded.rs:34-47`，`shard = (segment % num_shards)`），改分片数 = 改 ID 语义，是破坏性变更，运行期无法 rehash（存储的 ID 依赖该编码）；
3. **读读互斥**：分片用 `Mutex` 而非 `RwLock`（`sharded.rs:49-57`），读路径也拿独占锁（`get_by_internal_id`，`sharded.rs:254-258`）。点查是图数据库最高频操作，8 分片 = 理论 8 路并发点查；
4. **跨分片操作串行 + 结果撕裂**：`total_count`（`sharded.rs:274-280`）逐个上锁取样，返回永不对应任何真实时刻的近似值；
5. **边表不参与哈希分片**：分区维度是 `EdgeTableKey{src_label,dst_label,edge_label}` 的 schema 划分。单一 `(Person)-[Follows]->(Person)` 超级标签的所有边共用一把 `RwLock<EdgeStore>`，社交图负载下是单点瓶颈。

## 影响

- 多核写/读并发被分片数锁死；
- 超级边标签（明星标签）成为全局串行点；
- ID 编码与分片数耦合，扩容即破坏存储格式。

## 修复方向

1. **分片数自适应**：按 `available_parallelism()` 初始化（保留上限以约束 ID 布局）；
2. **`Mutex` → `RwLock`**：读路径读读并发（`get_by_internal_id` 等）；
3. **解耦 ID 编码与分片数**：`encode_id`/`decode_id` 改为查表或显式分片字段，解除 16 上限；
4. **超级边标签内部哈希分片**：按 src/dst 外部 id 哈希分流到多把锁（对标点表分片模式）；
5. **`total_count` 语义**：明确为"近似值"并在文档标注，或提供一致性计数（维护原子计数）。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P2-H。

## 验收

- 64 核机器上单标签点查/写并发超过 16 路（基准验证）；
- 读读并发（多线程点查无互斥损耗）；
- `total_count` 返回语义文档化；
- 全量 `cargo test --test '*'` + clippy 全绿。
