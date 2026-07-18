# graphdb-storage 存储模块分阶段改进方案

> 本方案基于 [LinkRS 存储模块不足分析](../analysis/linkrs-storage-gap-analysis.md) 和 [LinkRS 与 Ladybug 存储模块对比分析](../analysis/storage-comparison-analysis.md) 编制。
>
> 方案日期：2026-07。当前工作区已经存在部分 WAL batch、目录锁观测、snapshot 管理和索引 manifest 基础实现；这些能力只有在完成本方案规定的故障、并发和恢复验收后，才能视为阶段完成。

## 1. 总体结论

当前 `graphdb-storage` 的核心方向不需要推倒重来。内存优先、图原生 CSR、可变段冻结为不可变段、快照隔离和分层持久化，适合 LinkRS 的单机、中小规模、热数据图场景。与 Ladybug 的差距主要集中在工程完备性，而不是必须改成磁盘优先架构。

本方案的目标是把现有实现从“可以运行的内存优先存储”提升为“具备明确恢复语义、资源边界、后台维护能力和可运维性的内存优先存储”。重点依次是：

1. 修复崩溃恢复和数据损坏路径中的静默错误。
2. 为数据、索引、墓碑、快照和后台任务建立硬边界。
3. 消除全局锁、同步 freeze/compact 和长事务造成的不可预测停顿。
4. 让索引、CSR、Schema 变更能够在持续读写下逐步演进。
5. 用故障注入、指标和基准证明每个设计决策，而不是只依赖单元测试。

本方案与以下已有计划互为依赖关系：

- [Storage 架构分阶段修改方案](storage_architecture_refactoring_plan.md) 负责显式事务上下文、snapshot 数据源、原子 checkpoint、目录封装和 outbox。
- [存储、索引与同步架构迁移计划](storage_sync_architecture_migration_plan.md) 负责 WAL/outbox 闭环、typed index cursor、generation rebuild、manifest shard 和安全回收。

本文件补充物理存储、MVCC、资源管理、CSR、索引和 Schema 方面的整体决策；不复制已有计划的实现细节。若两个计划对同一组件有重叠，以“先形成一个可恢复的单一路径，再删除旧路径”为共同约束。

## 2. 范围和现状判断

### 2.1 保留的架构选择

| 领域 | 保留决策 | 原因 |
| --- | --- | --- |
| 总体存储 | 内存优先 + WAL + flush + checkpoint + snapshot | 保留热数据低延迟和 CSR 顺序遍历优势 |
| 图结构 | 多种可变 CSR + 冻结段 + 合并 | 已针对一对一、多边、带标签关系做了专门优化 |
| 并发隔离 | 快照隔离（SI） | 当前图查询以读为主，SSI 的写意图跟踪成本暂不值得 |
| 编码压缩 | 统计驱动的列编码选择器 + zstd 物理压缩 | 已覆盖字典、RLE、bit packing、FSST、ALP 等主要场景 |
| 索引有序语义 | 保留 B-tree 有序能力 | 范围、前缀和有序扫描比单纯等值查找更符合查询层需要 |
| 属性存储 | 顶点属性列存、边属性按现有实现保留 | 避免为了追随 Ladybug 而改变已有访问特征 |

### 2.2 必须改进的缺口

| 优先级 | 缺口 | 主要风险 | 计划阶段 |
| --- | --- | --- | --- |
| P0 | undo 仅驻留内存、事务边界不完整 | 崩溃后可能恢复未提交数据，破坏原子性 | 0、1 |
| P0 | WAL 损坏条目被跳过、LSN 早于 durable 状态 | 静默数据丢失或 checkpoint 引用无效 LSN | 1 |
| P0 | checkpoint 发布和 WAL 回收缺少统一安全协议 | 恢复时缺少完整数据或误删 WAL | 1 |
| P0 | VertexId 编码溢出静默变成错误值 | 顶点和边发生不可检测的数据损坏 | 1 |
| P1 | 全局目录写锁、同步 freeze/compact | 跨标签写入串行化，延迟出现长尾 | 3、4 |
| P1 | 长事务、墓碑和快照无资源上限 | GC 停滞，内存无界增长 | 2、3 |
| P1 | BTreeMap 索引无预算、flush/GC 持锁 | 索引 OOM，后台任务阻塞在线读写 | 2、5 |
| P1 | cache TTL 过长且失效粒度过粗 | 读到陈旧数据，破坏快照语义 | 2、3 |
| P1 | 超级节点容量翻倍、overflow 反复复制 | 幂律图产生大量浪费和写入延迟峰值 | 4 |
| P1 | Persistence 构造路径忽略用户配置 | 所有调优参数可能实际不生效 | 2 |
| P2 | 无自动碎片回收、删除间隙遍历低效 | 长时间运行后读放大和空间浪费 | 4、6 |
| P2 | Schema 不支持类型迁移和默认值回填 | 线上 Schema 演进需要停机或手工迁移 | 6 |
| P2 | 缺少生产级指标、故障演练和规模基准 | 不能判断是否安全，也不能定位停顿原因 | 7 |

