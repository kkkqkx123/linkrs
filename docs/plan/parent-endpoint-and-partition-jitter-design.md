# 父端点重复解析与分区调度抖动收敛设计

**日期**：2026-11-23
**状态**：实施设计
**证据来源**：`perf-data/parity/results/2026-11-23/expand-read-convergence/`

## 1. 问题

上一轮拆分后，固定门的展开自耗时仍以拓扑读取为主，两条用例的 `expand_topology_read_us` 在 88–131 ms；每条查询只有 110 个父批，但每批约 1959 个父顶点，累计 215488 个父端点。回测还显示分区工作偏斜：最终版 `partition_work_us` 的 max/min 为 1.20–1.56，分区内工作时长变异系数最高约 0.13。

两条路径存在可拆掉的重复工作：

- 拓扑-only 扫描已经把 `(external id, internal id, label)` 作为 `VertexHandleColumn` 交给展开，但 `parse_seeds` 只识别 `VertexIdentity`。rowless 展开仍逐行把 `VertexHandle` 转成 `Value::VertexId` 再做一次 `VertexId::try_from`，同一批父端点被重复解析。
- `packed_node_edge_headers_batch` 每批都重新做 space 解析、edge type 列表枚举、边类型名排序与端点标签解析。固定门中 10 个分区几乎同时启动，这些调用叠加在同一组 catalog 读锁上；它们不读 MVCC 行，却会放大分区完成时间的抖动。
- `partition_skew_ratio` 已进入 parity JSON，但逐用例 gate 表格没有显示它，`wall_time` 与 `work_time` 也没有进入 JSON，无法区分“总工作变多”和“尾部分区变慢”。

## 2. 目标与护栏

- 消除同一查询内父端点的逐值解析和拓扑 schema 的逐批解析；解析结果只放在单次执行的 `ExpandCtx`，不跨查询、不做全局可变缓存。
- 缓存键必须包含 space、边类型集合和物理布局版本；布局版本不匹配时回退到原解析路径。
- 不改 MVCC、TTL、pending gate、staged write、边顺序和悬空端点语义；不缓存行可见性或端点存在性。
- 不调整分区数、worker 数、通道容量等调度配置；先减少并发启动时的共享元数据工作，再用偏斜指标判断是否还需要调度策略改动。
- 不改 CSR/属性页布局，不新增替代模块，不引入采样或近似。

## 3. 代码方案

### 3.1 父端点身份直达

`identity_seed_ids` 同时接受 `TypedColumn::VertexIdentity` 与 `TypedColumn::VertexHandle`，直接返回外部 id 派生切片。`parse_seeds` 的 rowless 与 row 两种入口都通过 `seed_vid` 使用它，逐值 `Value` 解析只保留给没有身份列的输入。`identity_seed_handles` 继续从同一 `VertexHandleColumn` 取内部 handle，两条路径共享同一批父端点数据。

### 3.2 拓扑 schema 单次解析

在 `linkrs-storage` 增加只读 DTO `TopologySchema`，内容包括 space 名、vid 类型、布局版本以及每个选中边类型的类型名、类型 ID、边标签、起止端点标签。解析时按既有规则过滤边类型、解析端点标签；端点标签缺失的边类型仍只Intern类型ID，不进入 CSR 读取。

`StorageReader` 增加 `resolve_topology_schema`，默认实现用现有只读 schema 接口构造 DTO，GraphStorage 走同一份解析逻辑。`get_packed_node_edge_headers_batch` 接收该 DTO；DTO 的 space 或布局版本与当前存储不一致时，存储层在本地重新解析，避免跨快照使用陈旧标签。默认实现保持非原生 reader 可用。

`ExpandCtx` 增加 `topology_schema` 缓存，生命周期与现有 `dst_tag_cache` 一致。`expand_single_step_columnar` 在进入拓扑读取前解析一次并传给批量读取；微基准在计时外解析一次，使 F1/F5 与查询层同口径。

### 3.3 调度抖动观测

parity runner 从 EXPLAIN 解析 `wall_time` 和 `work_time`（去掉 `us` 后缀），与已有的 `partition_skew_ratio` 一起写入逐用例 JSON。gate 结果表增加三列，避免把偏斜误判为算子退化。EXPLAIN 的 `partition_work_us` 与原并行诊断保持原样。

## 4. 正确性矩阵

改动必须逐值一致覆盖：空父批、单父、多父、重边、自环、平行边、悬空端点；Out / In / Both；空边类型过滤与缺失端点标签；staged insert / update / delete 与已提交边混合；快照早于/晚于提交边界；TTL 表、NULL 属性、多标签目的端；唯一率 100% / 50% / 10%；DQL 全量、并行分区执行、storage lib 全量与列批集成测试。分区并集与未切片全表结果必须一致，行序不变。

## 5. 验收门槛

- devs release 编译通过，正确性门全绿。
- `expand_read_us` 两轮中位数较上一轮最终版下降至少 10%；`expand_topology_read_us` 与 `scan_visibility_us` 同步下降。
- `partition_skew_ratio` 不高于 1.30，`parallel_wall_time_us` / `parallel_work_time_us` 比例不退化。
- `group_by_count ≤40 ms`、`distinct_limit ≤30 ms` 为最终门；未达成时记录实测值，不通过改配置或近似补门。
- 生产默认单轮的九条纯计数与 q1–q9 中位数退化不超过 10%。

## 6. 不做项

- 不改变 `parallel.enabled` 与分区/通道默认配置，不把 bench 参数推广为默认值。
- 不跨查询缓存 schema 或端点解析，不缓存任何可见性结果。
- 不把 Both 分支改成未证明等价的无去重直写，不做身份绕行列。
- 不为了缩短代码路径而放宽 closed-loop 形状闸门。

## 7. 复测结论

devs release 编译通过，正确性门全绿。微基准 F1（2048 父）由上一轮 2927 us / 6.55 MB 降到 2722 us / 6.55 MB，约 7%；相对最初 3395 us / 8.68 MB 下降约 20% / 25%。`TopologySchema` 缓存与父端点身份直读没有引入语义差异。

固定门在三次运行中受外部负载影响明显（devs load average 5–12），`scan_read_us` 在 61–257 ms 间波动，`partition_skew_ratio` 在 1.14–1.49 间波动。`expand_read_us` 为 150–197 ms，与上一轮 160–215 ms 重叠，未取得设定的 10% 下降；两条中间门也未通过。结论为改动保留、门槛未达成；在负载平稳窗口之前不再调整分区数、通道容量或 worker 数。逐轮日志见 `perf-data/parity/results/2026-11-23/endpoint-skew-convergence/`，问题记录见 `docs/issue/linkrs-remaining-parity-issues.md` 第 40 节。
