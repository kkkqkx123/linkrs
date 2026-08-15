# 类型系统完整性整改方案（基于 LadybugDB 对比分析）

> 状态：设计方案（2026-08-15）。
>
> 来源：`docs/analysis/数据类型对比分析_ladybug_vs_linkrs.md`。本文档对分析结论
> 逐条做源码复核（截至当前 HEAD），将可行的改进整理为 P0/P1/P2 三档实施方案。
>
> 复核结论与分析文档的差异（重要）：
> 1. `Value::to_bytes`/`from_bytes`（`value_def.rs:445/522`）**为全仓无调用者的死代码**，
>    WAL/undo 实际走 serde/postcard（`graphdb-transaction/src/transaction/undo_log/file_backed.rs:306/358`）
>    ——「序列化丢数据」是潜在陷阱而非活跃故障，但双轨存在的事实本身是架构债（§4.2）。
> 2. `OrderedCodec` 已支持 `Decimal128`（`ordered_codec.rs:503` `encode_decimal`），
>    分析文档 8.3 的拒绝清单需更正（实际拒绝清单见 §1.4）。
> 3. `PropertyValue` 问题比分析文档更严重：`value_to_property_value`（`graphdb-transaction/src/transaction/codec.rs:16-24`）
>    对未列出的类型**一律映射为 `PropertyValue::Null`**，即非标量属性（Decimal128/Date/List/Map/Vector…）
>    的 undo 旧值被**静默改写为 Null**——回滚会把属性恢复成 Null，是活跃的数据正确性缺陷（§4.3）。

## 0. 结论摘要

| 项 | 现状 | 决策 |
|----|------|------|
| A | `PropertyValue` 6 变体 vs `Value` 33 变体；转换层把未知类型静默映射为 Null，undo 旧值失真 | **P0**：undo 直接复用 `Value`，删除 `PropertyValue` 与 codec 转换函数 |
| B | `DataType::Timestamp`(24)/`VID`(22) 声明但无 `Value` 变体；`VID` 在存储层 element_size=0 且非变长，若建列即写坏数据 | **P0**：删除孤儿类型；DDL 中 `TIMESTAMP` 归一为 `DateTime`，`VID` 关键字拒绝 |
| C | `Value::to_bytes`/`from_bytes` 仅覆盖 13 型、`_ => vec![0]` 静默塌缩，且无调用者 | **P1**：删除该双轨序列化；serde/postcard 为唯一持久化路径，补全量往返测试 |
| D | `is_indexable_type`（type_system.rs:354）含 Timestamp/VID/Geography；`OrderedCodec` 拒绝 Geography、支持 Decimal128——两者**互不一致** | **P1**：以 OrderedCodec 能力为真相源，对齐可索引类型表 |
| E | `can_cast` 白名单仅数值/字符串/日期；`get_common_type` 仅 Int/Float；`binary_operation_result_type` 仅 Float/Int | **P1**：补全转换矩阵与数值提升层级（含 Decimal128/日期/Json↔JsonB） |
| F | `deduce_value_type` 16 变体 + `_ => Empty`；`deduce_arithmetic_type` 仅 Float/Int | **P1**：`deduce_value_type` 覆盖全部 32 变体；算术推导复用类型提升 |
| G | `as_u8` 紧凑顺序码，`from_u8` 未知码静默返回 `Empty` | **P2**：预留新类型编号区间；`from_u8` 改为 `Result` |
| H | `VertexId::Add<u64>` 对非整型 ID `panic`（storage_ids.rs:354） | **P2**：改 `checked_add` 返回 `Option` |
| I | 无 SERIAL/自增序列；VID 完全应用托管 | **P2**：新增 `SERIAL`（仅设计，见 §2.9） |
| J | 无 STRUCT/ARRAY 复合类型；Map 键限 String | **P2（长期）**：`ExtraTypeInfo` 式元数据机制落地后再引入 |

## 1. 现状核查（复核后的代码事实）

### 1.1 孤儿类型：`Timestamp` / `VID`

- `DataType::Timestamp`（types.rs:69，码 24）与 `DataType::VID`（types.rs:67，码 22）
  在枚举中声明，但 `Value` 无对应变体，`get_type`（value_def.rs:96）永不返回二者。