分析文档中提到的“无组提交”“SI 而非 SSI”“多种 CSR”“不采用页式 BufferManager”等，属于当前定位下的合理取舍，不列为近期缺陷。组提交和磁盘页缓存只在基准证明当前架构无法满足目标时进入条件性后续阶段。

## 3. 目标不变量和设计决策

### 3.1 持久化和恢复

#### 决策 D1：WAL 以事务提交记录作为可见性边界

所有写入统一形成事务 batch，逻辑结构为：

```text
BEGIN(transaction_id)
REDO(transaction_id, operation)*
OUTBOX_INTENT(transaction_id, target)*
COMMIT(transaction_id, commit_lsn)
```

自动提交操作也必须走同一条 batch 路径。恢复分两步进行：先扫描并校验事务边界，再只重放具有完整 `COMMIT` 的事务。没有 commit 记录的事务视为 abort，不得恢复为可见数据。

这里不把独立的持久 undo 段作为第一阶段目标。活动事务的 abort 仍使用内存 undo；崩溃后的回滚通过“未提交 batch 不重放”完成。这个决策成立的前提是：checkpoint 只能导出固定读时间戳下的已提交状态，不能把未提交版本写入 checkpoint。若以后允许 checkpoint 包含未提交物理版本，则必须重新引入持久 undo 或事务本地 delta 文件，不能沿用当前方案。

#### 决策 D2：区分 accepted、durable、commit、checkpoint 四种 LSN

至少定义以下语义：

- `accepted_lsn`：进程已经接受并写入用户态缓冲区的末端位置。
- `durable_lsn`：WAL 写入并完成约定的同步后，重启可以依赖的位置。
- `checkpoint_lsn`：已发布 checkpoint 覆盖的 durable 位置。
- `commit_lsn`：事务 commit record 的末端位置，作为 outbox 和恢复的排序依据。

任何 checkpoint 或 WAL truncate 都只能使用 `durable_lsn` 及更早的安全边界。内存中的 `current_lsn` 不再被解释为“已经持久化”。

#### 决策 D3：WAL 损坏默认 fail closed

每条 WAL 记录增加格式版本、长度、事务 ID、操作类型和记录级 CRC。恢复行为分为三类：

1. 文件末尾存在可证明的半条记录，且此前边界和 checksum 全部有效：可以截断到最后一个完整记录，并记录告警。
2. 中间位置出现长度、版本或 checksum 损坏：恢复失败，保留原文件和损坏位置，不能静默跳过。
3. 业务 payload 反序列化失败：恢复失败，并输出事务 ID、LSN、操作类型和格式版本。

可以额外复制损坏字节到死信文件用于审计，但死信文件不能成为“继续恢复”的理由。启动时应提供只读诊断模式，供人工导出信息，不提供默认的有损恢复。

#### 决策 D4：checkpoint 采用不可变目录 + 原子 manifest

目标状态机为：

```text
Capture(read_ts, durable_lsn)
    -> Pin committed snapshot
    -> Write checkpoint_<seq>.tmp
    -> fsync files and metadata
    -> Atomic rename to checkpoint_<seq>
    -> fsync parent directory
    -> Publish manifest
    -> Truncate WAL to common safe_lsn
```

manifest 必须包含格式版本、checkpoint sequence、读时间戳、WAL LSN、文件清单、文件大小、checksum 和各组件 epoch。WAL 截断不能早于 manifest 发布和持久化。snapshot 只能从已经发布的不可变 checkpoint 创建，不能直接复制仍会变化的主 data 目录。

### 3.2 MVCC、事务和并发

#### 决策 D5：继续使用 SI，但不以“第二老快照”推进 GC

最老活跃快照仍是安全水位线。即使最老快照超时，也不能直接 GC 到第二老快照，因为这会删除最老快照仍可能读取的版本。正确的处理顺序是：

1. 记录快照创建时间、所属连接、事务 ID 和当前操作。
2. 达到告警阈值后停止继续创建不受控的长快照，并暴露诊断信息。
3. 达到硬超时后，由事务管理器拒绝该事务后续操作并释放其快照。
4. 在事务管理器不支持强制终止前，GC 只能等待安全水位线，不能牺牲一致性。

初始生产默认值建议为最多 1000 个活跃快照、最长 300 秒；分析任务可通过显式配置获得更长生命周期，但必须占用独立资源配额。

