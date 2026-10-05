# 性能热点与基准测试覆盖缺口分析

**日期**: 2026-10-02
**范围**: 全工作区（graphdb-storage / graphdb-transaction / graphdb-query / graphdb-fulltext / vector-search / graphdb-server）
**说明**: 本文档基于当前代码库的静态排查，给出性能开销较大的环节定位与对应需要补充的基准测试。与 `docs/tests/benches/` 下早期路线图文档互补，不重复其内容。

---

## 一、性能开销较大的环节（按预期收益排序）

### 1. 边属性读取路径的逐行克隆（存储层，最高优先级）

| 位置 | 问题 |
|---|---|
| `crates/graphdb-storage/src/edge/csr_with_properties/read.rs:92-152, 267-286` | 点读主路径 `get_projected_physical_by_edge_id`/批量版与 `read_properties_by_edge_id` 对每条边的每个非空属性克隆 schema 列名 `String`，并深拷贝 `Value`；遍历 expand 后逐边取属性时形成 O(列数 × 边数) 次堆分配 |
| `crates/graphdb-storage/src/edge/csr_with_properties/transfer.rs:12-25` | `export_row`/`import_row` 克隆整行所有列名 + Value |
| `crates/graphdb-storage/src/edge/csr_with_properties/mapping.rs:89-105` | 插入路径 `allocate_row` 每条边深拷贝完整属性值集 |
| `crates/graphdb-storage/src/edge/csr_with_properties/encoding.rs:132-155, 166-199` | `auto_encode_properties`/`adapt_encodings_for_checkpoint` 将整列物化为 `Vec<Option<Value>>`，O(列大小) 克隆，且与持久化统计（min/max/HLL）冗余 |

**方向**: 属性名改用 `Arc<str>`/借用投影；编码选择复用 `ColumnStatsSnapshot`。

### 2. HNSW 搜索路径的邻居列表克隆（向量检索）

| 位置 | 问题 |
|---|---|
| `crates/vector-search/src/index/hnsw.rs:697, 803, 812` | 贪心下降与 beam search 对**每个访问节点每层**克隆整个邻居列表（`read().clone()` → `Vec<u32>`），每次查询 O(访问数 × M) 次分配 |
| `crates/vector-search/src/index/hnsw.rs:868` | `select_neighbors` 每次剪枝 `candidates.to_vec()` + 全排序，建库/插入期间反复调用 |

**方向**: 改为 `Arc<[u32]>` 邻接 + seqlock（820 行附近已有版本号基础设施），或复用调用方 scratch buffer。

### 3. 事务提交临界区与线性扫描（事务层）

| 位置 | 问题 |
|---|---|
| `crates/graphdb-transaction/src/certify.rs:130-133` | 全局 `commit_lock: Mutex<()>`，所有并发写事务在 check-then-publish 临界区串行化（WAL fsync 已在锁外，但临界区本身不分片） |
| `crates/graphdb-transaction/src/certify.rs:414-417, 537` | Serializable 路径对 `committed_write_sets: Mutex<Vec<...>>` 做 O(N) 线性扫描；应改为按 commit_ts 排序的 BTreeMap 范围查询 |
| `crates/graphdb-transaction/src/recovery.rs:221-224` | `take_pending` 用 `position` + `remove` 在 pending Vec 中 O(n) 扫描 |
| `crates/graphdb-transaction/src/mvcc.rs:502-518` | `reap_expired_write_states` 持锁全量遍历 write_states |
| `crates/graphdb-transaction/src/wal/parser.rs:760-762` | `get_entry_by_lsn` 对全部 WAL 条目线性查找，应改 BTreeMap |

### 4. 每次写入同步 fsync（存储层 WAL）

| 位置 | 问题 |
|---|---|
| `crates/graphdb-storage/src/edge/edge_table/wal.rs:112-138` | `append_ops` 每次追加后 `sync_all()`，无组提交 |
| `crates/graphdb-storage/src/index/wal.rs:209-217` | 索引 WAL 每次追加 flush + `sync_all()` |
| `crates/graphdb-storage/src/persistence.rs:91-105` | 原子写临时文件 + 父目录双 `sync_all()`，频繁元数据路径开销大 |

**对照**: 主引擎 `crates/graphdb-storage/src/engine/wal_manager.rs:220-242` 已实现组提交（写锁先释放再做持久化等待），上述 WAL 未采用该模式——这是明确可复制到同层的现成方案。

### 5. 全文检索管理器的全局锁（fulltext）

| 位置 | 问题 |
|---|---|
| `crates/graphdb-fulltext/src/manager.rs:32, 544-552` | 每次搜索读取 `Mutex<Option<Arc<StatsManager>>>`，全局互斥锁只读操作，应改 `RwLock`/`OnceLock`/`ArcSwap` |
| `crates/graphdb-fulltext/src/manager.rs:34-38` | 单索引 publish fence：投递路径持读锁应用批次，重建发布持写锁跨越"最终追赶回放 + 引擎切换"长 I/O+计算段，阻塞该索引全部并发投递 |

### 6. 查询执行器逐项分配（query）

| 位置 | 问题 |
|---|---|
| `crates/graphdb-query/src/executor/streaming/chunk/core.rs:293-296` | `DataChunk::from_batch` 每批做行列转置物化（代码注释自述"在 chunk 列式化前仍需转置"），下游为列式时白费 |
| `crates/graphdb-query/src/executor/streaming/chunk/eval.rs:128-133` | 每个表达式每次求值克隆 selection 向量 |
| `crates/graphdb-query/src/executor/streaming/helpers/accumulator_states.rs:614-636` | percentile/median 每组 `to_vec()` + 全排序，O(n log n)/组 |
| `crates/graphdb-query/src/executor/expression/evaluator/operations.rs:480, 515, 522`、`utility.rs:267-280, 539-544` | JSON 路径访问/拼接逐行 `serde_json::to_string` 往返 |
| `crates/graphdb-core/src/value/geography.rs:1164-1166` | `to_geojson_string` 每次调用序列化，位于属性读取路径 |

