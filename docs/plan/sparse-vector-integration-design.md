# simvec 稀疏向量后续实现设计

> 状态：设计方案。依据 `docs/archive/sparse_vector_future_plan.md`，并结合 `crates/simvec` 当前代码与宿主工程链路现状。
>
> 归档方案中的“稀疏向量初步设想”已在 simvec 内落地为可运行的参考实现；本文不再重复设计“是否支持稀疏”，而是解决“如何把它变成可靠、可规模化、可被 Linkrs 主链路使用的功能”。
>
> 实施状态：契约收敛、宿主端到端链路、快照检查点与远程 Qdrant 对齐已落地；WAND/MaxScore 仍按 bench 准入条件保留为后续优化，不默认启用。

## 1. 结论

| 归档项 | 当前状态 | 本文处理 |
|---|---|---|
| 量化（Scalar/Binary/Product） | simvec 稠密存储已实现三类量化与重打分，倒排/稠密路径分离 | 不重复实现；稀疏集合明确不接量化 |
| 运行时 SIMD 多档 | 已改为基线编译 + 运行时 AVX2/AVX-512/NEON 分派，`x86-64-v3` 不再是固定要求 | 不重复实现 |
| 稀疏向量 | `VectorData::Sparse`、`VectorKind::Sparse`、单空间 `KindStore`、命名空间 `NamedStore`、WAL `UpsertKind`/`UpsertNamed`、倒排候选与加权点积已存在 | 剩余工作分为：语义收敛、持久化与检索性能、宿主端到端接入、远程 Qdrant 对齐 |
| 稠密 + BM25 混合 RRF | `linkrs-query/src/fusion.rs` 已有统一融合入口 | 稀疏索引接入后复用同一融合入口，不新增融合算法 |

优先级判断：

1. 先修契约与正确性。当前 simvec 新增字段已经使部分宿主代码的结构体字面量失效，且稀疏评分语义与查询选项处理存在不一致，必须先恢复可信基线。
2. 再做宿主端到端链路。核心层已有稀疏值，但 DDL、存储列、同步、查询、HTTP/gRPC/C API 仍按稠密向量工作，稀疏值目前无法真正写入索引并被检索。
3. 然后做持久化与性能。当前 `KindStore`/`NamedStore` 是“全内存 + 全量 WAL 回放”的参考实现，数据量和恢复时间不可控；WAND 尚未实现。
4. 最后补远程对等能力。Qdrant 的 proto 已经带稀疏字段，但客户端转换仍固定写入 `None`。

不引入“v2”模块，也不为旧数据、旧 ABI 做兼容层；新文件格式统一使用版本 1。

## 2. 现状与缺口

### 2.1 已落地的能力

- `crates/simvec/src/types/point.rs`：`VectorData::{Dense,Sparse,Multi}`，稀疏载荷为严格递增的索引与等长权重。
- `crates/simvec/src/types/config/vectors.rs`：`VectorKind::{Dense,Sparse,Multi}`、`SparseConfig`、`VectorSpaceConfig`、`VectorsConfig::Named`。
- `crates/simvec/src/kind_store.rs` 及子模块：稀疏/多向量的内存槽表、id 反查、term 倒排、精确打分、WAL 写入与全量回放。
- `crates/simvec/src/named_store.rs` 及子模块：按空间名隔离的稠密/稀疏混合集合，稀疏空间复用同一倒排与打分逻辑。
- `crates/simvec/src/storage/wal.rs`：`UpsertKind`、`UpsertNamed` 记录，保持稠密记录变体序号不变。
- `crates/simvec/src/engine/`：按集合类型路由到稠密 `CollectionStore`、稀疏/多向量 `KindStore`、`NamedStore`。

### 2.2 simvec 内必须收敛的语义