#### 决策 D6：目录锁和表锁分层

`GraphDataStore` 的目录只负责 label、表句柄和反向映射；表数据放在独立的 `Arc` 句柄和表内锁中。目录 map 锁只在查找、创建和删除句柄时持有，不能覆盖顶点或边的实际读写、序列化和 compaction。

锁顺序固定为“目录元数据 -> 表句柄 -> 表内索引”，禁止持有一个表锁时无序取得另一个表锁。涉及多表操作时使用显式 catalog transaction 或按稳定 key 排序取得锁。不会继续向调用方暴露多个原始 `HashMap`/`RwLock` 组合。

#### 决策 D7：墓碑 GC 有上限并且增量执行

保留热层 HashMap + 冷层有序结构，但增加条目数和估算字节数双重预算。GC 采用固定 batch 和游标，后台调度；在线操作只负责记录压力，不执行完整 O(n) 扫描。达到 soft limit 时加速 GC，达到 hard limit 时对删除写入施加背压或返回资源耗尽错误。

`compact_with_ts` 不得忽略传入时间戳。更安全的接口是由维护器计算 `safe_gc_ts` 后调用只接受安全水位线的 compact 方法，删除条件和 MVCC 可见性规则在同一处定义。

### 3.3 内存、缓存和后台任务

#### 决策 D8：建立统一内存准入账本

数据 CSR、冻结段、属性列、索引、墓碑、cache、WAL buffer 和后台构建临时空间都必须向统一 memory accounting 报告。至少拆分为 data、index、mvcc、cache、background 五类，不能只限制可变 CSR 而放任索引增长。

采用 soft/hard 两级水位：建议 80% 触发后台 flush/freeze/GC，95% 触发写入背压。所有后台任务使用预算，不能因为 compaction 或 index rebuild 绕过总预算。

#### 决策 D9：cache 必须版本感知，写入采用精确失效

缓存键至少包含表标识、实体 ID 和版本信息，命中后仍需按 read timestamp 做可见性判断。写入、删除和 Schema 变更按实体或列精确失效，不再只按 label 粗粒度失效。默认 TTL 从 3600 秒降为 60 秒，TTL、TTI 和容量均由配置控制；缓存不可作为事务正确性的来源。

### 3.4 CSR、索引和 Schema

#### 决策 D10：超级节点使用分块 overflow，不使用不断翻倍复制

保留小度数顶点的内联/主块优化。超过阈值后使用固定大小的 overflow chunk 或 page 链，新增边追加到新 chunk，不复制整个旧 overflow。compact 时才将多个 chunk 重排为连续 CSR。初始建议 `max_inline_degree = 65536`、`overflow_chunk_edges = 4096`，实际值由基准调整并写入配置。

#### 决策 D11：索引先演进为有序分段存储，不直接复制 ART

保留 BTreeMap 的有序 key 语义，但把单一无界 map 演进为“active memtable + immutable index segments + manifest”。flush 通过交换 active/inactive 所有权后在锁外序列化；查询合并多个有序段并按 MVCC 去重。索引阶段必须实现精确、范围和前缀扫描。

本方案暂不引入 Ladybug 的 ART + 线性哈希组合，也不直接引入 `sled` 作为隐式存储后端。等有基准证明等值查找成为瓶颈后，再增加独立 hash index 变体；等索引超出内存预算后，再实现明确的 page/segment 回收协议。

#### 决策 D12：Schema 迁移使用版本状态机，转换失败不静默截断

属性类型迁移采用 `Pending -> Backfilling -> CatchingUp -> Active -> Retired` 状态机。新写入按照迁移状态写入新版本列，后台按固定快照回填，读取根据快照选择旧列或新列。数值、字符串和时间类型转换必须有明确规则；无法转换的值进入错误报告并阻止发布，不得使用默认零值掩盖错误。

新增属性可以声明默认值和是否允许 null。默认值在 Schema metadata 中持久化，回填和新写入使用同一份定义。删除旧列必须等待所有引用旧 Schema 版本的快照结束。

## 4. 阶段总览