- `Timestamp` 的"半支持"：DDL 解析 `"TIMESTAMP" => DataType::Timestamp`
  （`graphdb-query/.../ddl_parser/helpers.rs:296/371`、`graphdb-api/.../graph_service.rs:1299`）；
  存储层 `element_size` 把 Timestamp 当 28 字节定长（`column_store.rs:66`），
  `value_convert.rs:713` 与 `schema_api.rs:609` 把 Timestamp 归一为 `Value::DateTime`。
  ——语义即「DateTime 别名」，却以独立类型码存在，徒增四链路分叉。
- `VID` 的"危险半支持"：`parse_vid_type_str`（`schema_executor.rs:970`）与
  `SpaceInfo.vid_type`（`space.rs:66`）接受 `VID`；但存储层 `element_size(VID)=0` 且
  `is_variable_length_type(VID)=false`（column_store.rs:56/73）——若 VID 列真的被创建，
  会生成 **0 字节定长列，写入即越界损坏**。`Value::VertexId` 在 `get_type` 中映射为
  `DataType::Vertex`（value_def.rs:126），`DataType::VID` 无任何落地路径。

### 1.2 双轨序列化与死代码

- 路径 A（完整）：`Value` 全部 33 变体均 `#[derive(Serialize, Deserialize)]`（value_def.rs:29），
  WAL/undo 经 postcard（file_backed.rs:306/358）无损持久化。
- 路径 B（残缺）：`Value::to_bytes`（value_def.rs:445）仅 13 型，其余 `_ => vec![0]`；
  `from_bytes`（value_def.rs:522）对未知 tag 返回 `None`。
  **全仓 grep 无任何 `.to_bytes()` 调用者**（`uuid.rs`/`bloom_filter.rs` 的同名方法无关）。
- 双轨并存的危害不在当下调用，而在「新类型只改 serde 忘了手动 codec」的维护陷阱，
  以及 `_ => vec![0]` 掩盖遗漏的静默失败模式。

### 1.3 类型转换 / 提升能力

- `can_cast`（type_system.rs:142）：覆盖整数↔整数/浮点/字符串、浮点↔整数/字符串、
  字符串→数值/Bool/Date/DateTime、FixedString→多数、Bool→数值、Null→任意、Empty→基础型。
  缺：Decimal128 全部、DateTime/Date/Time→String、Date↔DateTime、Json↔JsonB、
  Uuid→String、嵌套/图/向量类型。
- `get_common_type`（type_system.rs:76）：同型、superior（Null/Empty）、Int/Float 之外
  一律 `Empty`。
- `binary_operation_result_type`（type_system.rs:118）：仅 Float/Int 与 `+-*/`。

### 1.4 OrderedCodec 支持面与可索引类型不一致

- `OrderedCodec`（ordered_codec.rs）编码支持：Bool/Int 系/Float/Double/**Decimal128**/
  String/FixedString/Blob/Date/Time/DateTime/Uuid。
- 拒绝（ordered_codec.rs:555-568）：Vertex/Edge/Path/List/Map/Set/**Geography**/Vector/
  DataSet/Json/JsonB/Interval/VertexId/EdgeId。
