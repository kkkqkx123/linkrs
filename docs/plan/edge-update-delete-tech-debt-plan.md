# 边更新/删除技术债务清理方案

> 审查范围：`bfd06`（edge-update phase1）以来的全部改动。
> 审查结论：设计中的各项修复（段合并误删、hot 删除历史保留、双方向一致性、
> DeletionInfo 统计、snapshot_dirty、属性删除同步、自动维护、tombstone 统一入口、
> 写写冲突检测、版本链收敛）均已落地，但存在以下技术债务。
> 生成日期：2026-08-22

## 一、问题清单

| 编号 | 问题 | 严重度 | 阶段 |
|------|------|--------|------|
| D1 | `graphdb-storage` 测试目标编译失败：`MockStorage` 未实现 OLAP 阶段新增的 `AutoCommitBatchOps`/`AutoCommitGroupOps`，8 个编译错误，整个 crate 单测无法运行 | 高（回归） | 一 |
| D2 | 段合并整组全删时段泄漏：合并组的边全部被物理丢弃时 `merged_entries` 为空，源段不从 `segments` 移除也不回收，死段永久滞留（merge.rs:97/143） | 高（内存泄漏） | 一 |
| D3 | `PropertyTable::update()` 无写写冲突检查，cold delta 应用路径（storage/cold/delta.rs:455）可静默覆盖；与 `set_property` 的冲突检测语义不一致 | 中 | 一 |
| D4 | 版本链折叠不感知活跃快照：`fold_oldest_versions` 超限折叠可能破坏老快照仍需要的历史版本；`gc_versions` 是快照感知的，两者语义不一致 | 中 | 一 |
| D5 | `buffer_manager.rs` 完全未集成：workspace 内零引用，8 个 `#[allow(dead_code)]`，自述 stub | 中 | 二 |
| D6 | 条带锁机制"装饰性"存在：`PropertyTable.stripe_locks` 被构建/克隆/持久化，但从不保护任何读写路径，唯一加锁在单测中；`striped_lock.rs` 整个文件无真实消费方 | 中 | 二 |
| D7 | 死配置面：`EdgeTableConfig.region_high_deletion_ratio` / `region_low_density_threshold` 带 `#[allow(dead_code)]` | 低 | 二 |
| D8 | `gc_tombstones` 每次执行死诊断工作（`_probe` 等 "keep path exercised" 代码） | 低 | 二 |
| D9 | 快路径冲突检查重复执行两次（set_property 预检 + set_property_fixed_size 内部再检） | 低 | 二 |
| D10 | `compact()` 构建从未使用的 `offset_mapping` 死局部变量（compact_with_relocation 才是使用方） | 低 | 二 |
| D11 | 物理合并日志重复打印同一变量（merge.rs:586-588） | 低 | 二 |
| D12 | 注释错误：版本链 doc 称 "newest first"，实际 push 追加为 oldest-first（property_table.rs 三处）；`set_version_chain_cap` 注释引用外部计划编号 | 低 | 二 |
| D13 | `version_chain_cap` 无配置管道：setter 仅测试调用，cap 只能走默认常量 | 低 | 二 |

遗留事项（前三项已在第二批修改中落地，见"二·补充"；第四项继续跟踪）：

- ~~维护触发三轨并行（写路径四级 / 后台线程 / 手动管线），职责边界需文档化收敛~~；
- ~~无活跃快照时 PhysicalDeletion 合并被显式降级为普通合并……需要运维出口
  （定期快照或显式 purge API）的设计决策~~；
- ~~段路径删除无 undo 配套（仅 mutable CSR 路径可回滚）~~；
- `PropertyTable::update()` 与 `compact()`/`compact_with_relocation()` 双轨并存。

## 二·补充 遗留事项修复设计（第二批）

### D14 维护触发三轨收敛

`CompactionMode` 枚举中 `AutoGC` / `PhysicalDeletion` 两个变体在生产代码中零调用
（仅 `Standard` 被 3 处使用），属于死配置面。收敛方案：

- 删除 `CompactionMode` 枚举，`compact_and_freeze(ts, config)` 成为唯一管线入口，
  步骤固定为 compact_csr → freeze → merge → compact_properties → tombstone GC → stats；
- 管线的回收强度不再由调用方指定模式，而是由**保留状态**推导
  （见 D15 的 `effective_retention_bound`）：
  - 保留边界有界（存在活跃快照或设置了 retention floor）→ 合并物理丢弃
    delete_ts <= 边界的边 + GC 早于边界的 tombstone；
  - 边界无界（无快照且未设置 floor）→ 合并保留全部边、跳过 GC，
    完整时间旅行历史不受影响——自动等价于原 `Standard` 行为；
- 写路径四级维护、后台线程、手动/管理触发全部经由同一管线，
  三轨差异只剩"何时触发"（阈值 / 后台策略 / 显式调用），不再有语义分叉。

### D15 无快照场景的回收出口（retention floor）

核心问题：`min_active_snapshot_ts == MAX`（无活跃快照）时，所有回收决策点把 MAX
当作真实时间戳处理会摧毁时间旅行历史；当作"无界"处理则删除永远不回收。方案：

- `MVCCManager` 新增 `retention_floor: Timestamp` 字段（默认 0 = 未启用，运行期
  状态、不持久化）与 `effective_retention_bound()`：有活跃快照时返回其最小值
  （快照始终优先）；无快照且 floor > 0 时返回 floor；否则返回 MAX；
- 全部回收决策点统一改用 effective bound：delta 压缩 cutoff（compaction/freeze）、
  tombstone GC（写路径 tier 1 与统一管线）、属性压缩与版本链 GC（tier 2 /
  compact_properties）、物理合并门控（tier 4 / auto_merge_segments / 统一管线）；