| 阶段 | 主题 | 优先级 | 主要依赖 | 完成标志 |
| --- | --- | --- | --- | --- |
| 0 | 基线、故障注入和不变量 | P0 | 无 | 缺陷可稳定复现，基线可重复 |
| 1 | WAL、恢复、checkpoint 和编码安全 | P0 | 阶段 0；复用既有 checkpoint 计划 | 崩溃恢复只产生完整已提交状态 |
| 2 | 资源预算、配置契约和可观测基础 | P0/P1 | 阶段 0 | 所有主要资源有上限并能观测 |
| 3 | MVCC、事务上下文和目录并发 | P1 | 阶段 1、2；既有架构计划阶段 1/4 | 长事务、跨表并发和锁顺序有明确行为 |
| 4 | CSR freeze、overflow、compact 和迭代 | P1 | 阶段 2、3 | 写入不同步执行长维护，读写长尾受控 |
| 5 | 有界、可扫描、可恢复的索引 | P1 | 阶段 1、3；既有同步计划阶段 4/5/6 | index 与表在任意快照一致 |
| 6 | Schema 迁移和顶点迭代优化 | P2 | 阶段 3、4 | Schema 可在线演进，删除间隙不造成线性浪费 |
| 7 | 全链路验证、性能和运维收尾 | P2 | 阶段 1 至 6 | 故障演练、指标和基准完整 |
| 8 | 超内存数据集扩展（条件性） | P3 | 阶段 7 的基准结论 | 只有目标场景确实需要时才启动 |

阶段 1、2 可以并行设计，但阶段 1 的恢复协议必须先冻结，阶段 2 才能定义 WAL、checkpoint 和后台空间的预算。阶段 5 不得在阶段 1 的事务 commit LSN 和阶段 3 的快照语义未稳定前发布新的索引 generation。

## 5. 阶段 0：基线、故障注入和不变量

### 5.1 修改内容

1. 对分析文档中的每项发现重新对应当前代码、测试和调用链，标记为“未开始、部分实现、已闭环”。
2. 为 WAL、恢复、checkpoint、snapshot、freeze、compact、index rebuild 和 Schema migration 建立统一 fault point。故障点只在测试支持配置下编译。
3. 增加 `verify_invariants` 测试辅助能力，至少检查：
   - label name、label ID、table map 和 edge reverse index 一致；
   - 顶点、边、索引在同一 snapshot 下结果一致；
   - WAL checkpoint LSN 不超过 durable LSN；
   - 已发布 manifest 引用的文件完整且 checksum 正确；
   - active snapshot、墓碑和内存账本计数不出现负值或越界。
4. 固定并发、长事务、超级节点、删除间隙、索引百万条目和损坏 WAL 的最小复现数据集。

### 5.2 验收标准

- 现有 storage 单元、集成和恢复测试在不改变实现的情况下可重复通过。
- 每个 P0 缺陷至少有一个在当前代码上失败的回归测试，或有明确的“当前代码已修复但尚未完成压力验证”说明。
- fault point 不进入普通生产路径。
- 生成一份当前实现矩阵，作为后续阶段的起点，不能以分析文档中的旧行号替代代码验证。

## 6. 阶段 1：WAL、恢复、checkpoint 和编码安全

### 6.1 修改范围

- `crates/graphdb-transaction` 的 WAL record、writer、reader 和 recovery 类型。
- `graphdb-storage/src/storage/engine/transaction/{wal_manager,recovery,undo}.rs`。
- `persistence_coordinator.rs`、`snapshot_manager.rs` 及其恢复测试。
- `edge_table/core.rs`、undo/recovery 中的 VertexId 编码路径。

### 6.2 实施顺序

1. 冻结新的 WAL wire format 和版本号，所有自动提交、显式事务、Schema、索引和 outbox 事件统一走 transaction batch。
2. 将恢复从“读到一条就立即应用”改为“校验事务边界后只应用完整 commit batch”。
3. 将 LSN 分成 accepted、durable、commit 和 checkpoint 语义，并移动 durable 状态更新到同步成功之后。
4. 增加记录级 checksum、严格的长度校验和 fail-closed 错误类型。
5. 按既有 checkpoint 计划完成临时目录、文件 fsync、原子 rename、manifest 发布、目录 fsync 和安全 WAL truncate。
6. snapshot 只接受已发布 checkpoint 路径，并验证 metadata 中的 LSN、timestamp、文件清单和 checksum。
7. 所有 `unwrap_or(0) as ...` 形式的 VertexId/endpoint 编码改为显式 `StorageError`。本阶段保留当前 ID 宽度，不因个别溢出立即扩大到 128 位。

### 6.3 恢复测试矩阵

必须在以下位置注入进程崩溃或 I/O 错误，并重新打开数据库：

- REDO 写入前、REDO 写入中、COMMIT 写入前、COMMIT 写入中、fsync 后。
- checkpoint 数据文件、metadata、manifest 发布和 WAL truncate 前后。
- 顶点写入、边写入、索引事件和 Schema 事件之间。
- WAL 中间记录 checksum 损坏、尾部半条记录、未知版本和 payload 解码失败。

### 6.4 阶段验收

