# benches/ 性能测试迁移与整合分析

日期：2026-10-09 ｜ 范围：根目录 `benches/` 下全部 28 个 `[[bench]]` 目标 ＋ 2 个共享辅助模块

结论先行：根 `benches/` 当前混杂了各子 crate 的单元级／模块级基准，
只有真正的跨层端到端基准适合留在根目录；其余应按“被测代码归属”下沉到各包
`benches/`。同时存在 10 处以上可验证的内容重复，需先合并再搬迁，
否则会把重复原样扩散到各包。

## 1. 现状盘点

### 1.1 规模与结构

- 根 `Cargo.toml` 声明 28 个 `[[bench]]`（`autobenches = false`），文件总计约 8600 行。
- 两种 harness 并存：14 个 criterion 基准，12 个 plain-main 基准（`fn main` 手工打印中位数），
  两类结果不可比，CI 无法统一采集。
- 2 个共享辅助模块：`bench_group.rs`（统一 warm-up／measurement 窗口）、
  `results_report.rs`（写 `benches/results/<suite>/results.txt`），各目标以
  `#[path = "..."] mod ...` 按路径引入。
- 接收侧现状：所有 `linkrs-*` 子 crate 均无 `[[bench]]` 声明、无 `criterion` dev 依赖、
  无 `benches/` 目录（`linkrs-storage/benches/` 为空目录）。第三方 `simvec`、
  `tantivy` 自带 `benches/`，不受本次分析影响。
- 文档已漂移：`benches/README.md` 与 `BENCHMARK_REPORT.md` 仍只描述最初 6 个套件，
  与实际 28 个目标严重不符；`api_bench` 文档宣称覆盖 HTTP／gRPC／路由／鉴权等 7 组，
  实际只实现了 2 组 `serde_json` 编解码。

### 1.2 全量归属表

判定规则：只看被测 API 归属哪个 crate 的依赖闭包；同时触及 3 个及以上
工作区 crate 或触及网络传输层的，才有资格留在根目录。

| 基准文件 | 行数 | harness | 实际依赖 | 建议去向 |
|---|---|---|---|---|
| storage_bench.rs | 638 | criterion | core＋storage | linkrs-storage |
| csr_perf_bench.rs | 862 | plain-main | core＋storage（MutableCsr 内部） | linkrs-storage |
| neighbor_batch_bench.rs | 166 | plain-main | core＋storage | linkrs-storage |
| edge_property_tables_bench.rs | 800 | plain-main | core＋storage（EdgeStore／MutableCsr） | linkrs-storage |
| edge_point_write_bench.rs | 213 | plain-main | core＋storage | linkrs-storage |
| edge_group_commit_bench.rs | 201 | plain-main | core＋storage | linkrs-storage |
| edge_scan_speedup_bench.rs | 219 | plain-main | core＋storage＋rayon | linkrs-storage |
| edge_read_alloc_bench.rs | 181 | criterion | core＋storage | linkrs-storage |
| storage_read_baseline.rs | 209 | plain-main | core＋storage | linkrs-storage |
| indexed_bulk_load_bench.rs | 109 | criterion | core＋storage | linkrs-storage |
| import_bench.rs | 209 | plain-main | core＋storage | linkrs-storage |
| write_gate_bench.rs | 155 | plain-main | core＋storage | linkrs-storage |
| wal_bench.rs | 255 | criterion | core（wal）＋storage | linkrs-storage（fsync 微基准可拆 core） |
| crash_recovery_bench.rs | 200 | criterion | core＋storage（GraphStorage::open） | linkrs-storage |
| end_to_end_bench.rs | 239 | criterion | core＋storage（仅） | linkrs-storage（改名，见重复项 D12） |
| transaction_bench.rs | 243 | criterion | core＋transaction | linkrs-transaction |
| txn_conflict_bench.rs | 122 | criterion | core＋transaction | linkrs-transaction |
| rollback_bench.rs | 206 | plain-main | core＋storage＋transaction | linkrs-transaction（storage 作 dev 依赖） |
| query_bench.rs | 766 | criterion | core＋storage＋query＋metrics | linkrs-query |
| query_stage_bench.rs | 257 | criterion | core＋storage＋query＋metrics | linkrs-query |
| traversal_perf_bench.rs | 200 | plain-main | core＋storage＋query＋metrics | linkrs-query |
| parallel_scale_bench.rs | 392 | plain-main | core＋storage＋query＋metrics | linkrs-query |
| olap_e2e_bench.rs | 320 | criterion | core＋storage＋query＋metrics | linkrs-query（根目录保留回归门引用，见 3.5） |
| operator_bench.rs | 216 | criterion | core＋query（DataChunk／SlotLayout） | linkrs-query |
| accumulation_bench.rs | 334 | criterion | core＋query（执行器内部） | linkrs-query |
| columnar_necessity_bench.rs | 588 | criterion | core＋query（DataChunk／SlotLayout） | linkrs-query |
| search_bench.rs（fulltext 部分） | 共 175 | criterion | config＋fulltext | linkrs-fulltext |
| search_bench.rs（vector 部分） | 共 175 | criterion | 无实质依赖（手写 L1 求和桩） | simvec，重写而非搬运 |
| api_bench.rs | 98 | criterion | core（Vertex／Value 的 serde） | linkrs-core（改名 serde_bench）；api／server 另写真实基准 |