| 缺口 | 位置 | 后果 | 修正方向 |
|---|---|---|---|
| `SparseConfig.vocab_size` 与 `CollectionConfig.vector_size` 重复，前者实际未参与校验 | `types/config/vectors.rs`、`types/config.rs`、`engine/core.rs` | 两个上界可能不一致；恢复配置时只能回写 `vector_size` | 去掉 `SparseConfig`，只保留 `vector_size` 作为词表上界；`with_sparse` 不再接受额外配置 |
| `Cosine` 与 `Dot` 在稀疏打分里都是裸点积 | `kind_store/score.rs`、`named_store/score.rs` | 同名指标在稠密/稀疏路径含义不同 | 稀疏集合只接受 `Dot` 与 `Cosine`；`Cosine` 用缓存的向量范数归一；`Euclid`/`Manhattan` 在稀疏集合配置校验时拒绝 |
| 稀疏权重只校验有限性，未禁止负值 | `types/point.rs` | WAND 的上界不成立，且 SPLADE 语义下负值无意义 | 稀疏权重要求有限且非负；零值在宿主转换层规范化掉 |
| `KindStore` 未拒绝 `search_mode`，`NamedStore` 拒绝但两者都不统一使用 `SearchQuery::score_threshold()` | `kind_store/read.rs`、`named_store/read.rs` | `Range` 阈值、TopK/Range 的 limit 口径与稠密路径不一致 | 三个读取路径共用同一查询选项校验；统一用 `effective_limit()` 和 `score_threshold()` |
| `offset` 在排序前按候选槽跳过 | `kind_store/read.rs`、`named_store/read.rs` | 分页跳过的不是排名靠前的命中，结果错误 | 先打分排序，再按排名跳过 offset |
| `payload_selector` 在稀疏/命名读取中未消费 | 同上 | 稀疏搜索永远返回完整 payload，和稠密行为不一致 | 复用稠密结果收尾阶段的投影逻辑 |
| `with_vector=true` 在稀疏搜索中是无操作 | `kind_store/read.rs`、`named_store/read.rs`、`types/query.rs` | 调用方拿不到稀疏载荷，却以为拿到了 | `SearchResult` 增加承载 `VectorData` 的字段；非稠密结果填充该字段 |
| `collection_names`、`list_collections`、`get_index_metadata` 漏掉 kind/named 集合 | `engine/core.rs`、`engine/api.rs` | 管理接口看不到稀疏集合，元数据不可用 | 三个入口统一遍历三类注册表 |
| `collection_info` 对稀疏集合丢掉 kind/sparse 参数，`memory_bytes` 恒为 0 | `engine/core.rs` | 诊断信息不可信 | 回写真实 kind/词表上界；指标接入后返回真实计数 |
| `collection_metrics`、`create_payload_index` 只走稠密 store，`create_collection` 未做跨类型重名检查 | `engine/data.rs`、`engine/maintenance.rs`、`engine/core.rs` | 稀疏集合的指标查询失败；payload 索引返回误导性“集合不存在”；同名集合可能覆盖已有目录 | 指标按集合类型路由；稀疏集合明确返回“不支持 payload 索引”；创建前统一做存在性检查 |
| 写入/查询错误不记录指标 | `kind_store/write.rs`、`named_store/write.rs` | 观测盲区 | 为两类 store 增加与稠密同形的指标计数，并接入 `collection_metrics` |

### 2.3 持久化与性能缺口

- `KindStore::open`、`NamedStore::open` 只做全量 WAL 回放；没有 snapshot/checkpoint，WAL 只增不减，恢复时间随写入量线性增长。
- 所有 live point 常驻内存，倒排也是纯内存结构；归档方案中的 `sparse.bin` mmap 与带跳表的 `sparse_index.bin` 尚未实现。
- 倒排 postings 未排序、无块最大权重；查询时先并集候选再逐条精算，高频 term 会退化为近似全表扫描。
- 没有稀疏专项 bench，无法用数据决定是否值得 mmap/WAND，也无法守住归档方案约定的 10% 准入线。
- payload 相关的 WAL 记录按 slot 寻址；若未来压缩槽位会使这些记录失效，快照方案必须显式固定槽号语义。

### 2.4 宿主链路缺口

- **编译基线**：`vector-client/src/engine/grpc/convert.rs`、`engine/http.rs` 等处的 `VectorPoint` 结构体字面量仍只有 `id/vector/payload`，缺少 simvec 新增的 `data/named` 字段；`SearchResult` 也面临同类扩展。
- **值映射**：`linkrs-sync/src/vector_sync.rs` 的 `VectorPointData.vector` 和 `SearchOptions.query_vector` 都是 `Vec<f32>`；`manager/delivery.rs` 通过 `Value::as_vector()` 取值，稀疏值在此被当成“无向量”并触发删除语义。
- **DDL/查询**：`linkrs-query/src/parser/ast/vector.rs`、`parser/parsing/vector_parser.rs`、`vector_ddl.rs`、planning/streaming spec/operator 只有 `vector_size` 与距离参数，没有向量种类；`SEARCH VECTOR` 的 `query_data` 只支持稠密数组。
- **主存储**：`linkrs-storage/src/vertex/column/variable_width.rs` 对 `DataType::VectorSparse(_)` 仍调用 `to_dense()` 写入、读取时恒定构造 dense 值；稀疏值无法从列存原样往返。
- **协议/API**：`linkrs-wire/src/vector.rs`、HTTP handlers、根 `proto/`、gRPC handlers 的 point/search DTO 只有 `repeated float vector`；`linkrs-api` 的向量 API 同样只接受稠密参数。
- **C ABI**：simvec 的 `simvec_create_collection`/`simvec_search` 走 JSON，可以携带新字段，但 `simvec_upsert`、`simvec_upsert_batch`、`simvec_search_vector` 仍是稠密专用；`include/simvec.h` 与 bindings 文档未覆盖稀疏。

