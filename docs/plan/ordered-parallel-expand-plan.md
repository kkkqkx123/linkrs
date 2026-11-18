# 有序并行展开与分区预聚合设计

日期：2026-11-18
对应问题：`docs/issue/linkrs-remaining-parity-issues.md`
当前基线：`perf-data/parity/results/2026-11-18/stage7-final/`

## 背景与结论

当前固定门中位数为 `distinct_limit` 305 / 313 ms、`group_by_count` 283 / 306 ms。PROFILE 中源顶点扫描约 57 ms，`ExpandAll` 累计约 0.27 s，后续 Project、Aggregate、Distinct、TopN 合计只有几十毫秒。也就是说，单条邻接读取路径已经经过多轮收紧，继续在存储接口上做局部改动的收益有限。

两次目的端半连接原型都出现明显回退，说明目的端唯一化与专用批量读在当前基数下不划算。当前最值得尝试的方向不是再改读取接口，而是释放多核能力：把扫描、展开和投影构成可并行的前段流水，把聚合与去重改成分区局部状态加最终合并，同时保持输出顺序与现有 blocking 算子语义。

devs 上 benchmark 显示 `requested_workers: 1, actual_workers: 1`。现有执行器已有 morsel 工作池和并行诊断字段，但当前计划形态只有一个源到 blocking 算子的流水线，没有可用的多子节点 Exchange，因此并行通道没有生效。

## 目标

- 在拓扑-only、closed-loop、行内无需求的展开形状上启用并行前段流水。
- 保持 aggregate / distinct / group-by / TopN 的结果、顺序与重复语义不变。
- 在 quiet window 固定门上取得可复现的显著收益；至少验证 2 倍级别提升，不预设已达到十倍硬门。
- 当形状不被证明安全、worker 数不足或资源受限时，自动退回当前串行路径。

## 设计

### 并行边界

并行范围限定在源扫描到 Project 之间。Blocking 算子仍作为计划边界：前段按 morsel 并行产出，进入 Aggregate / Distinct 之前做分区局部预聚合，最后由单线程确定性合并。

这样做的原因：

- 顶点扫描已有分区范围和分片信息，可作为自然任务粒度；
- 展开读取在同一查询快照下是只读的，适合多 worker 并发；
- 全局 HashMap 直接并发写会破坏确定性并引入锁竞争，不应采用；
- TopN / ORDER BY 依赖最终全局顺序，不能在局部提前截断。

### Morsel 调度

每个 worker 领取一批源顶点 morsel，沿现有 `VertexHandle → 拓扑 CSR → typed Project` 路径处理。morsel 携带单调序号，parallel stage 维护有界缓冲并按序号把结果交还下游。

缓冲上限由现有 chunk 数和字节预算控制。背压时 worker 暂停领取新任务，而不是无界堆积。任一条 morsel 出错或查询取消时，整批任务停止并向上传递原始错误。

### 分区局部状态

Aggregate 与 Distinct 不再让所有 worker 写同一个全局状态：

- `count`：每个分区维护局部计数，最终求和；
- `GROUP BY`：每个分区维护局部 group map，最终按键合并累加器；
- `DISTINCT`：每个分区维护局部去重集合，最终合并后保留首次出现顺序；
- `ORDER BY ... LIMIT`：只在最终合并后的数据上执行。

局部状态的数量与 worker 数或分区数绑定，不随 morsel 增长。局部状态仍需遵守既有内存预算与 spill 契约；若某个分区 spill，最终合并必须按既有 run 合并规则读取，不能绕过 spill 管理。

### 启用条件

只在以下条件同时满足时启用：

- 计划中前段是 closed-loop 单步展开，且已经证明为 rowless typed 路径；
- 下游是 narrow count、group-count 或 distinct 形状；
- 没有改变边顺序的路径语义、OPTIONAL MATCH 回退、聚合 FILTER、多键分组或跨行窗口语义；
- 存储 reader 的实现声明支持同一快照上的并发只读；
- worker 数大于一，且内存预算允许有界缓冲和局部状态。

其他形状保持当前串行路径，不做自动降级到未知语义的尝试。

### 观测

复用现有 parallel 诊断字段，至少补齐：

- requested / actual workers；
- parallel wall time 与 work time；
- buffered chunks 与 buffered bytes 峰值；
- 每个分区局部状态行数与 spill 次数；
- 串行回退原因。

PROFILE 中必须能看到 actual workers 大于一，否则性能结果不视为并行生效。

## 实施顺序

- 先为 parallel stage 定义 morsel 序号、有界缓冲、错误传播与取消契约，并用纯 typed chunk 的单测覆盖顺序恢复。
- 接入扫描到 Project 的前段流水，先从单个 narrow 形状开始，保持串行实现作为回退。
- 增加分区局部 count / group-count / distinct 状态，并实现确定性最终合并。
- 把 max_workers 与内存预算接入启用判定，在 materializer 中装配并行或串行执行形态。
- 补充 spill 场景与取消场景测试。
- 最后在 devs 上做完整正确性门与两轮固定门。

## 正确性不变量

- 同一查询快照内的 MVCC 判定不变；不引入跨调用可见性缓存。
- 局部状态只改变累加组织方式，不改变 NULL、重复边和空组语义。
- 输出顺序由 morsel 序号与最终排序共同决定；局部去重只减少重复，不改变首次出现顺序。
- 每个局部状态独立遵守内存预算；spill 后的合并结果必须与串行路径一致。
- 并行 worker 只做只读存储访问，不绕过事务暂存、TTL、pending gate 或权限检查。

## 风险与应对

- **锁竞争**：源顶点分片与边表读锁可能成为争点。先限定 worker 数并用绑核数据验证；若 work time 随 worker 数上升，回退到较小并行度。
- **偏斜**：按来源分区可能局部过热。优先复用现有顶点分片边界，必要时按 morsel 动态领取而不是静态绑定。
- **内存放大**：局部 group map 与有界缓冲会增加峰值。必须受现有预算约束，且 spill 行为纳入测试。
- **不确定性**：并行完成顺序不能影响结果。所有输出通过序号恢复，聚合与去重通过确定性合并。
- **语义覆盖不足**：只启用已证明等价的窄形状；任何未覆盖表达式、路径或窗口语义都走串行路径。

## 不做

- 不改变 `EdgeId` 权威、外部 MVCC 权威或边属性存储布局。
- 不把属性内联进 CSR，不复制可见性权威，不建立跨调用缓存。
- 不在本阶段改为 mmap 主存储或 buffer manager。
- 不在本阶段继续修改目的端读取接口；该方向已由两版原型证伪。
- 不为了并行提前执行 TopN 或 LIMIT。

## 验收

- devs release 编译成功。
- DQL 全量 319/319、streaming lib 414/414。
- 固定门两轮比较阶段七基线：`distinct_limit` 305 / 313 ms、`group_by_count` 283 / 306 ms。
- 九条纯计数无回归。
- PROFILE 显示 actual workers 大于一，且并行段 wall time 明显下降。
- 原始日志与 JSON 落在 `perf-data/parity/results/2026-11-18/` 下对应阶段目录。