关键发现：

- 没有任何一个现有基准触及 `linkrs-api`、`linkrs-server`、`linkrs-wire` 的真实代码。
  `api_bench` 实际是 core 类型的 JSON 编解码测试，放错了位置；
  `linkrs-server`／`linkrs-api` 当前处于零基准覆盖状态。
- `end_to_end_bench` 名不副实：其查询／搜索／并发工作流全部是 storage 直接读写，
  未经过 query／transaction／api，搬迁时应改名（如 `storage_workflow_bench`），
  根目录的真端到端基准需要重写。
- 12 个 `linkrs::` 根立面导入（`linkrs::core`／`linkrs::storage` 等）在搬迁时需改写为
  直接 crate 导入（`linkrs_core`／`linkrs_storage`），机械可完成；
  另有 6 个文件已使用直接导入，可原样搬运。

### 1.3 按包汇总

- linkrs-storage：15 个（含改名的 end_to_end），全部是 GraphStorage／CSR／WAL／恢复。
- linkrs-transaction：3 个（transaction、txn_conflict、rollback）。
- linkrs-query：8 个（含 olap_e2e），全部是 pipeline／执行器／列式微基准。
- linkrs-fulltext：1 个（search_bench 的 fulltext 两组）。
- simvec：1 个（search_bench 的 vector_distance，需重写）。
- linkrs-core：1 个（api_bench 改名 serde_bench）。
- 留根：0 个现状文件符合留根标准；建议根目录仅保留重写后的真端到端门基准
  （见 3.5），现状 28 个文件全部可搬走。

## 2. 重复与整合清单

以下每项均已核对 group 名／函数名／被测 API，三类处理意见：
合并（内容二选一或融合）、改名（名实不符）、重写（桩代码不可搬运）。

### 2.1 事务层重复

- D1 冲突检测测了两次：`transaction_bench` 的 `conflict_detection` 组与
  `txn_conflict_bench` 的 `bench_conflict` 测的是同一 `TransactionManager`
  写写冲突，前者是单点开销，后者带重叠率／客户端数／中止率。
  整合：以 `txn_conflict_bench` 为准，删除 `transaction_bench` 中的冲突组，
  两处对 `certify` 热点的注释互相引用即可。
- D2 WAL 成本三处推导：`wal_bench` 直接测 fsync 与 `SyncPolicy`，
  `import_bench` 与 `rollback_bench` 各自用 persistent-memory 差值间接推导。
  整合：以 `wal_bench` 为唯一 WAL 成本来源，后两者删除重复推导，
  改为引用 wal 基准结论。