### 2.5 远程对齐缺口

- `vector-client/src/engine/grpc/convert.rs` 固定写 `sparse_vectors_config: None`、`sparse_indices: None`；upsert/search 结果只处理 `Vector.data`。
- `RemoteVectorEngine` 没有能力声明，宿主无法判断某个后端是否支持稀疏/命名空间，只能在运行时失败。
- Qdrant 稀疏查询使用 `SearchPoints.vector` 传权重、`sparse_indices` 传下标；当前转换完全没走这条路径。

## 3. 设计原则

- 一个集合只有一个词表上界：`vector_size`；不保留第二个“vocab_size”。
- 稀疏值规范化只发生一次：在 `linkrs-sync` 的宿主转换边界完成排序、去重、去零和上界校验；simvec 内部只接受规范形式。
- 倒排与 WAL 职责分离：WAL 是提交权威，snapshot 与倒排是可重建派生结构；任何派生文件损坏都不能影响已提交数据。
- 精确语义优先于快捷优化：带 payload 过滤的查询先走精确候选路径，不把过滤器折进 WAND 上界；优化不得改变返回顺序和阈值语义。
- 不创造旁路：宿主只能通过既有 DDL → 索引注册 → outbox/同步 → 查询算子的链路访问稀疏索引；不新增稀疏专用写路径。
- 不在稀疏集合上叠加 HNSW/IVF/量化；这三类优化继续服务稠密集合。

## 4. 分阶段实现方案

### 阶段 A：契约修复与基线恢复

1. **类型收敛**
   - 删除 `SparseConfig`，`CollectionConfig::with_sparse` 只保留维度和距离参数；`collection_config` 回写时不再伪造 sparse 子配置。
   - 在 `CollectionConfig::validate_kind` 与命名空间的空间校验中，稀疏空间只放行 `Dot`/`Cosine`；`NamedStore::create` 复用同一校验。
   - `VectorData::validate` 增加非负权重校验；宿主转换层负责把核心层可能无序的输入变成严格递增形式，非法输入直接报错而不是静默修正评分。
2. **查询行为统一**
   - 抽取一份通用查询选项检查，供稠密、`KindStore`、`NamedStore` 共用：非稠密集合拒绝 `search_mode`/`nprobe`/量化参数；`score_threshold()` 是唯一阈值来源。
   - 三个读取路径统一为“打分 → 阈值 → 排序 → offset → limit → payload 投影”，消除当前 offset 语义分歧。
   - `SearchResult` 增加非稠密载荷字段，`with_vector=true` 时填充 `Sparse`/`Multi` 数据；宿主 DTO 同步扩展。
3. **注册表与可观测性**
   - `collection_names`、`list_collections`、`get_index_metadata`、`collection_info`、`collection_metrics` 统一按三类 store 路由；kind/named store 持有自己的 `Metrics`。
   - `create_collection` 在所有分派之前做跨注册表重名检查；payload 索引对非稠密集合返回明确的“不支持”错误。
4. **编译对齐**
   - 修复 `vector-client` 与服务端中所有 `VectorPoint`/`SearchResult` 结构体字面量；HTTP/gRPC 入站转换把未知 `data` 明确报错，不能静默丢弃。

**验收**：工作区恢复编译；稀疏配置只能声明 dot/cosine；offset/payload 投影/阈值与稠密路径行为一致；稀疏集合出现在管理接口与指标中；新增回归覆盖上述每一项。

### 阶段 B：宿主端到端稀疏链路

1. **DDL 与计划**
   - `VectorIndexConfig` 增加向量种类字段（默认稠密），并扩展 GQL `CREATE VECTOR INDEX ... WITH (kind='sparse', vector_size=..., distance='dot')`；`vector_size` 即词表上界。
   - 稀疏集合显式拒绝 HNSW/IVF/量化配置；若列类型是 `VECTOR_SPARSE(n)`，DDL 的 `vector_size` 必须与 `n` 一致，不一致直接报错。
   - 变更贯穿 AST、parser、`vector_ddl::build_collection_config`、plan node、streaming spec 与 `vector_operator/manage.rs`，保持单一配置构造入口。