- 恢复结果只能是完整的旧状态或完整的已提交状态，不出现半事务。
- 未提交 WAL 不会被恢复为可见数据，已发布 checkpoint 不包含未提交状态。
- 中间损坏会明确失败，只有可证明的尾部半条记录可以按规则截断。
- WAL truncate 不会早于最新有效 checkpoint 的共同 safe LSN。
- VertexId 溢出在写入、undo、恢复和索引路径中都返回错误。

### 6.5 阶段 0–1 执行记录（2026-07-18）

| 项目 | 当前状态 | 代码/测试证据 |
| --- | --- | --- |
| 现有 invariant 基线 | 已接入现有基线 | `DataStore`、`VertexTable` 已有 `verify_invariants`，Schema 操作和 storage 测试继续复用该检查 |
| checkpoint 故障边界 | 已覆盖核心边界 | `PersistenceFaultPoint` 覆盖临时目录写入、metadata、fsync 和发布前后；失败时不得留下已发布 manifest |
| WAL 默认恢复策略 | 已完成 | 默认 `WalRecoveryMode` 为 `AbortOnCorruption`；非法 header、未知操作、checksum、解压和 LSN 链错误 fail-closed |
| torn tail | 已完成 | 仅允许文件末尾可证明的半条记录被忽略，并累计 `corrupted_count`；此前完整记录仍可恢复 |
| durable/checkpoint LSN | 已完成 | checkpoint 先同步 WAL，再读取 `durable_lsn`；truncate 拒绝超过 durable LSN 的位置 |
| checkpoint 清理 | 已完成 | 启动和复用 sequence 时清理没有已发布 manifest 的陈旧目录 |
| VertexId 编码安全 | 已完成 | undo 路径使用显式整数和 `u32` 范围校验，不再以 `unwrap_or(0)` 静默降级 |
| WAL 单元验证 | 已通过 | `cargo test -p graphdb-transaction --lib -- --nocapture`：196 passed |
| storage 集成验证 | 待解除工作区阻塞 | 当前索引 manifest API 的已有调用方不匹配（`manifest_epoch` 和构造参数数量），未覆盖或回退该用户修改 |

因此，阶段 0–1 的 WAL、恢复和 checkpoint 核心实现已完成；完整 storage 集成验收须在索引 manifest API 冲突修复后重新执行。该冲突不改变本阶段的恢复协议设计，也不能以跳过 storage 编译作为最终验收。

## 7. 阶段 2：资源预算、配置契约和可观测基础

### 7.1 配置重整

将分散的配置收口到 `PropertyGraphConfig` 的明确子配置，并确保 `new_with_persistence`、测试构造器和 server 初始化都使用调用方传入的配置，不再隐式创建 `default()` 覆盖用户设置。

建议增加以下配置项。表中数值是第一轮基准的起始值，不是永远固定的公共契约：

| 配置 | 起始值/策略 | 作用 |
| --- | --- | --- |
| `max_memory_bytes` | 必须为有限正数 | 限制数据、索引、MVCC、cache 和后台总量 |
| `index_memory_bytes` | 总预算的独立子预算 | 防止索引挤占数据内存 |
| soft/hard memory ratio | 0.80 / 0.95 | 触发后台维护和写入背压 |
| dirty flush | 50,000 次、64 MiB 或 30 秒任一先到 | 取代每 1,000 次操作单独触发 flush |
| `max_active_snapshots` | 1,000 | 防止 snapshot registry 无界增长 |
| `max_snapshot_age` | 300 秒 | 长事务告警和最终拒绝边界 |
| `max_tombstones` | 1,000,000 条并叠加字节上限 | 限制冷层增长 |
| `index_gc_batch` | 10,000 条 | 限制单次 GC 持锁时间 |
| `operation_timeout` | 30 秒 | 限制 freeze、merge、flush 等后台操作 |
| `record_cache.ttl` | 60 秒 | 缩短默认陈旧窗口，仍由精确失效保证正确性 |
| `compression` | zstd level 3 | 保留默认平衡档，后续按基准开放 lz4/snappy |

生产构造函数对零值、反向阈值、超过总预算的子预算和不支持的压缩配置返回错误。测试可以使用显式的低预算配置，不能依赖“无限内存”默认值验证生产行为。

### 7.2 资源和指标

实现统一 memory accounting 和后台 scheduler，至少提供：

- 各类别当前/峰值字节数和预算。
- flush、freeze、merge、index GC 的队列长度、运行时长和失败原因。
- active snapshot 数量、最老年龄、最老事务 ID。
- hot/cold tombstone 数量、估算大小和最近 GC 进度。
- WAL accepted/durable/checkpoint LSN、恢复耗时和损坏计数。
- cache 命中、精确失效、过期和容量驱逐。