### 2.2 存储写入重复

- D3 批量导入测了两次：`indexed_bulk_load_bench` 的 `indexed_bulk_load` 组与
  `storage_bench` 的 `storage_bulk_vertex_insert` 组同为
  `GraphStorage::batch_insert_vertices` 规模曲线。
  整合：合并为 storage_bench 内一组，保留索引开／关对照维度。
- D4 CPU／IO 归因方法完全相同：`import_bench` 与 `edge_point_write_bench`
  同用内存存储 vs 持久存储对照、同测 `batch_insert_vertices`／`batch_insert_edges`。
  整合：合并为一个 `ingest_bench`（CPU 侧 vs WAL＋fsync 侧）， edge 点写
  的单提交 vs 批量提交对照作为其中一组保留。
- D5 提交串行化测了两次：`edge_group_commit_bench`（commit 持有表锁时长）与
  `write_gate_bench`（`AutoCommitWriteGate` 等待占比）是同一全局串行提交的
  两面，且前者的决策门注释直接引用后者的测量值。
  整合：合并为一个提交竞争基准，两组指标同报告输出。

### 2.3 存储读取重复（四套读路径）

- D6 `edge_scan_speedup_bench`（rayon 分区扫描加速比）、`neighbor_batch_bench`
 （batch 访问器 vs `get_node_edges` 全物化）、`storage_bench` 内四个
  cursor／scan 组、`storage_read_baseline`（宽表全扫描 vs 投影扫描 vs 窄表随机读）
  四者覆盖同一读路径，其中投影剪枝、批量邻居、cursor 扫描在语义上大量重叠。
  整合：合并为 storage 内一个读基准，按组保留（全扫描／投影扫描／点查／批量邻居／
  并行加速），`edge_read_alloc_bench`（分配计数器，唯一）保持独立。
- D7 CSR 内部与表布局决策重叠：`csr_perf_bench`（插入／扫描／内存／删除／tombstone）
  与 `edge_property_tables_bench` 的 B1／B2／B4 在同一 `MutableCsr`／`EdgeStore`
  上做布局决策实验。
  整合：二选一保留决策记录，`csr_perf` 独有的基线探针段
  （分配计数、repack 事件、tombstone 复用命中）必须保留，不可随合并丢失。

### 2.4 查询层重复

- D8 两跳遍历四个版本：`query_bench` B5（`two_hop_count`）、`olap_e2e` Q1／Q2、
  `traversal_perf_bench`（100k×3 锚定／非锚定）、`parallel_scale_bench`
  的查询语句走的是同一 pipeline，且数据集各造各的（随机图 vs 固定种子
  优先连接图）。
  整合：统一使用 olap 的固定种子数据集作为共享 fixture；保留分工为
  query_bench 管算子覆盖、olap 管规模基线门、traversal 的规模曲线并入 olap
  或 query_bench 的 large 组后删除独立文件。
- D9 解析成本测了两次：`query_bench` 的 `query_parse` 组与
  `query_stage_bench` 的 `1_parse`／`2_bind`。
  整合：以 stagebench 为按阶段成本唯一来源，query_bench 的 parse 组降级为冒烟或删除。
- D10 执行器微基准三套共享 fixture：`operator_bench`、`accumulation_bench`、
  `columnar_necessity_bench` 共用 `DataChunk`／`SlotLayout` 构造
 （`create_chunk`／`create_row_chunk`／`create_column_chunk`／`create_wide_chunk`
  近乎重复），且 `selection_propagation` 与 `selectivity_propagation`、
  `typed_data_chunk_filter` 与 `row_vs_column_filter` 语义相邻。
  整合：三文件合并为一个 `executor_bench`，fixture 提为共享模块。

### 2.5 桩与名实不符（重写／改名，不搬运）

- D11 `search_bench` 不可直接搬运：无 fulltext 特性时两个同名函数退化为
  `black_box(100)` 桩基准，测的是空值；`vector_distance` 手写 L1 求和，
  与实际向量距离内核无关（simvec 已有 `vector_scan`／`ivf`／`hnsw` 等真基准）。
  整合：fulltext 两组移 linkrs-fulltext；向量部分在 simvec 重写后删除。
