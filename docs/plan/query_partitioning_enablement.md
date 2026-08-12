# 分区规划启用前置条件方案

## 1. 现状分析

`optimizer/partitioning.rs`（1185 行）已实现保守的分区选择器：

- `PartitioningConfig` 默认**全关闭**（`partitioning.rs:32-43`）：
  - `enabled: false`
  - `min_rows_per_partition: 100_000`
  - `max_partitions: 1`，`max_workers: 1`
  - `vertex_id_range: None`（需调用方提供"信任的顶点 ID 域"，None 时拒绝分区）
- 支持能力：单标签顶点扫描分区、纯边表链（按 src-id 域切分）、多扫描
  （UNION/MINUS/INTERSECT/简单等值连接）共分区；递归/路径算法、写计划、
  跨事务计划一律拒绝
- 接入点：`OptimizerEngine::apply_partitioning_selection()`（optimize 末尾），
  成功则 `plan.set_partition_spec(spec)`，失败写入 `parallel_fallback_reason`
  （EXPLAIN 可见）
- **未完成标记**：`layout_signature()` 的 `PartitionSpec::layout_version` 目前是
  配置签名（注释明确"storage layer may supply a real monotonic version in a
  later phase"）；复杂源拓扑（多类型 join、非简单 key）被显式拒绝，等待物理
  规划器的显式 source-domain 映射

### 启用阻塞项

1. **信任的 ID 域来源缺失**：`vertex_id_range` 需证明覆盖扫描，当前无人提供
   （统计可估计工作量但不能证明覆盖；猜测整数全范围会静默漏掉非数值/稀疏 ID）
2. **layout_version 无真实单调版本**：存储布局变化（如段合并、数据搬迁）时
   计划缓存无法失效
3. **配置未接线**：无服务器配置项暴露
4. **物理计划编码缺失**：分区 spec 在 `StreamingExecutionEngine` 后置组装
   （`register_partitioned_root`），未直接编码进 `PhysicalPlan`

## 2. 方案设计

### 2.1 存储层提供单调布局版本

- `graphdb-storage` 暴露 `layout_version(): u64`（或段级版本），任何影响
  vertex/edge 分段布局的操作（段分配、合并、收缩）递增该版本
- `PartitioningPlanner::layout_signature()` 改由该真实版本参与签名
  （替代当前的纯配置签名），计划缓存指纹随之失效
- 实施方式建议：存储的 `StorageEngine` / 段管理器增加
  `Arc<AtomicU64>` 单调计数器，读路径零开销

### 2.2 元数据推导 vertex_id_range

不要求调用方信任输入，改为从存储/元数据自证：

- 若存储顶点 ID 为单调分配（序列分配器），由分配器当前水位推导
  `[1, watermark]` 域，并随 `layout_version` 一同读取
- 若 ID 为任意 i64（用户指定），分区规划仅在存在 ID 索引范围证据时启用
  （如主键空间登记表）
- 无证据时维持现状拒绝（默认关闭即安全）

### 2.3 配置接线

- `graphdb-config` 增加分区规划配置组（server 配置）：

```toml
[query.partitioning]
enabled = false            # 默认关闭，验证充分后默认开启
min_rows_per_partition = 100000
max_partitions = 8
max_workers = 4
```

- `PartitioningConfig` 改为从配置构建（保留 `Default` 全关闭语义）

### 2.4 物理计划编码分区 spec

- `PhysicalPlan` / `PhysicalOperatorSpec` 增加 `partition_spec` 字段，
  `PhysicalPlanBuilder` 从逻辑计划的 `PartitionSpec` 直接物化
- `StreamingExecutionEngine` 改为读取物理计划中的分区信息（保留
  `register_partitioned_root` 为兼容入口），消除后置组装

### 2.5 后续扩展（不在本期）

- Hash / Round-Robin 分区策略
- 分区级工作窃取（动态负载均衡）
- 复杂源拓扑的 source-domain 映射（依赖 2.4 完成后）

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 存储层单调 layout_version | `graphdb-storage`（段管理器/StorageEngine） |
| 2 | `layout_signature()` 接入真实版本 | `optimizer/partitioning.rs` |
| 3 | ID 域自证（分配器水位/主键空间） | `optimizer/partitioning.rs`, 元数据层 |
| 4 | 服务器配置接线 | `graphdb-config`, `partitioning.rs` |
| 5 | PhysicalPlan 编码 partition spec | `executor/streaming/plan/`, `engine.rs` |
| 6 | 默认开启 + 全量验证 | 配置默认值, `tests/` |

## 4. 验证方法

- 正确性：开启分区后与串行执行的**逐行结果一致**（现有
  `partitioning.rs` 已有 20+ 单测，补充跨配置回归）
- 计划缓存：变更布局（触发 layout_version 递增）后计划指纹变化，缓存失效
- 并行收益：benchmark 对比 `max_workers=1` 与 `max_workers=4` 的
  scan/join 吞吐
- 回退验证：`enabled=false` 行为与现状完全一致（默认值即回退开关）

## 5. 预期收益

- 文档 4.2.5 项落地：分区规划从"代码存在"到"可安全启用"
- P8 并行框架真正被配置驱动，scan/join 场景吞吐提升
- 计划缓存与布局变更一致性（layout_version 兜底）

## 6. 风险与回退

- **风险**：ID 域推导错误 → 静默漏行。缓解：默认关闭 + 域必须自证；
  生产开启前要求步骤 2/3 完成并有覆盖测试
- **风险**：layout_version 计数器遗漏递增点 → 陈旧缓存。缓解：版本递增
  集中在存储布局变更入口（单点审计）
- **回退**：配置 `enabled=false` 即完全回退，无代码回退成本