2. **查询表达**
   - `VectorQueryExpr` 增加稀疏查询类型；字面量采用 `SPARSE([索引...],[权重...])`，参数绑定允许 `Value::Vector(VectorValue::Sparse)`。
   - `SearchOptions` 从 `query_vector: Vec<f32>` 改为承载 `VectorData`；`SEARCH VECTOR`、`MATCH`、`LOOKUP` 三个算子统一走同一解析入口；索引管理器按集合种类校验查询载荷。
3. **主存储列**
   - `VariableWidthColumn` 为 `DataType::VectorSparse(n)` 使用带类型标签的序列化格式保存 `VectorValue::Sparse`，读取时恢复稀疏形式；`Vector`/`VectorDense` 的现有编码不变。
   - 写入时校验最大下标小于 `n`、权重规范形式；`page_io.rs`、`overflow.rs` 的向量分支同步该判定。
4. **同步与重建**
   - `VectorPointData` 改为承载 `VectorData`；`value/vector` 转换集中在 `linkrs-sync/src/vector_type_conv.rs`，负责 `VectorValue` ↔ `VectorData`、维度上界、规范化和错误报告。
   - `vector_contexts_for_payload`、rebuild 回放、`prepare_change_batch_for_collection` 全部使用同一转换；删除语义仍按点 id，不按载荷内容判断。
   - outbox 事件携带稀疏值时不得退化成删除；rebuild 的计数与恢复流程沿用现有发布/回退机制。
5. **协议与 API**
   - `linkrs-wire` 的 point/search DTO 增加稀疏载荷对象；HTTP handler 与根 `proto/`、gRPC handler 增加相应消息/字段。
   - `linkrs-api` 的索引创建与搜索入口接受种类参数；`linkrs-server` 的信息接口回写 kind 与词表上界。
   - simvec C API 增加 JSON 载荷写入与搜索入口（不改动现有稠密专用函数语义），重新生成 `include/simvec.h` 并更新 `docs/binding-matrix.md`。
6. **混合检索**
   - 稀疏索引与全文索引作为两个 branch 交给现有 `HybridFusionRequest`；不新增融合策略。阈值、limit 与 rank 语义沿用 `fusion.rs`。

**验收**：通过 GQL/HTTP/gRPC/C API 创建稀疏索引、写入顶点稀疏属性、重启恢复后检索命中；稀疏值从列存往返不丢索引和权重；rebuild 前后结果一致；对稀疏集合下稠密查询得到明确错误；稀疏 + BM25 混合检索走 RRF 并稳定排序。

### 阶段 C：持久化与倒排性能

1. **快照与检查点**
   - 在 `crates/simvec/src/storage/` 下新增 kind/named 共用的快照编解码组件，格式包含版本 1、快照 LSN、空间配置、点 id/槽号、载荷、稀疏权重与范数；沿用现有 CRC 与临时文件 + 原子改名发布方式。
   - `KindStore::open`/`NamedStore::open` 先加载快照，再只回放 `txn_id` 大于快照 LSN 的 WAL 事务；崩溃发生在截断前后都能收敛到同一状态。截断后事务号继续递增，不清零，避免旧 LSN 与新日志混淆。
   - 快照保留槽号，不为压缩而重编号；payload WAL 中的 slot 因此在整个生命周期有效。WAL 截断触发条件复用 `StorageConfig` 的字节阈值，维护线程扩展为同时照看 kind/named 集合。
   - 快照只包含 live point；倒排表在打开时由快照重建，不单独持久化。这样派生文件只有一种，恢复逻辑保持简单。
2. **WAND/MaxScore 倒排**
   - 在 `crates/simvec/src/index/sparse.rs` 实现 `KindStore` 与 `NamedStore` 共用的稀疏索引：term 到按槽号排序的 postings，分块保存块内最大权重；每个点保存范数供 Cosine 使用。
   - 查询按 term 合并游标，选 pivot 计算 `sum(query_weight * block_max)` 作为上界；上界不超过当前 Top-K 第 K 名的文档才做精确打分。权重非负是上界成立的前提，由阶段 A 的校验保证。
   - 候选很少、查询 term 很多或存在 payload 过滤时走现有精确并集路径；阈值参数由 bench 决定，不暴露给用户配置。
   - 更新/删除只重建受影响 term 的 postings 与块摘要；删除走 tombstone，槽号不变。
3. **基准与准入**
   - 新增稀疏 bench：短查询、宽词表、Zipf 热点、高/低过滤比例四组，对比当前并集扫描与 WAND，并记录恢复时间随 WAL 大小的变化。
   - 只有 WAND 在目标场景稳定优于精确路径超过 10%，才把它设为默认；否则保留精确路径，WAND 仅作为可选实现继续观测。