- D12 `api_bench` 与 `end_to_end_bench` 名实不符：前者见 1.2，移 core 并改名；
  后者移 storage 并改名。`linkrs-server`／`linkrs-api` 的真实基准
  （HTTP 路由／序列化、gRPC 编解码、并发请求）目前为零，需要新建而非搬运。
- D13 harness 分裂：12 个 plain-main 手工中位数与 14 个 criterion 结果
  在统计口径上不可比。整合：搬迁时统一为 criterion；
  仅 8 核以上扩展性探针（edge_scan_speedup、parallel_scale、write_gate 类）
  可保留 plain-main 并注明机器要求，或改写为 criterion＋自定义报告。
- D14 共享辅助模块去向：`bench_group.rs`、`results_report.rs` 每个接收包各需一份。
  整合：按包复制（两文件共不足 50 行），不新建跨包 dev 依赖；
  同时统一各包 warm-up／measurement 窗口，保持跨包数字可比。

## 3. 搬迁方案

### 3.1 顺序建议：先合后搬

1. 在根目录内先完成 D1、D3、D4、D5、D9、D10 的合并（删除重复组，减少搬运体积）。
2. 再按 1.2 表逐包搬迁，每包一次补齐 `criterion` dev 依赖、`[[bench]]` 声明、
   辅助模块复制、导入改写。
3. 最后处理 D11／D12 的重写与新建（fulltext、simvec、server／api 真基准），
   更新 `benches/README.md` 与 `BENCHMARK_REPORT.md`（两者均已过时，不可沿用旧结构描述）。
4. `benches/results/` 历史报告按去向包归档或注明版本后封存，避免新旧结果混读。

### 3.2 每包落地清单

- linkrs-storage：新建 `benches/`，落 15 个文件合并后的约 6～7 个目标
  （storage 主基准、csr、读基准、ingest、提交竞争、wal、恢复、分配计数）。
  需 dev 依赖 criterion、tempfile（已有）、linkrs-config（已有）。
- linkrs-transaction：新建 `benches/`，落 2 个目标（transaction 主基准、
  txn_conflict；rollback 从 storage 侧移入，需新增 storage dev 依赖）。
- linkrs-query：新建 `benches/`，落 4～5 个目标（query 主基准、query_stage、
  parallel_scale、executor 合并基准、olap_e2e）。
  query 已有 storage test-support dev 依赖，搬迁阻力最小。
- linkrs-fulltext：新建 `benches/`，落 fulltext 两组；注意特性门
  （`fulltext`／`jieba`）在包内基准声明中的透传。
- linkrs-core：新建 `benches/`，落 serde 改名基准。
- simvec：vector_distance 重写为真距离内核基准，与现有 vector benches 去重。

### 3.3 导入改写

- 16 个文件的 `linkrs::core／storage／query／transaction` 改为
  `linkrs_core／linkrs_storage／linkrs_query／linkrs_transaction` 直接导入。
- `#[path]` 引入的 `bench_group`／`report` 模块随文件复制到各包 `benches/`，
  路径引用保持相对关系不变。

### 3.4 门禁与回归

- 将 `olap_e2e_bench` 的 Q1～Q5 与 `traversal_perf` 的两项阈值
  （锚定 1 跳、非锚定 2 跳）定为搬迁后必须通过的回归门，搬迁前后各跑一次对照。
- plain-main 改 criterion 的文件，以原打印中位数为基线做一次等价性确认。

### 3.5 根目录最终形态

根 `benches/` 只保留真正跨层的端到端回归门（建议 1～2 个，由现状重写得到，
而非现状搬迁得到），其余 28 个目标全部下沉。`benches/data/` 的 GQL 数据与
生成脚本移至使用方包或保留根目录作共享 fixture，需在 README 中明确归属。