### 7.3 阶段验收

- 修改配置后，flush、WAL sync、cache、freeze 和 checkpoint 的行为确实改变，并有测试证明。
- 达到 soft limit 时产生可观测事件并启动后台维护；达到 hard limit 时写入得到确定的背压错误。
- 所有新指标在普通 storage 测试中不会引入全局共享状态或线程泄漏。

### 7.4 阶段 2 执行记录（2026-07-18）

本轮完成了阶段 2 的配置契约、资源账本、主要存储组件观测和写入准入闭环。设计决策与实现对应关系如下：

| 决策 | 实现 | 验收证据 |
| --- | --- | --- |
| 资源配置必须有明确归属 | `PropertyGraphConfig.resources` 收口总内存、索引、快照、墓碑、GC、维护超时、dirty flush 和 cache TTL/TTI；`graphdb-config::StorageConfig` 提供同名外部配置 | `graphdb-config` 全部单元测试通过；非法预算关系返回配置错误 |
| 持久化构造不得覆盖调用方配置 | `PersistenceConfig.property_graph_config` 携带内存优先存储配置；`GraphStorage::new_with_persistence`、`open_with_config`、server startup 使用该配置 | `persistent_storage_retains_caller_resource_config` 通过 |
| 内存必须统一记账 | 新增 `MemoryAccounting`，拆分 data/index/mvcc/cache/background，记录当前值、峰值、soft 事件和 hard 拒绝；index 使用运行时 generation/shard 的估算字节数，cache 使用 Moka weighted size | resource budget 3 项测试通过；`GraphStorage::resource_snapshot` 可导出快照 |
| hard limit 必须产生确定行为 | 所有主要 `StorageWriter` 入口在修改前刷新账本并执行 admission；总预算、索引预算和 native index 墓碑数量/估算字节数超限返回 `CapacityExceeded` | 账本拒绝测试通过；完整 storage 单元测试通过 |
| cache 不得无界且必须可观测 | 默认 TTL 调整为 60 秒；缓存配额拆分后总量不再因 high-priority 加成超过上限；记录 hit/miss、insert、expiration、eviction 和精确失效计数 | `test_cache_metrics_include_insertions_and_precise_invalidations` 通过 |
| WAL durability 必须可观测 | `WalMetrics` 暴露 accepted/durable LSN、sync 成功/失败次数；checkpoint 继续使用同步后的 durable LSN | WAL metrics 单元测试通过 |
| dirty flush 起始值统一 | 默认 flush threshold/interval 调整为 50,000 次、30 秒；持久化 server 配置显式传递 checkpoint interval，避免隐式默认值覆盖 | `cargo check -p graphdb-api --lib` 通过 |

阶段 2 本轮验证结果：`cargo fmt --all -- --check`、`cargo test -p graphdb-storage --lib -- --nocapture`（524 passed）、`cargo test -p graphdb-config --lib -- --nocapture`（57 passed）、`cargo check -p graphdb-api --lib` 均通过。

边界说明：当前账本对数据、索引和 cache 采用诊断/写入准入时的估算刷新；后台 scheduler 的真实队列和维护耗时指标、跨所有表的 tombstone 精确字节统计以及快照年龄强制终止仍属于阶段 3/7 的扩展，不能在本记录中宣称已经完成。`max_active_snapshots` 已进入契约并提供 admission API，最终由事务上下文统一调用。

## 8. 阶段 3：MVCC、事务上下文和目录并发

### 8.1 修改内容

1. 依照既有架构计划删除 storage 实例上的全局事务上下文，使用显式 `StorageOperationContext` 或绑定 handle 传递 read timestamp、transaction ID 和 write timestamp。
2. cursor 创建时固定 read timestamp，生命周期内不从 storage 全局状态重新读取。
3. 将 `GraphDataStore` 迁移为目录 map + `Arc<TableHandle>`；目录只管理元数据和句柄，表操作不持有目录写锁。
4. 为跨表事务定义固定的 catalog 操作顺序，并删除调用方直接组合多个原始锁的 accessor。
5. 为 snapshot 注册增加数量、年龄、连接/事务归属和显式 release；过期事务由 transaction 层终止，storage 不做不安全 GC。
6. 将 tombstone GC 改为按游标的增量任务，修复 forward batch 填满后跳过 reverse index 的问题。
7. 修复 compact 的时间戳契约：要么只接受已经计算好的安全水位线，要么在内部严格验证 `delete_ts` 与活跃 snapshot 的关系，删除带有误导性 `_ts` 参数的接口。
8. 缓存键加入版本/快照信息，写路径按实体和 Schema 版本精确失效。

### 8.2 阶段验收

