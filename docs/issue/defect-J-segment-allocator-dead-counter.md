# 问题：`segment_allocator` 是纯负收益的死计数器（false sharing）

- 状态：新建（已验证，待修复）
- 类型：性能缺陷（无效原子 RMW / false sharing）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 J
- 关联：`docs/issue/defect-H-shard-cap-16.md`（同文件 sharded.rs）

## 问题描述

`crates/graphdb-storage/src/storage/vertex/vertex_table/sharded.rs:70` 的 `segment_allocator: AtomicU32` 全库仅 4 处引用，**没有任何一处 `load` 它来做分配决策**：

- `:70` 字段声明；
- `:107` 初始化；
- `:174` `claim_segment()` 的 `fetch_add(1, Ordering::Relaxed)`；
- `:754-755` 加载时 `store(max_segment + 1, ...)`。

## 根因分析（已代码级确认）

- 段归属完全由 `segment_of(idx, local_counter)`（`sharded.rs:168-170`）确定性计算，`segment_allocator` 的值从不被读取用于分配；
- 唯一的调用点 `record_allocation`（`:180-190`）在段切换时 `claim_segment()`，纯粹为 `fetch_add` 而 `fetch_add`；
- 所有 8~16 个分片的插入都在同一条 cache line 上做 RMW → 典型 **false sharing**，带来跨核竞争却不产生任何语义；
- 附带：`record_allocation` 的 `load → max → store` 三步非原子序列（`:181-188`）目前安全仅因调用点持 `shard.table.lock()`，但字段声明为 `AtomicU32` 暗示可无锁访问——若未来锁外调用将引入静默 ID 分配竞态（重复 internal_id）。

## 影响

- 写路径热路径上的无效 RMW 竞争，多核写吞吐受损；
- 死代码误导审查者（以为有全局段分配协调）。

## 修复方向

1. **删除** `segment_allocator` 字段与 `claim_segment` 调用，消除 false sharing；
2. 同步处理 `record_allocation`：既然已被锁保护，`local_counter`/`current_segment` 改回普通 `u32` 字段（或保留 Atomic 但改用 `fetch_max` + 文档化锁内使用契约）。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P2-J。

## 验收

- 删除后 `cargo test --test '*'` 全绿；
- 多核写基准无回归（或小幅提升，消除 RMW 竞争）；
- 全库不再引用 `segment_allocator`（grep 校验）。
