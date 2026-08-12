# 遗留验证清单与低优先级小项 方案

> 状态：待实施。收集各方案文档文末「遗留」与分析文档低优先级
> 建议中的零散项，集中落地。

## 1. 验证类遗留（高优先，成本最低）

各方案文档因资源受限环境未跑 integration，以下验证待补：

| # | 验证项 | 来源 | 预期断言 |
|---|--------|------|----------|
| 1 | `cargo test -p graphdb-query --test '*'` 全量回归 | `query_transaction_snapshot.md` 遗留 | 全通过，含事务/反馈/分区 integration |
| 2 | `BEGIN READ ONLY` 会话级端到端 | 同上 | 同一只读事务内两语句读同一快照（插入数据后快照不变） |
| 3 | 只读事务内执行 DML 被拒绝 | 同上 | 返回错误（`TransactionScope::ensure_can_write` 路径） |
| 4 | SAVEPOINT 真实服务器端到端 | 本次核查 | `SAVEPOINT sp` → 写 → `ROLLBACK TO sp` → 数据恢复；`RELEASE SAVEPOINT` 后回滚报错。现有 `tests/transaction/http_api.rs:299-317` 是 mock 服务器，需真实链路用例 |
| 5 | 属性裁剪基准收益度量 | `query_property_pruning.md` 遗留 | benches 中对比裁剪前后 GetVertices/GetEdges/GetNeighbors 吞吐与内存 |

**落地方式**：`tests/` 新增/补强 integration 用例（复用
`integration_transaction.rs` 既有框架）；基准在 `benches/` 增补。

## 2. EXPLAIN 中间算子列级 projected 展示（低）

**现状**：`physical_plan_explain.rs:198-214` 仅在 **Source 层**
（`SourceSpec::StorageScanVertices/StorageScanEdges/GetVertices/GetEdges/
GetNeighbors`）展示 `projected` 列；Unary（AppendVertices/Project 等）
与 Join 等中间算子的列级消费信息不展示。

**方案**：

- 为 Unary 算子补充列级信息：`AppendVertices` 展示 `vertex_props`，
  Project 展示投影表达式列名（复用 metadata 布局，不重复计算）
- 输出格式：table 模式加列、json 模式加字段（`format.rs` 的
  `serializable_plan` 结构同步扩展）
- 验证：EXPLAIN 单测断言；回归说明不影响既有输出断言

**涉及文件**：`executor/explain/physical_plan_explain.rs`,
`executor/explain/format.rs`

## 3. 分区策略扩展与工作窃取（低，设计先行）

**现状**：`optimizer/partitioning.rs` 仅支持 **Range 均分**
（`split_range`，行 669：`Range<i64>` 按 partition_count 切分），
无 Hash / RoundRobin 分区；`MorselWorkerPool` 静态按分区分配，
无动态负载均衡。

**方案（设计先行，验证收益再实施）**：

1. **Hash 分区**：`PartitionSpec` 增加策略枚举
   （`Range | Hash | RoundRobin`），Hash 对顶点 ID 哈希取模——
   适用于 ID 域不连续（string-id 或稀疏）场景，弥补 Range 的
   `vertex_id_range` 依赖
2. **RoundRobin**：对索引扫描/无键源按行轮流分发
3. **工作窃取**：`MorselWorkerPool`（P8 并行框架）任务队列改为
   可窃取结构（worker 空闲时从其他 worker 队列尾部取任务），
   仅在有界通道背压稳定后评估
4. **门控**：沿用 `PartitioningLayoutInfo` 自证域；Hash 分区在
   string-id 域下也可启用（不依赖 Range 的数值假设）

**风险**：并行任务顺序语义（Gather 合并顺序）在窃取下需保持稳定；
仅当基准显示 Range 均分负载倾斜时推进 3。

**涉及文件**：`optimizer/partitioning.rs`,
`executor/streaming/partition.rs`（或 worker pool 所在模块）

## 4. 实施顺序

1. 第 1 节验证清单（高优先，回归安全网——先于新功能落地前跑通）
2. 第 2 节 EXPLAIN 列级展示（低风险增量）
3. 第 3 节分区策略（设计评审后分步实施）

## 5. 风险与回退

- 验证项无功能风险，仅耗时
- EXPLAIN 列级展示：输出断言兼容（新增列/字段为 additive）
- 分区策略：Hash/RoundRobin 作为新策略枚举值，不改变默认 Range 行为；
  工作窃取默认关闭（配置开关），负载倾斜基准不达标则不启用