- 附带修正两个同族缺陷：
  - `gc_tombstones_batch` 不再用入参覆写 `min_active_snapshot_ts`
    （否则 floor 作为 GC 参数会污染真实的快照下限，后续快照注册只降不升导致过度回收）；
  - `PropertyTable::gc_versions(MAX)` 由"清空全部版本链"改为 no-op（MAX 不是
    时间戳，无界即无可安全回收项）；
- 运维出口 API 链：`TimeTravelEdgeStore::set_retention_floor` /
  `EdgeStore::set_retention_floor` / `GraphStorageContext::set_edge_retention_floor`
  （作用于全部分区）/ `GraphStorage::set_edge_retention_floor`。
  运营侧在确认不再需要某时间点之前的历史后设置 floor，再由常规维护管线完成回收。

### D16 段删除 undo

段路径删除（tombstone + 属性 mark_deleted，无 CSR 条目可回滚）此前在事务回滚时
静默失效。方案：

- `TimeTravelEdgeStore::revert_delete_edge_by_offset` 在 CSR 回滚失败后回退到段路径：
  新增 `segment_find_edge_any`（忽略 tombstone 的段内查找），命中且
  `delete_ts_of(edge_id) <= ts` 时执行撤销——`mvcc.remove_deletion` 移除两层
  tombstone、`properties.revert_deletion` 恢复属性行、置位 snapshot_dirty、
  重建属性索引条目；
- 时间戳守卫保证只撤销本删除点之前的删除（与 delta 路径 `revert_delete_by_offset`
  的 `delete_ts <= ts` 语义一致）；早于删除点的回滚请求返回 false 不产生副作用；
- 复用既有 `RemoveEdgeUndo` undo 记录结构，事务层零改动。

## 二、修复设计

### 阶段一（正确性）

**D1**：为 `MockStorage` 增加两个 trait 的桩实现，统一返回
`StorageError::not_supported`。Mock 是测试替身，不需要真实的批处理窗口能力；
实现后 `QueryStorage` 的 blanket impl 生效，`snapshot_handle()` 可用，
SyncWrapper 泛型约束满足。

**D2**：`merge_selected_segments_with_deletion_filter_with_free_space` 的空结果分支
改为同样移除并回收源段（`free_space.recycle_csr`），只是不再构建新段、返回 0。
上层 `edge_table.rs` 以 `segments_before != segments_after` 判定是否重建索引与快照，
天然兼容"只删不建"。空结果只可能发生在启用删除过滤且全部边被丢弃的场景，
不会误回收活数据。

**D3**：`PropertyTable::update()` 在标记删除前调用 `check_write_conflict(row_idx,
offset, ts)`。cold delta 应用路径受 `delta_ts >= snapshot_ts` 约束，属严格前向写，
不会被误拒。

**D4**：
- `PropertyTable` 新增 `retention_horizon: Timestamp` 字段（默认 `Timestamp::MAX`
  即"无快照保护需求"，保持现有行为）与 `set_retention_horizon`；
- `fold_oldest_versions` 仅当被折叠条目（chain[1]）的 `delete_ts <= retention_horizon`
  时才折叠——此时任何活跃快照 ts >= min_active >= delete_ts 都落在该条目可见区间
  之后，折叠不影响快照读取；否则跳过本轮折叠，允许链临时超限；
- 同步时机挂在快照注册的唯一入口：`TimeTravelEdgeStore::register_snapshot` /
  `unregister_snapshot`（edge_table.rs:479-485）及持久化加载点，注册后把
  `mvcc.min_active_snapshot_ts` 推给 properties。陈旧 horizon 只会偏低（保守方向），
  不影响正确性。

### 阶段二（死代码清理）

**D5**：删除 `buffer_manager.rs` 及 storage.rs 模块声明。
**D6**：删除 `striped_lock.rs`；移除 PropertyTable 的 `stripe_count` /
`stripe_locks` 字段、`PROPERTY_TABLE_STRIPES` 常量、`stripe_for_row` /
`stripe_count()` 方法、持久化字节（dump 尾部 4 字节与 load 对应解析）、
core.rs 的 `property_stripe_for_offset` 及相关测试断言。项目处于开发阶段、
无需向后兼容，持久化格式变更可直接落地。
**D7**：删除两个死配置字段及其 Default 初始化（所有字面量构造均带
`..Default::default()`，无破坏）。
**D8**：删除 gc_tombstones 中的诊断块。
**D9**：`set_property` 的预检下沉到慢路径分支（快路径已由
`set_property_fixed_size` 自检），消除重复检查且覆盖不变。
**D10**：删除 `compact()` 中未使用的 `offset_mapping`。
**D11**：日志改为分别统计组数与段数。
**D12**：修正三处链序注释；清理注释中的计划编号引用（代码注释仅描述意图）。
**D13**：`EdgeTableConfig` 增加 `version_chain_cap` 字段（默认 64），
构造函数接线到 `properties.set_version_chain_cap`。

## 三、验证

```shell
cargo check -p graphdb-storage --all-targets   # 测试目标必须恢复编译
cargo test -p graphdb-storage --lib            # 存储层单测
cargo check --workspace                        # 全 workspace 编译
cargo clippy --workspace                       # 无新增告警
```

新增测试：

- 整组全删合并：构造段内全部边已删除且早于活跃快照，合并后段列表缩短、CSR 已回收；
- 快照感知折叠：注册老快照后高频更新超过 cap，断言快照时间旅行查询结果不受折叠影响；
- update 冲突检查：对已删除行以更晚 ts 调用 update 返回冲突错误。