### 7. 服务端（较轻）

- `crates/graphdb-server/src/http/handlers/rebuild.rs:189, 441`：请求路径持读锁克隆 storage handle（若为 Arc 则廉价，但模式待确认）。
- `crates/graphdb-api/src/embedded/result.rs:110-123`：结果转换用 `to_string_pretty`，嵌入式 API 每次调用 pretty-print。
- HTTP 热点处理器本身较薄（axum Json），主要 JSON 开销在查询执行器（§6）。

### 8. 其他次要项

- `crates/graphdb-storage/src/edge/pure_csr/maintenance.rs:74`：排序校验克隆整个活跃邻接切片，可单趟判断有序性。
- `crates/graphdb-storage/src/index/shard_runtime/shard.rs:469`：分片 WAL 缓冲单互斥锁，追加与 checkpoint 争用。

---

## 二、需要补充的基准测试

### 2.1 现有覆盖概览

现有 23 个根 crate 基准目标 + vector-search 7 个 + tantivy 内部基准。已覆盖：CSR 插入/扫描/检查点、批量导入 WAL 归因、边写入延迟归因、组提交锁持有时间、写门等待、事务单元级操作、1-2 hop 遍历、OLAP 端到端、并行扩展、行/列式对比、HNSW/IVF 建库与摄取、向量精确扫描、分配统计（仅 vector-search）。

### 2.2 完全缺失（优先补齐）

| 缺口 | 说明 | 建议基准 |
|---|---|---|
| **HTTP/gRPC 服务端** | `api_bench.rs` 只测 JSON serde；路线图目标 HTTP <2ms / gRPC <1ms 但无任何测量 | 真实 axum handler 回环请求、gRPC 端到端往返、路由、并发连接；与 §一.7 对应 |
| **解析→计划→优化独立基准** | `query_bench` 的 parse 组实际调的是 `storage.get_vertex`，解析器/规划器成本只能从端到端总量反推 | 用真实 GQL 语句隔离测 parse、plan、optimize、execute 四阶段耗时 |
| **WAL 独立基准** | WAL 只以"持久 − 内存"差值形式出现于 import/rollback 基准 | 直接测 WAL 追加吞吐、fsync 延迟分布、崩溃回放吞吐；验证 §一.4 组提交改造前后 |
| **崩溃恢复 / 重启** | 仅 edge_property_tables B6 薄覆盖 | WAL 崩溃回放、checkpoint 中断、重启延迟（恢复到可用状态的时间） |
| **大规模与长期运行** | 常规最大 1M；`csr_perf_bench` 核心仅 4096 顶点 | 10M+ 顶点/边规模曲线；长时写入下的压缩增长、索引膨胀、checkpoint 开销老化 |
| **根 crate 分配统计** | 仅 vector-search 有 counting allocator | 对 storage 读取路径、边属性读取（§一.1）、查询执行器（§一.6）做每操作分配字节数/次数基准，作为克隆消除的回归防线 |
| **真实争用下的 MVCC** | `transaction_bench` 无真实争用；`write_gate_bench` 只测门等待占比 | 多客户端写写冲突、快照陈旧度、中止风暴、读写混合并发扩展；验证 §一.3 提交临界区分片改造前后 |

### 2.3 覆盖薄弱（需加深）

| 领域 | 现状 | 补充方向 |
|---|---|---|
| 全文 BM25 集成路径 | 根 `search_bench` 单组小规模且 feature-gated；深度仅在 vendored tantivy 内部基准 | BM25 延迟/召回矩阵、删除/更新重索引、并发搜索（对应 §一.5 锁改造） |
| 遍历深度 | 1-2 hop 覆盖良好 | ≥3 hop、变长路径、最短路、高度数 hub 最坏情况 |
| HNSW 查询侧规模 | 建库/摄取已覆盖，查询侧缺 recall/latency vs ef/M 曲线 | 大 N 下 HNSW 查询基准 + 图查询过滤向量检索组合场景（对应 §一.2 邻居克隆改造） |
| 序列化 | 向量 postcard 有 persist_crc；根 crate 检查点/CSR 编解码无系统基准 | checkpoint 编码/解码吞吐与体积、列编码选择成本（对应 §一.1 encoding 项） |

### 2.4 基准与热点改造的对应关系（验收锚点）

每项热点改造应先落基准、再改造、以基准对照验收：

1. 边属性读克隆（§一.1）→ 新增边属性读取路径分配统计基准 + 现有 `neighbor_batch_bench`/`edge_property_tables_bench` 对照。
2. HNSW 邻居克隆（§一.2）→ 新增大 N 查询侧 recall/latency 基准 + `concurrent_search_bench` 对照。
3. 提交临界区（§一.3）→ 新增多客户端冲突工作负载基准 + `edge_group_commit_bench`/`write_gate_bench` 对照。
4. WAL per-append fsync（§一.4）→ 新增 WAL 独立基准，改造后 fsync 次数应随批内操作数摊薄。
5. fulltext 全局锁（§一.5）→ 并发搜索基准改造前后对比。
6. 列式 chunk 转置（§一.6）→ `columnar_necessity_bench`/`accumulation_bench` 扩展批转置开销项。