- 至少 8 个并发事务在同一 storage 上使用不同 read timestamp，互不串扰。
- 两个不同 label 的并发写入不会因为目录锁串行化；Schema create/drop 与数据写入压力测试无死锁。
- 长事务达到告警和硬超时后行为可预测，GC 不越过它仍需要的版本。
- 任意快照下，cache 读取与直接表读取结果一致。
- tombstone GC 单次工作量有上限，持续写入时冷层不会无界增长而无告警。

## 9. 阶段 4：CSR freeze、overflow、compact 和迭代

### 9.1 CSR 改造

1. 保留现有 6 种 CSR 变体和冻结段格式，先统一它们的内存统计、快照导出和维护接口。
2. 将超级节点 overflow 改为固定 chunk/page 链，新增边不再反复复制完整 overflow。chunk 大小和单顶点内联上限可配置。
3. 写入超过 soft memory threshold 时只提交“需要 freeze”的任务，由后台 worker 处理；写入进入新的 delta CSR，不在用户写调用中执行 freeze -> merge -> compact -> sparse index rebuild 全链路。
4. 达到 hard threshold 时允许短暂阻塞或返回明确背压错误，但必须有 operation timeout 和指标。
5. compact 使用新 CSR/新 immutable segment 构建完成后再发布；读者通过旧 segment handle 继续读取，发布后再安全回收旧对象。
6. 根据 fragmentation ratio、墓碑比例、段数量和最小绝对大小自动调度 compact，避免小表频繁 compact。
7. 顶点迭代维护有效 ID bitmap。由于外部 ID 稳定性优先，不使用可能复用内部 ID 的简单 freelist；删除间隙由 bitmap 跳过。

### 9.2 阶段验收

- 超级节点在 10 万、100 万边规模下不会因连续倍增产生数量级溢出容量。
- 写入延迟不包含不可预测的完整 freeze/compact；后台队列满时出现可解释的背压。
- compact 期间长读可以完成，发布前后同一 snapshot 的结果一致。
- 碎片超过阈值后能自动调度，compact 失败不会丢失旧段。
- 大量删除间隙的 VertexIterator 只遍历有效 ID，不再扫描整个 `[0, total_count)` 空间。

## 10. 阶段 5：有界、可扫描、可恢复的索引

### 10.1 目标结构

将 `GenericIndexManager` 的单一双向 BTreeMap 逐步迁移为：

```text
IndexCatalog
  └── IndexGeneration
       ├── active memtable
       ├── immutable in-memory segments
       ├── persisted segments
       └── manifest(epoch, generation, safe_lsn, checksum)
```

正向和反向索引可以继续独立维护，但必须共享事务 batch、MVCC timestamp 和 generation 状态。active memtable 达到 soft budget 后通过所有权交换进入 immutable 队列；序列化和 merge 在锁外进行。

### 10.2 修改内容

1. 增加 `scan_range`、`scan_prefix`、边界 inclusive/exclusive 和稳定排序 API，避免 query 层扫描后再过滤。
2. 引入 index memory budget、immutable segment 数量上限和后台 merge；超出 hard budget 时触发写入背压。
3. 将全量 BTreeMap flush 改为 snapshot/交换后序列化，在线读写不持有长时间读锁。
4. 将 index tombstone GC 改为 forward/reverse 都推进的增量算法，记录 cursor 和进度。
5. 复用既有迁移计划中的 `OrderedKeyCodec`、typed predicate、`IndexRow`、generation rebuild、publish fence 和 manifest shard；index rebuild 必须记录 `snapshot_ts`、`start_lsn`、catch-up frontier 和发布 epoch。
6. 索引写入、表数据和恢复 redo 使用同一事务边界；外部 fulltext/vector 索引继续通过 outbox，不与 native index 混用确认语义。
7. 本阶段不直接改成 ART 或线性哈希。只有在等值查询基准持续成为瓶颈时，才新增可选 hash index，并明确它不提供范围/前缀能力。

### 10.3 阶段验收

- 百万级索引条目在预算内运行，超预算有 flush、merge 或背压，不以 OOM 作为控制手段。
- 精确、范围、前缀、limit/offset 查询在任意 read timestamp 下与表扫描结果一致。
- flush、GC 和 merge 不长时间阻塞在线写入。
- rebuild 期间持续写入，发布后不丢失 `start_lsn` 之后的变更；任意 crash point 只能恢复到旧 generation 或完整新 generation。
- split 前后全局顺序、覆盖列和 MVCC 结果一致；长读持有旧 manifest 时旧文件不回收。

## 11. 阶段 6：Schema 迁移和顶点迭代优化

### 11.1 Schema 迁移

在 `vertex_table/schema.rs` 及其 persistence/recovery 路径中增加版本化 migration record，支持：