**验收**：WAL 大小与恢复时间在大量 upsert/delete 后保持有界；快照损坏时可由 WAL 恢复；WAND 路径结果与精确路径逐项一致（含 tie-break）；bench 结论写入 `docs/benchmark/`。

### 阶段 D：远程 Qdrant 对齐

1. 给 `RemoteVectorEngine` 增加能力声明（支持种类、命名空间），索引管理器在创建前校验；不支持的后端直接拒绝，不静默降级。
2. `vector-client` 的转换层把 `CollectionConfig` 的稀疏种类映射到 `sparse_vectors_config`，把 `VectorData::Sparse` 映射到 upsert 的 `SparseVector`，把查询映射到 `SearchPoints.vector` + `sparse_indices`，并把结果解析回 `VectorData::Sparse`。
3. 命名稀疏空间显式设置 `vector_name`；HTTP 后端走同一份载荷转换，避免 gRPC/REST 语义漂移。
4. 远程与本地的结果分数继续遵守“高分更相似”的契约；Qdrant 原始距离换算沿用现有 `distance_utils.rs`。

**验收**：同一 DDL 在本地与 Qdrant 后端都能创建稀疏集合；写入/查询/结果解析往返一致；后端不支持时给出能力错误而非空结果。

## 5. 关键取舍

- **不做的事**：稀疏量化、稀疏 HNSW/IVF、SPLADE 训练、稀疏 mmap 首期落地、独立稀疏融合算法、旧数据迁移。mmap 只在阶段 C 的 bench 证明必要时再引入，且不新建替代模块，只在现有 `storage`/`index` 内扩展。
- **过滤与 WAND**：payload 过滤无法安全折进 WAND 上界，首期宁可退回精确候选路径；正确性优先于加速。
- **快照与槽号**：固定槽号换取 WAL 记录语义稳定，代价是快照里可能有空洞；空洞只在快照重写时清理，不做运行时压缩。
- **C API 演进**：现有稠密函数保持原含义，新增 JSON 载荷入口覆盖稀疏/命名场景；不把稠密函数改成多形函数，以免误用难以发现。

## 6. 文件级落点

- `crates/simvec/src/types/`：删除冗余 sparse 配置、收紧校验、扩展 `SearchResult`。
- `crates/simvec/src/kind_store/`、`crates/simvec/src/named_store/`：统一查询语义、指标、快照接入。
- `crates/simvec/src/index/sparse.rs`（新增）：共享倒排与 WAND 实现。
- `crates/simvec/src/storage/`：快照编解码、WAL 从 LSN 回放、维护触发。
- `crates/simvec/src/engine/`：注册表、元数据、指标、payload 索引与创建检查。
- `crates/simvec/src/c_api/`、`include/simvec.h`、`crates/simvec/bindings/`、`docs/binding-matrix.md`：JSON 载荷入口与绑定文档。
- `crates/linkrs-core/src/value/vector.rs`：必要时补充稀疏值的规范构造/校验帮助函数，不改变既有值语义。
- `crates/linkrs-storage/src/vertex/column/`：稀疏列编解码与维度校验。
- `crates/linkrs-query/src/parser/`、`ast/`、`planning/`、`executor/streaming/operators/`、`vector_ddl.rs`：稀疏 DDL 与查询字面量。
- `crates/linkrs-sync/src/`：值转换、point data、搜索选项、同步与 rebuild。
- `crates/linkrs-api/src/`、`crates/linkrs-server/src/`、`crates/linkrs-wire/src/`、`proto/`：端到端 API 与协议。
- `crates/vector-client/src/engine/`：远程稀疏转换与能力声明。
- `benches/` 与 `docs/benchmark/`：稀疏检索与恢复基准、结论。

## 7. 验证顺序

按阶段分别验证，避免一次改完再看问题：

1. `cargo test -p simvec --lib` 与 kind/named 集成测试，覆盖语义修正与快照回放。
2. `cargo test -p linkrs-storage -p linkrs-sync -p linkrs-query`，覆盖稀疏列往返、同步、DDL 与搜索。
3. `cargo test -p linkrs-api -p linkrs-server -p vector-client`，覆盖 API/协议与远程转换。
4. `cargo bench -p simvec` 的稀疏基准，以及本地/Qdrant 双后端一致性检查。

每阶段只在上一阶段的契约稳定后启动；最后一阶段的 bench 结论决定是否把 WAND 设为默认路径。