- `is_indexable_type`（type_system.rs:354）：含 Timestamp/VID/**Geography**，**不含
  Decimal128/Uuid**（后两者 codec 均支持）。
- 结果：`Geography` 字段可通过 `is_indexable_type` 建索引，写入索引时 OrderedCodec
  直接报错；`Decimal128`/`Uuid` 属性实际可索引却被拒绝建索引。**两处规则未对齐。**

### 1.5 表达式类型推导

- `deduce_type`（type_deduce.rs:16）：Variable/Property/TagProperty/EdgeProperty/
  Parameter/SessionVariable/WindowFunction/Reduce → `Empty`（依赖下游 binder 补全，
  下游缺失即静默丢类型）。
- `deduce_value_type`（type_deduce.rs:53）：仅 16 变体，`Double/BigInt/Set/Vector/Json/
  JsonB/Geography/DataSet/FixedString/Blob/Decimal128/VertexId/EdgeId` 落 `_ => Empty`。
- `deduce_arithmetic_type`（type_deduce.rs:109）：仅 Float/Int。

### 1.6 PropertyValue 静默 Null 化（活跃缺陷）

- `value_to_property_value`（graphdb-transaction/src/transaction/codec.rs:16-24）：
  仅 BigInt/Double/String/Blob/Bool/Null，**其余全部 `=> PropertyValue::Null`**。
- 调用链：`writer.rs:139`（写 undo 前转换）→ `UpdateVertexPropUndo.old_value:
  PropertyValue`（undo_log.rs:161）→ 回滚 `ops.rs:288/343` 经 `property_value_to_value`
  还原为 `Value::Null`。
- 后果：对 `Decimal128`/`Date`/`DateTime`/`List`/`Map`/`Vector` 等非标量属性做 UPDATE 后
  回滚，属性被恢复成 **Null**（而非原值）——静默数据丢失。serde 路径（`Value`）本就
  无损，`PropertyValue` 是纯负资产。

## 2. 目标设计

### 2.1 (P0) undo 统一复用 `Value`，删除 `PropertyValue`

- `UndoLogEntry` 各变体（`UpdateVertexPropUndo`/`UpdateEdgePropUndo`/`InsertVertexUndo`/
  `InsertEdgeUndo`）的 `old_value` 字段类型由 `PropertyValue` 改为 `Value`。
- 删除 `PropertyValue` 枚举（property_value.rs）、`value_to_property_value` /
  `property_value_to_value`（codec.rs）及 `undo_log.rs:22` 的 re-export。
- `writer.rs:139` 改为直接写 `old_value: Option<Value>`。
- `PropertyValue` 的 serde 是 postcard 自描述格式，改字段类型后旧文件不可读——项目
  处于开发期（无向后兼容要求），接受格式变更。
- 影响面：grep `PropertyValue` 涉及 `graphdb-transaction`（rollback.rs、manager.rs、
  undo_log.rs、codec.rs、file_backed.rs）与 `graphdb-storage`（writer.rs、ops.rs、
  property_table.rs、test_mock.rs、metrics.rs），编译器驱动逐一替换。

### 2.2 (P0) 消除孤儿类型

- **删除 `DataType::Timestamp`**：
  - DDL/API 中 `"TIMESTAMP"` 关键字一律解析为 `DataType::DateTime`
    （helpers.rs:296/371、graph_service.rs:1299、schema_ext.rs、recovery.rs:1037）。
  - `as_u8`/`from_u8` 移除 24 号（该码位保留占位，见 §2.7）。
  - `is_indexable_type`/`element_size`/`value_convert`/`schema_api` 中 Timestamp 分支删除。
- **删除 `DataType::VID`**：
  - `parse_vid_type_str`（schema_executor.rs:970）遇 `"VID"` 返回明确错误
    （提示使用 INT64/STRING 等实际类型）。
  - `SpaceInfo.vid_type` 保留 `DataType` 字段，但仅允许基础整型/字符串；
    `from_u8`/`as_u8` 移除 22 号（码位保留）。
  - `is_indexable_type` 中 VID 分支删除。
- 若未来需要真正的 epoch 时间戳（NebulaGraph `TIMESTAMP` 语义）或独立 VID 类型，
  以新增 `Value::Timestamp(i64)` 变体的方式重新引入（届时补序列化/转换/索引四链路）。

### 2.3 (P1) 删除 `Value::to_bytes`/`from_bytes`，单轨序列化

- 删除 `value_def.rs:445-646` 的 `to_bytes`/`from_bytes`。
- 持久化唯一路径 = serde（WAL/undo/快照，postcard 编码）。
- 新增穷尽性测试：对 `Value` 全部 33 变体逐一构造样本，postcard 往返断言
  `decode(encode(v)) == v`，并 `#[deny(warnings)]` 下对变体枚举做
  `debug_assert!(matches!(_))` 穷尽检查（防新增变体漏测）。
- 未来若需紧凑线格式（gRPC 传输等），重新设计并全量覆盖 + 往返测试后再引入，
  不得回到"部分覆盖 + 静默兜底"模式。

### 2.4 (P1) 可索引类型与 OrderedCodec 对齐（单一真相源）

- 在 `OrderedCodec` 处定义能力函数：`pub fn supports_ordered_key(t: &DataType) -> bool`，
  `is_indexable_type` 改为调用它（或与之共享同一匹配表）。
- 对齐结果（本方案推荐值）：
  - 新增可索引：`Decimal128`、`Uuid`（codec 已支持）。
  - 移除可索引：`Geography`（codec 拒绝，需 Range/多维索引能力后另行放开）。
  - 孤儿类型 Timestamp/VID 随 §2.2 一并消失。
- 索引创建路径（DDL 校验）与索引写入路径（codec 编码）此后行为一致，
  不一致即编译期/测试期暴露。

### 2.5 (P1) 类型转换矩阵与数值提升

- `can_cast` 增补（保持手写白名单，但按类别分组注释）：
  - `Decimal128 ↔ Int/BigInt/Float/Double/String`（Decimal128→数值按 scale 规则，
    →String 无损）；
  - `DateTime/Date/Time → String`，`String → DateTime/Date/Time/Uuid`；
  - `Date ↔ DateTime`（Date 提为 DateTime 零点）；
  - `Json ↔ JsonB`（双向）；
  - `Uuid → String`；
  - `List/Map/Set` 同构互相转换（仅同 shape 时，见 §2.6 说明）。
- `get_common_type` 按**数值提升层级**扩展：
  `SmallInt < Int < BigInt < Decimal128 < Float < Double`（同符号按高层取，
  浮点/定点交叉取 Double），`Date/DateTime` 层级，`String/Blob` 不提升；
  Null/Empty 仍为 superior。
- `binary_operation_result_type` 算术分支改为调用 `get_common_type`（Decimal128 参与
  `+ - *` 得 Decimal128，`/` 得 Double 的规则由实现细化）。

### 2.6 (P1) 表达式类型推导补全

- `deduce_value_type`（type_deduce.rs:53）改为对全部 32 个 `Value` 变体穷尽匹配，
  删除 `_ => DataType::Empty` 兜底（编译器保证新增变体必须处理）：
  `Double→Double`、`BigInt→BigInt`、`Decimal128→Decimal128`、`Set→Set`、
  `Vector→VectorDense(n)`、`Json/JsonB/Geography/DataSet/FixedString/Blob→对应型`、
  `VertexId→Vertex`、`EdgeId→Edge`。
- `deduce_arithmetic_type` 复用 §2.5 的 `get_common_type`。
- `Variable/Property/*` 的 schema 级类型解析属 binder 职责，本方案仅在文档中标注
  （`Expression::deduce_type` 对无 schema 上下文的结构推导保持 `Empty` 是合理语义，
  但应在 binder 绑定处保证补全并加调试断言，避免静默 `Empty` 流向下游）。

### 2.7 (P2) 类型编号预留与 `from_u8` 严格化

- `as_u8` 不重排现有码（0-31 保持，删除 Timestamp/VID 后的 22/24 作为保留码位）。
- 新类型从 `64` 起分配（分类预留区间，如 64-95 数值、96-127 时间、128-159 嵌套、
  160+ 领域/扩展），`from_u8` 对 22/24（保留）与 ≥64 区间内的未知码**报错**，
  其余未知码返回 `Result::Err`（不再静默 `Empty`）。
- `from_u8` 签名改为 `pub fn from_u8(v: u8) -> Result<DataType, TypeCodecError>`，
  调用点（recovery.rs:1035 等 schema 反序列化路径）改为 `?` 传播；
  老数据遇到新类型码时**显式失败**而非静默降级（前向兼容性硬伤的根因消除）。

### 2.8 (P2) `VertexId`/`EdgeId` 算术安全化

- `impl Add<u64> for VertexId`（storage_ids.rs:354）：非整型 `panic!` →
  新增 `checked_add` 返回 `Option<Self>`；`Add` trait 保持语义但对非整型改为
  `assert`（开发期断言）或删除 trait 实现，改用显式方法。
- `fetch_add`/`AddAssign` 同步约束为非整型调用方编译期不可达（调用点核查）。

### 2.9 (P2) SERIAL 自增列（设计草案，不在本期实施）

- 定位：单节点本地部署，应用托管 VID 与 README 定位存在张力。
- 草案：`CREATE TAG ... (id SERIAL)` 在绑定期展开为内部 `Int64` 列 + 每空间
  一个单调序列生成器（`AtomicU64` 或持久化 `SERIAL_NEXT` 元数据行）；
  插入时未显式提供则自动分配。约束对齐 Ladybug：SERIAL 列不可 DEFAULT、
  参与 cast/索引。本期仅记录设计，待 §2.5/§2.6 落地后单独立卷。

### 2.10 (P2 长期) STRUCT/ARRAY 与 `ExtraTypeInfo` 式元数据

- 前置条件：`DataType` 枚举当前无法承载嵌套子类型（Struct 字段表、Array 元素类型），
  需先引入元数据载体（参考 Ladybug `ExtraTypeInfo` 指针/Arc 方案）：
  `DataType::Struct(Arc<StructTypeInfo>)` / `DataType::Array(Arc<ArrayTypeInfo>)`。
- 联动面：binder（字段解析）、存储（变长编码 + 子列）、OrderedCodec（拒绝或
  结构化排序）、can_cast/get_common_type（`combineTypes` 字段联合）、
  serde（derive 自描述天然支持）、推断（递归 deduce）。
- Map 键泛化（`HashMap<Value, Value>`）与 STRUCT 同属"类型系统元数据化"工程，
  一并列入该长期计划。

## 3. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| 1 | (P0-A) `PropertyValue`→`Value`：改 undo 结构字段、删 codec 转换、替换全部调用点 | `cargo test -p graphdb-transaction --lib` 全量（undo/rollback 单测）；`cargo test -p graphdb-storage --lib`（writer/ops） |
| 2 | (P0-A) 增补非标量属性 UPDATE→回滚 e2e（Decimal128/Date/List/Map/Vector 各一例，断言恢复原值） | 新增测试于 `crates/graphdb-transaction/tests/`；integration 回归 |
| 3 | (P0-B) 删 `DataType::Timestamp`/`VID` + 归一 DDL 关键字 + 清四链路分支 | `cargo check -p graphdb-query -p graphdb-api -p graphdb-storage`（编译器驱动）；DDL `CREATE TAG ... TIMESTAMP/VID` 用例断言错误/归一行为 |
| 4 | (P1-C) 删 `to_bytes`/`from_bytes`；新增 Value 全变体 postcard 往返穷尽测试 | 新测试 `crates/graphdb-core/src/core/value/serialization_roundtrip_test.rs`；lib 全量 |
| 5 | (P1-D) `supports_ordered_key` 单一真相源；is_indexable_type 对齐（+Decimal128/Uuid，-Geography，清孤儿） | OrderedCodec 既有单测；索引 DDL 对 Decimal128/Uuid 建索引 e2e、Geography 建索引报错断言 |
| 6 | (P1-E) `can_cast`/`get_common_type`/`binary_operation_result_type` 矩阵扩展 | type_system.rs 单测补齐（每个新增 pair 一例）；表达式 e2e（Decimal128 算术、Json↔JsonB cast、Date→String） |
| 7 | (P1-F) `deduce_value_type` 穷尽化 + 算术推导复用提升 | type_deduce.rs 单测全变体；已有查询回归（binder/planner 类型断言） |
| 8 | (P2-G) `from_u8 → Result` + 码位预留 | schema 序列化/反序列化单测；未知码用例断言报错而非 Empty |
| 9 | (P2-H) `VertexId` checked 算术 | storage_ids.rs 单测（整型正常、非整型返回 None） |
| 10 | 全量回归 | `cargo test --workspace --lib`、`cargo test --test integration_e2e`、`cargo clippy --workspace --all-targets` |

## 4. 风险与回退

| 风险 | 缓解 |
|------|------|
| `PropertyValue`→`Value` 触及事务/存储两个 crate 多处调用点 | 编译器驱动 + 步骤 1/2 独立成组；undo 结构 serde 自描述，行为等价（Value 覆盖超集） |
| 删孤儿类型后 DDL 语义变化（`TIMESTAMP` 从"半实现类型"变 DateTime 别名） | 现状存储层已按 DateTime 28 字节处理，归一为零行为差异；文档化语义（TIMESTAMP = DATETIME） |
| `from_u8` 改 Result 波及 schema 恢复路径 | 仅 recovery.rs:1035 等少数调用点，`?` 传播；保留码位避免重排既有编号 |
| OrderedCodec 对齐后 `Geography` 不可建索引（行为收紧） | 明确错误信息引导（"Geography 暂不支持有序索引"）；向量索引走独立路径不受影响 |
| 删除 to_bytes 后未来需要紧凑线格式 | 重新设计时以「全量覆盖 + 穷尽测试」为硬性门禁（§2.3），回退成本为零（当前无调用者） |
| 步骤 6/7 矩阵扩展误判提升规则 | 每对提升规则配单测；`binary_operation_result_type` 与 `deduce_arithmetic_type` 收敛到同一函数避免双写漂移 |
| 回退 | 各 P0/P1 步骤独立可回滚；P2 步骤（8/9）行为保持（编号不重排、checked 不破坏正常路径） |
