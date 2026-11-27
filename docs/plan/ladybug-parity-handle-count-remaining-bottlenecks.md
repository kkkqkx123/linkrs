# 句柄直通拓扑计数的剩余瓶颈与收敛方案

**日期**：2026-11-27  
**前置文档**：`docs/plan/ladybug-parity-handle-native-topology-count.md`

## 1. 现状判断

前序改造已经消除了逐边外部身份构造、row view、seed row 克隆，并把 `group_by_count` 安静窗口墙钟压到约 9.7–10.7ms。当前未通过的是 workload 结构门：`expand_topology_read_us` 约 32ms、`parallel_work_us` 约 96–128ms，目标分别是 25ms 和 80ms。墙钟在外部负载或偏斜升高时会回到约 14ms，尚不能宣称稳定 8ms。

前序文档已经完成并行工作量归因，排除了 exchange、分区打开/关闭、逐边可见性主导和哈希分桶聚合。因此本轮不再扩大语义面，也不再重试已回退的批量放大与哈希分桶。

## 2. 剩余瓶颈

按稳定轮次 `work_time≈96165us` 口径：

| 瓶颈 | 累计耗时 | 判断 |
|---|---:|---|
| 句柄拓扑 setup | 11929us | 单线程不足 1us，8 路并发放大到约 12ms。每批重复 schema 判定、边表目录查询、gate 准备和 recorder 触发的外部主键反解，是当前最可疑的可压缩项。 |
| 可见 CSR 遍历 | 14634us | 单线程与裸 degree 读取接近，是必须保留的真实工作量；可压缩的是每批句柄解析与边表选择常量，而不是 MVCC 判定。 |
| 排序聚合 | 5247us | 排序形态已优于哈希分桶。当前 packed key 的高 32 位是常量 label，可以用更窄的整数 key 完成等价排序。 |
| 唯一主键解析 | 约 10ms | 整数主键仍走 40 字节 `VertexId` 构造和哈希去重，属于不必要的宽类型间接层。 |
| PartialAggregate | 约 11ms | 权重路径仍做一次 preflight `HashSet` 再写 `HashMap`，同一批 key 被哈希两次。 |
| 扫描侧 | 约 33ms | row view 已消除，剩余主要是表加载与归并；`scan_visibility_us` 已达标，不应再做可见性证明。 |
| 偏斜与噪声 | 1.2–1.44 | 不是工作量门的首要矛盾；exchange 只有几十微秒。 |

## 3. 修改方案

### 3.1 复用固定快照的每分区读绑定

在每个 expand 算子实例内只创建一次上下文，不再每个输入 batch 重建 `ExpandCtx`。相同快照下把 schema 条目、读时间戳、near/far label 和边表 `Arc` 列表在分区打开时解析一次，后续 batch 复用。仍每批重新构造短生命周期 pending gate，仍逐批持有边表读锁并执行 MVCC 判定；不持有跨批锁，不做跨查询缓存。

### 3.2 消除无效 read recording

自动提交的 mutation recorder 对顶点/schema 读是 no-op，但读取器仍因为 recorder 存在而逐批把内部 handle 反解成外部 `VertexId`。给 recorder 增加读记录能力声明，自动提交返回 false，显式事务保持原行为。只在真正需要记录 read set 时反解外部主键。

### 3.3 借用已验证的 seed handle

当 seed handle 全部存在且 near label 匹配时，直接遍历 handle slice，不再为每个 batch 构造 `Vec<Option<(u32, LabelId)>>`。任一 handle 缺失或 label 不匹配时走原解析路径。

### 3.4 收窄聚合 key

句柄计数快路径已经保证单一边类型和单一远端 label；对其它 label 仍保留回退。此时把排序 key 从 `(label<<32)|endpoint` 的 `u64` 收窄为 endpoint `u32`，排序后补回已知 label。结果与现有排序 handle 顺序逐值一致。

### 3.5 整数主键原生解析

新增按内部 handle 批量解析整数外部主键的存储读取，保持 raw key 优先、按读时间戳回退、单 label 约束和负数拒绝语义。query 侧用 `i64` 完成唯一键合并与加权，再生成目标标量 `Value`；文本主键、多标签、无法解析或 TTL 条件不满足时回退原路径。存在性证明和 dangling-free 守卫不变。

### 3.6 聚合单趟写入

把 packed key 与 typed key 的 preflight `HashSet` 去掉，改为单趟 `HashMap::entry`：已存在直接累加，空位先完成内存预留再插入。保持内存记账失败时不改变状态。

### 3.7 扫描与并行参数

上述改动落地并复测后再判断扫描是否成为主导。只有证据明确时才评估扫描 shard scratch 复用；在 workload 进入目标区间前不增加分区或 worker，不修改扫描顺序契约。

## 4. 护栏

- 不采样、不近似、不截断，不改变 multiplicity。
- 不跳过读时间戳、pending gate、staged write、TTL 与 dangling-free 检查。
- 不恢复逐边外部身份构造、哈希分桶和批量放大。
- 不引入跨调用缓存；读绑定只属于一次执行的一个分区。
- 任何快路径不满足证明时回退现有 packed topology 路径。

## 5. 验收

正确性沿用既有结果门：`group_by_count [14|59]`、`distinct_limit [14]`、`join_two_hop [382018]`、`variable_path_count [400153]`，并要求 `result_stable=true`、`spill_rows=0`、`typed_group_hits=112/112`、零回退。

新增专项门覆盖：整数句柄去重与 `VertexId` 去重逐值一致、重复外部主键的 multiplicity、自动提交与显式事务的 read-set 行为、单类型/单方向/单远端 label 回退、空批与 staged write。

性能门先看 `expand_topology_read_us ≤ 25000us`、`parallel_work_us ≤ 80000us`，再在安静窗口复核墙钟是否稳定进入 8–12ms。高负载窗口只作参考。