- `ALTER PROPERTY TYPE` 的显式转换规则。
- `ADD PROPERTY ... DEFAULT ...` 的默认值持久化和回填。
- 后台 backfill 的固定 snapshot、进度、重试和失败报告。
- 新旧列并存期间的读取选择和写入路由。
- migration publish 前的校验与 crash recovery。

禁止在未定义转换规则时自动 `as` 转换、静默截断或用 0/null 覆盖错误。默认值不等同于转换失败的兜底值。

### 11.2 阶段验收

- 类型迁移期间普通读写继续可用，读取结果与对应 snapshot 一致。
- 任意回填 crash point 重启后可继续或回滚，不产生半列。
- 不可转换值可定位到实体 ID、属性名和原始值，且不会错误发布新 Schema。
- 大量删除顶点后的迭代性能由有效 ID bitmap 测试证明。

## 12. 阶段 7：全链路验证、性能和运维收尾

### 12.1 故障和恢复演练

至少完整演练以下场景：

1. WAL 在 commit 前后崩溃。
2. checkpoint 在每个 fsync/rename/truncate 边界崩溃。
3. snapshot、manifest、索引 segment、Schema migration 文件 checksum 损坏。
4. checkpoint 期间持续写入并同时运行长读。
5. outbox SQLite 丢失、回退、重复投递和 dead-letter。
6. index rebuild/split 在持续写入和长读下崩溃。
7. 内存达到 soft/hard limit，验证后台维护和背压恢复。

### 12.2 基准

建立改造前后可重复的基准，至少记录：

- 单条和批量顶点/边写入吞吐与 p50/p95/p99 延迟。
- 不同 label 并发写入和读写混合吞吐。
- 一跳邻接遍历、范围/前缀索引查询和 cache 命中延迟。
- checkpoint、freeze、compact、index merge 的耗时、写放大和暂停时间。
- WAL recovery 吞吐、最大恢复时间和恢复后空间。
- tombstone、snapshot、outbox、index backlog 的增长和回收速度。

### 12.3 完成定义

- `cargo fmt --check` 通过。
- `cargo test -p graphdb-storage --lib -- --nocapture` 通过。
- `cargo test -p graphdb-storage --test '*' -- --nocapture` 通过。
- `cargo check --workspace --features server,fulltext-search,c-api,grpc,qdrant` 通过。
- `cargo clippy --all-targets --all-features` 通过。
- 关键故障注入、并发、恢复和资源背压测试全部通过。
- 所有旧路径、兼容 adapter、静默错误分支和不再成立的测试 fixture 已删除；不长期保留双实现。
- 运维文档记录 WAL 损坏、长事务超时、dead-letter、背压、checkpoint 回退和 Schema migration 失败的处置方式。

## 13. 阶段 8：超内存数据集扩展（条件性）

Ladybug 对比中最明显的长期优势是 4KB 页面、BufferManager 和磁盘优先的超内存数据集能力。但这不是当前 LinkRS 的第一目标，也不应在尚未完成资源账本和后台维护前直接引入裸指针页面管理。

只有阶段 7 的基准证明“目标场景经常超过可接受内存容量”，才启动本阶段。届时单独立项比较以下方案：

1. 保持内存优先，只将冷 immutable segment 按块 mmap/按需加载。
2. 为 index segment 增加 page cache 和显式 pin/unpin。
3. 对全量数据采用 shadow paging 或更完整的磁盘优先存储。

选择标准是恢复语义、实现复杂度、内存上限、冷数据延迟和写放大，而不是简单追求与 Ladybug 的组件名称一致。在没有基准和故障模型前，不引入新的 unsafe 页面层，也不把 swap 视为存储引擎的容量方案。

## 14. 阶段间强约束

1. 阶段 1 完成前，不删除现有可恢复 WAL 路径，也不声明任意 WAL 损坏可自动恢复。
2. 阶段 1 完成前，不能回收只由进程内状态证明安全的 WAL。
3. 阶段 2 完成前，不允许索引、墓碑、snapshot 或后台临时文件无限增长。
4. 阶段 3 完成前，不启用跨表并发改造或新的 cursor snapshot 语义。
5. 阶段 4 完成前，不在在线写调用中执行完整 freeze/merge/compact 链。
6. 阶段 5 完成前，optimizer 不得选择未通过 MVCC 一致性验证的 native index generation。
7. 阶段 6 完成前，不删除旧 Schema 列或复用可能仍被旧快照使用的内部 ID。
8. 任一新格式必须有版本、checksum、失败恢复策略和 migration 测试；项目虽不要求旧运行时兼容，但不能没有明确的格式升级边界。
